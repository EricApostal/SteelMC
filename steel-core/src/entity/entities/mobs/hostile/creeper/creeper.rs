//! Vanilla `Creeper` (`net.minecraft.world.entity.monster.Creeper`).

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::CreeperEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_item_tags};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    AvoidEntityGoal, CreeperLike, FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, RandomLookAroundGoal, SwellGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, RemovalReason, SharedEntity,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Creeper.DEFAULT_MAX_SWELL`.
const DEFAULT_MAX_SWELL: i32 = 30;
/// Vanilla `Creeper.DEFAULT_EXPLOSION_RADIUS`.
const DEFAULT_EXPLOSION_RADIUS: i32 = 3;
/// Vanilla `Creeper.getSwelling`: the divisor is `maxSwell - 2`.
const SWELLING_DIVISOR_OFFSET: i32 = 2;
/// Vanilla `Creeper.causeFallDamage` swell gain per block fallen.
const FALL_DISTANCE_SWELL_GAIN: f64 = 1.5;
/// Vanilla `Creeper.causeFallDamage` clamp: `maxSwell - 5`.
const FALL_DISTANCE_SWELL_CLAMP_OFFSET: i32 = 5;
/// Vanilla `Creeper.getComfortableFallDistance` base.
const COMFORTABLE_FALL_DISTANCE: f32 = 3.0;
/// Vanilla `Creeper.explodeCreeper` charged multiplier.
const CHARGED_EXPLOSION_MULTIPLIER: f32 = 2.0;
/// Vanilla `Creeper` `DATA_IS_POWERED`/`DATA_SWELL_DIR` defaults.
const DEFAULT_SWELL_DIR: i32 = -1;

/// Vanilla `Creeper`.
#[entity_behavior(class = "Creeper")]
pub struct CreeperEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<CreeperEntityData>,
    swell: SyncMutex<i32>,
    old_swell: SyncMutex<i32>,
    max_swell: SyncMutex<i32>,
    explosion_radius: SyncMutex<i32>,
    dropped_skulls: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CreeperEntity`.
unsafe impl DowncastType for CreeperEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/creeper");
}

impl CreeperEntity {
    /// Creates a new creeper at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a creeper from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = CreeperEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.swell_dir.set(DEFAULT_SWELL_DIR);
        entity_data.is_powered.set(false);
        entity_data.is_ignited.set(false);

        {
            // Vanilla `Creeper.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(2, SwellGoal::new());
            goal_selector.add_goal(
                3,
                AvoidEntityGoal::with_selector(6.0, 1.0, 1.2, |target, _world| {
                    target.entity_type() == &vanilla_entities::OCELOT
                }),
            );
            goal_selector.add_goal(
                3,
                AvoidEntityGoal::with_selector(6.0, 1.0, 1.2, |target, _world| {
                    target.entity_type() == &vanilla_entities::CAT
                }),
            );
            goal_selector.add_goal(4, MeleeAttackGoal::new(1.0, false));
            goal_selector.add_goal(5, WaterAvoidingRandomStrollGoal::new(0.8));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(6, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
            target_selector.add_goal(2, HurtByTargetGoal::new().set_alert_others(empty()));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            swell: SyncMutex::new(0),
            old_swell: SyncMutex::new(0),
            max_swell: SyncMutex::new(DEFAULT_MAX_SWELL),
            explosion_radius: SyncMutex::new(DEFAULT_EXPLOSION_RADIUS),
            dropped_skulls: SyncMutex::new(false),
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }

        let display = self.living_base.mob_effect_display_state();

        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }

        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Vanilla `Creeper.isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        *self.entity_data.lock().is_powered.get()
    }

    /// Vanilla `Creeper.isIgnited`.
    #[must_use]
    pub fn is_ignited(&self) -> bool {
        *self.entity_data.lock().is_ignited.get()
    }

    /// Vanilla `Creeper.ignite`.
    pub fn ignite(&self) {
        self.entity_data.lock().is_ignited.set(true);
    }

    /// Handles a lightning strike hitting the creeper (vanilla `Creeper.thunderHit`).
    ///
    /// BLOCKED(LightningBolt entity + server-side strike dispatch): Steel has no lightning bolt
    /// entity yet, so nothing calls this hook; the charged flag itself is correct and the call
    /// site matches the `MushroomCowEntity::thunder_hit` convention
    /// (`mobs/passive/mooshroom/mod.rs:189`).
    pub fn thunder_hit(&self) {
        self.entity_data.lock().is_powered.set(true);
    }

    /// Vanilla `Creeper.getSwellDir`.
    #[must_use]
    pub fn swell_dir(&self) -> i32 {
        *self.entity_data.lock().swell_dir.get()
    }

    /// Vanilla `Creeper.setSwellDir`.
    pub fn set_swell_dir(&self, dir: i32) {
        self.entity_data.lock().swell_dir.set(dir);
    }

    /// Vanilla `Creeper.getSwelling`.
    #[must_use]
    pub fn swelling(&self, partial_ticks: f32) -> f32 {
        let old = *self.old_swell.lock();
        let current = *self.swell.lock();
        let max_swell = *self.max_swell.lock();
        let lerped = old as f32 + (current - old) as f32 * partial_ticks;
        lerped / (max_swell - SWELLING_DIVISOR_OFFSET) as f32
    }

    /// Vanilla `Creeper` fuse counters (`swell`/`oldSwell`/`maxSwell`).
    #[must_use]
    pub fn swell(&self) -> i32 {
        *self.swell.lock()
    }

    /// Sets the fuse counter directly (used by save data and lightning charge tests).
    pub fn set_swell(&self, swell: i32) {
        *self.swell.lock() = swell;
    }

    /// Vanilla `Creeper.maxSwell`.
    #[must_use]
    pub fn max_swell(&self) -> i32 {
        *self.max_swell.lock()
    }

    /// Vanilla `Creeper.explosionRadius`.
    #[must_use]
    pub fn explosion_radius(&self) -> i32 {
        *self.explosion_radius.lock()
    }

    /// Vanilla `Creeper.tick` body: drives the fuse from the synced swell direction.
    fn tick_swell(self: &Arc<Self>) {
        if !Entity::is_alive(self.as_ref()) {
            return;
        }

        *self.old_swell.lock() = *self.swell.lock();
        if self.is_ignited() {
            self.set_swell_dir(1);
        }

        let swell_dir = self.swell_dir();
        {
            let mut swell = self.swell.lock();
            if swell_dir > 0 && *swell == 0 {
                drop(swell);
                // Vanilla plays the primed sound locally and fires the `prime_fuse` game event.
                self.play_sound(&sound_events::ENTITY_CREEPER_PRIMED, 1.0, 0.5);
                self.game_event(&steel_registry::vanilla_game_events::PRIME_FUSE);
                swell = self.swell.lock();
            }

            *swell += swell_dir;
            if *swell < 0 {
                *swell = 0;
            }

            let max_swell = *self.max_swell.lock();
            if *swell >= max_swell {
                *swell = max_swell;
            }
        }

        if *self.swell.lock() >= *self.max_swell.lock() {
            self.explode_creeper();
        }
    }

    /// Vanilla `Creeper.explodeCreeper`.
    ///
    /// BLOCKED: `ServerLevel.explode` has no Steel equivalent (`World::explode` does not exist —
    /// see `behavior/blocks/building/bed_block.rs:329` and `respawn_anchor_block.rs:32`), and the
    /// lingering effect cloud entity `AreaEffectCloud` has no factory. The explosion radius
    /// multiplier and the remaining death sequence are implemented; the two blocked calls are
    /// marked below and reported as dependencies.
    fn explode_creeper(self: &Arc<Self>) {
        if self.level().is_none() {
            return;
        }

        let _explosion_multiplier = if self.is_powered() {
            CHARGED_EXPLOSION_MULTIPLIER
        } else {
            1.0
        };

        // Vanilla: `level.explode(this, x, y, z, explosionRadius * explosionMultiplier,
        // ExplosionInteraction.MOB)` then `spawnLingeringCloud()`, then
        // `triggerOnDeathMobEffects(level, RemovalReason.KILLED)`.
        // BLOCKED(World::explode) — no explosion foundation exists in Steel.
        // BLOCKED(AreaEffectCloud entity factory) — no `AreaEffectCloud` implementation exists.
        self.set_removed(RemovalReason::Killed);
    }

    /// Vanilla `Creeper.setTarget` refuses to target goats.
    fn set_target_refusing_goat(&self, target: Option<&SharedEntity>) -> bool {
        if let Some(target) = target
            && target.entity_type() == &vanilla_entities::GOAT
        {
            return false;
        }
        Mob::set_target(self, target)
    }
}

impl Entity for CreeperEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        // Vanilla `Creeper.tick` runs the fuse before `super.tick()`.
        Arc::clone(&self).tick_swell();
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        Monster::sound_source_monster(self)
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        Monster::fall_sounds_monster(self)
    }

    fn swim_sound(&self) -> SoundEventRef {
        Monster::swim_sound_monster(self)
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        // Vanilla `Creeper` inherits `Monster`'s empty step sound; creepers only play the
        // primed sound and their hurt/death sounds.
    }

    fn cause_fall_damage(
        self: Arc<Self>,
        fall_distance: f64,
        damage_modifier: f32,
        source: &DamageSource,
    ) -> bool {
        let damaged = LivingEntity::cause_living_fall_damage(
            self.as_ref(),
            fall_distance,
            damage_modifier,
            source,
        );

        // Vanilla `Creeper.causeFallDamage` accelerates the fuse on impact.
        let mut swell = self.swell.lock();
        *swell += (fall_distance * FALL_DISTANCE_SWELL_GAIN) as i32;
        let clamp = *self.max_swell.lock() - FALL_DISTANCE_SWELL_CLAMP_OFFSET;
        if *swell > clamp {
            *swell = clamp;
        }

        damaged
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("powered", self.is_powered());
        nbt.insert("Fuse", *self.max_swell.lock() as i16);
        nbt.insert("ExplosionRadius", *self.explosion_radius.lock() as i8);
        nbt.insert("ignited", self.is_ignited());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.entity_data
            .lock()
            .is_powered
            .set(nbt.byte("powered").is_some_and(|value| value != 0));
        *self.max_swell.lock() = i32::from(nbt.short("Fuse").unwrap_or(DEFAULT_MAX_SWELL as i16));
        *self.explosion_radius.lock() = i32::from(
            nbt.byte("ExplosionRadius")
                .unwrap_or(DEFAULT_EXPLOSION_RADIUS as i8),
        );
        if nbt.byte("ignited").is_some_and(|value| value != 0) {
            self.ignite();
        }
    }
}

impl LivingEntity for CreeperEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CREEPER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_CREEPER_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for CreeperEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        None
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn set_target(&self, target: Option<&SharedEntity>) -> bool {
        self.set_target_refusing_goat(target)
    }

    /// Vanilla `Creeper.getMaxFallDistance`: a creeper with a target accepts
    /// `health - 1` extra fall damage to close the distance.
    fn max_fall_distance(&self) -> i32 {
        let allowed = if self.target().is_some() {
            self.get_health() - 1.0
        } else {
            0.0
        };
        (allowed + COMFORTABLE_FALL_DISTANCE).floor() as i32
    }

    /// Vanilla `Creeper.doHurtTarget` never damages directly — it only squeezes.
    fn do_hurt_target(
        &self,
        _entity: &SharedEntity,
        _world: &World,
        _target: &SharedEntity,
    ) -> bool {
        true
    }

    /// Vanilla `Creeper.mobInteract`: `#minecraft:creeper_igniters` primed the creeper.
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let is_igniter = {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            !stack.is_empty()
                && stack
                    .item()
                    .has_tag(&vanilla_item_tags::ItemTag::CREEPER_IGNITERS)
        };
        if !is_igniter {
            return InteractionResult::Pass;
        }

        let is_fire_charge = {
            let inventory = player.inventory.lock();
            inventory
                .get_item_in_hand(hand)
                .is(&steel_registry::vanilla_items::FIRE_CHARGE)
        };
        if let Some(world) = self.level() {
            let sound = if is_fire_charge {
                &sound_events::ITEM_FIRECHARGE_USE
            } else {
                &sound_events::ITEM_FLINTANDSTEEL_USE
            };
            world.play_sound_at(
                sound,
                SoundSource::Neutral,
                self.position(),
                1.0,
                rand::random::<f32>() * 0.4 + 0.8,
                Some(player.id()),
            );
        }

        // Steel's server-side entity code only runs on the logical server, so the vanilla
        // `!level.isClientSide()` guard needs no counterpart here.
        self.ignite();
        {
            let has_infinite_materials = player.has_infinite_materials();
            let inventory = player.inventory.lock();
            let damageable = !inventory.get_item_in_hand(hand).is_damageable_item();
            drop(inventory);
            if damageable {
                player
                    .inventory
                    .lock()
                    .mutate_item_in_hand(hand, |stack| stack.consume(1, has_infinite_materials));
            } else {
                // Vanilla: `itemStack.hurtAndBreak(1, player, hand.asEquipmentSlot())`.
                // BLOCKED(item durability) — no `hurt_and_break` entry point is exposed on the
                // player inventory yet, so the durability loss for damageable igniters is the
                // remaining vanilla step.
            }
        }

        InteractionResult::Success
    }
}

impl Monster for CreeperEntity {}

impl PathfinderMob for CreeperEntity {}

impl CreeperLike for CreeperEntity {
    fn swell_dir(&self) -> i32 {
        CreeperEntity::swell_dir(self)
    }

    fn set_swell_dir(&self, swell_dir: i32) {
        CreeperEntity::set_swell_dir(self, swell_dir);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use glam::DVec3;
    use simdnbt::borrow::read_compound;
    use simdnbt::owned::NbtCompound;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::{CreeperEntity, DEFAULT_EXPLOSION_RADIUS, DEFAULT_MAX_SWELL};
    use crate::entity::Entity;

    #[test]
    fn creeper_nbt_round_trip() {
        init_vanilla_registry();

        let creeper = CreeperEntity::new(&vanilla_entities::CREEPER, 1, DVec3::ZERO, Weak::new());
        assert!(!creeper.is_powered());
        assert!(!creeper.is_ignited());
        assert_eq!(creeper.max_swell(), DEFAULT_MAX_SWELL);
        assert_eq!(creeper.explosion_radius(), DEFAULT_EXPLOSION_RADIUS);

        creeper.entity_data.lock().is_powered.set(true);
        creeper.set_swell(12);
        creeper.ignite();

        let mut nbt = NbtCompound::new();
        creeper.save_additional(&mut nbt);
        assert_eq!(nbt.byte("powered"), Some(1));
        assert_eq!(nbt.short("Fuse"), Some(30));
        assert_eq!(nbt.byte("ExplosionRadius"), Some(3));
        assert_eq!(nbt.byte("ignited"), Some(1));

        let loaded = CreeperEntity::new(&vanilla_entities::CREEPER, 2, DVec3::ZERO, Weak::new());
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("reborrow failed: {error}"));
        loaded.load_additional((&borrowed).into());

        assert!(loaded.is_powered());
        assert!(loaded.is_ignited());
        assert_eq!(loaded.max_swell(), DEFAULT_MAX_SWELL);
        assert_eq!(loaded.explosion_radius(), DEFAULT_EXPLOSION_RADIUS);
    }

    #[test]
    fn swelling_interpolates_between_fuse_states() {
        init_vanilla_registry();

        let creeper = CreeperEntity::new(&vanilla_entities::CREEPER, 3, DVec3::ZERO, Weak::new());
        creeper.set_swell(0);
        assert!((creeper.swelling(0.5)).abs() < f32::EPSILON);

        creeper.set_swell(DEFAULT_MAX_SWELL);
        assert!((creeper.swelling(1.0) - (DEFAULT_MAX_SWELL as f32 / 28.0)).abs() < 1.0e-4);
    }
}
