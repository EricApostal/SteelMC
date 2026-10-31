//! Vanilla `SnowGolem` (`net.minecraft.world.entity.animal.golem.SnowGolem`).
//!
//! Vanilla's `AbstractGolem` layer only contributes the 120-tick ambient interval and the
//! "never despawns" rule; the snow golem adds the synced pumpkin flag, the snowball ranged
//! attack, shearing with shears and the snow trail it leaves while mob griefing is enabled.
//!
//! ## Deviations
//!
//! - GAP(snow-golem-melt): `SnowGolem.aiStep`'s `EnvironmentAttributes.SNOW_GOLEM_MELTS` damage —
//!   Steel has no environment-attribute system, so the golem never melts.
//! - GAP(snow-golem-leash-offset): `SnowGolem.getLeashOffset` —
//!   [`SnowGolemEntity::leash_offset`] records vanilla's `(0, 0.75 * eyeHeight, bbWidth * 0.4)`,
//!   but neither `Entity` nor `Leashable` exposes a `getLeashOffset` hook, so nothing consumes it
//!   and the default rope attachment stays in force.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SnowGolemEntityData;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_entities, vanilla_game_events, vanilla_game_rules,
    vanilla_items, vanilla_loot_tables,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};

use crate::behavior::{BLOCK_BEHAVIORS, InteractionResult};
use crate::entity::ai::goal::{
    LookAtPlayerGoal, NearestAttackableTargetGoal, RandomLookAroundGoal, RangedAttackGoal,
    RangedAttackMob, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::SnowballEntity;
use crate::entity::living_entity::shearing_loot_items_with_rng;
use crate::entity::projectile::spawn_projectile;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, Projectile as _, SharedEntity,
    ThrowableItemProjectile as _, next_entity_id,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Vanilla `SnowGolem.PUMPKIN_FLAG`: bit 16 of the synced data byte marks the pumpkin.
const PUMPKIN_FLAG: i8 = 16;
/// Vanilla `SnowGolem.DEFAULT_PUMPKIN`.
const DEFAULT_PUMPKIN: bool = true;
/// Vanilla `SnowGolem.rangedAttackUncertainty`.
const RANGED_ATTACK_UNCERTAINTY: f32 = 12.0;
/// Vanilla `SnowGolem.performRangedAttack` snowball velocity.
const SNOWBALL_POWER: f32 = 1.6;
/// Vanilla `SnowGolem.performRangedAttack` shot pitch: `0.4F / (random * 0.4F + 0.8F)`.
const SHOOT_SOUND_PITCH_SCALE: f32 = 0.4;
const SHOOT_SOUND_PITCH_BASE: f32 = 0.8;
/// Vanilla `AbstractGolem.getAmbientSoundInterval`.
const AMBIENT_SOUND_INTERVAL: i32 = 120;
/// Vanilla `SnowGolem.aiStep` X sample offset `(i % 2 * 2 - 1) * 0.25F`, indexed by `i % 2`.
const SNOW_TRAIL_X_OFFSETS: [f64; 2] = [-0.25, 0.25];
/// Vanilla `SnowGolem.aiStep` Z sample offset `(i / 2 % 2 * 2 - 1) * 0.25F`, indexed by `i / 2`.
const SNOW_TRAIL_Z_OFFSETS: [f64; 2] = [-0.25, 0.25];
/// Vanilla `SnowGolem.aiStep` samples four block positions around the golem.
const SNOW_TRAIL_SAMPLES: usize = 4;

/// Vanilla snow golem: a pumpkin-wearing mob that throws snowballs at monsters.
#[entity_behavior(class = "SnowGolem")]
pub struct SnowGolemEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SnowGolemEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SnowGolemEntity`.
unsafe impl DowncastType for SnowGolemEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/snow_golem");
}

impl SnowGolemEntity {
    /// Creates a new snow golem at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a snow golem from persisted base entity state.
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
        let mut entity_data = SnowGolemEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `SnowGolem.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, RangedAttackGoal::new(1.25, 20, 10.0));
            goal_selector.add_goal(
                2,
                WaterAvoidingRandomStrollGoal::with_probability(1.0, 1.000_000_1e-5),
            );
            goal_selector.add_goal(3, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(4, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            // Vanilla `NearestAttackableTargetGoal<>(this, Mob.class, 10, true, false,
            // (target, level) -> target instanceof Enemy)`.
            target_selector.add_goal(
                1,
                NearestAttackableTargetGoal::new_with_interval(
                    10,
                    true,
                    false,
                    |target, _world| target.as_mob().is_some() && target.as_enemy().is_some(),
                ),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Vanilla `SnowGolem.hasPumpkin`.
    #[must_use]
    pub fn has_pumpkin(&self) -> bool {
        (*self.entity_data.lock().pumpkin.get() & PUMPKIN_FLAG) != 0
    }

    /// Vanilla `SnowGolem.setPumpkin`.
    pub fn set_pumpkin(&self, pumpkin: bool) {
        let mut entity_data = self.entity_data.lock();
        let current = *entity_data.pumpkin.get();
        let next = if pumpkin {
            current | PUMPKIN_FLAG
        } else {
            current & !PUMPKIN_FLAG
        };
        entity_data.pumpkin.set(next);
    }

    /// Vanilla `SnowGolem.readyForShearing`: the pumpkin must still be worn.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        self.has_pumpkin()
    }

    /// Vanilla `SnowGolem.isSensitiveToWater`.
    ///
    /// `LivingEntity` has no capability hook for this yet; thrown water potions resolve it by
    /// entity type in [`crate::entity::entities::objects::projectiles::thrown_potion`].
    #[must_use]
    pub const fn is_sensitive_to_water() -> bool {
        true
    }

    /// Vanilla `SnowGolem.getLeashOffset`.
    ///
    /// No `Entity`/`Leashable` hook consumes this yet — see the module-level GAP.
    #[must_use]
    pub fn leash_offset(&self) -> DVec3 {
        // Vanilla multiplies the float bounding-box width: `getBbWidth() * 0.4F`.
        let width = self.bounding_box().width() as f32;
        DVec3::new(0.0, 0.75 * self.get_eye_height(), f64::from(width * 0.4))
    }

    /// Runs vanilla `SnowGolem.shear`: plays the shear sound, removes the pumpkin and drops the
    /// shearing loot table at eye height.
    pub fn shear(&self, world: &World, tool: &ItemStack) {
        // Vanilla's `shear` uses the entity overload of `playSound`, which sends
        // `ClientboundSoundEntityPacket` so the sound follows the entity.
        world.play_sound_from_entity(
            &sound_events::ENTITY_SNOW_GOLEM_SHEAR,
            SoundSource::Players,
            self.id(),
            self.position(),
            1.0,
            1.0,
            None,
        );

        self.set_pumpkin(false);

        let mut rng = rand::rng();
        for drop in shearing_loot_items_with_rng(
            self,
            &vanilla_loot_tables::SHEARING_SNOW_GOLEM,
            tool,
            &mut rng,
        ) {
            // Vanilla `SnowGolem.shear` spawns the drop at `getEyeHeight()`.
            self.spawn_at_location(drop, self.get_eye_height());
        }
    }

    /// Vanilla `SnowGolem.aiStep`'s server half: leaves snow behind while mob griefing is on.
    fn tick_snow_trail(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if !world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
            return;
        }

        let snow = vanilla_blocks::SNOW.default_state();
        let behavior = BLOCK_BEHAVIORS.get_behavior(snow.get_block());
        let position = self.position();
        let y = steel_math::fast_floor(position.y);

        for index in 0..SNOW_TRAIL_SAMPLES {
            // Vanilla: `(i % 2 * 2 - 1) * 0.25F` on X and `(i / 2 % 2 * 2 - 1) * 0.25F` on Z.
            let x = steel_math::fast_floor(position.x + SNOW_TRAIL_X_OFFSETS[index % 2]);
            let z = steel_math::fast_floor(position.z + SNOW_TRAIL_Z_OFFSETS[index / 2 % 2]);
            let snow_pos = BlockPos::new(x, y, z);

            if world.get_block_state(snow_pos).is_air()
                && behavior.can_survive(snow, world.as_ref(), snow_pos)
            {
                world.set_block(snow_pos, snow, UpdateFlags::UPDATE_ALL);
                // Vanilla `GameEvent.Context.of(this, snow)`.
                world.game_event(
                    &vanilla_game_events::BLOCK_PLACE,
                    snow_pos,
                    &GameEventContext::new(Some(self), Some(snow)),
                );
            }
        }
    }
}

impl Entity for SnowGolemEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
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

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Pumpkin", self.has_pumpkin());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        // Vanilla `input.getBooleanOr("Pumpkin", true)`.
        self.set_pumpkin(
            nbt.byte("Pumpkin")
                .map_or(DEFAULT_PUMPKIN, |value| value != 0),
        );
    }
}

impl LivingEntity for SnowGolemEntity {
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SNOW_GOLEM_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SNOW_GOLEM_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        // Vanilla `SnowGolem.aiStep` appends the server-only melting roll (GAP) and snow trail.
        self.tick_snow_trail();
        result
    }
}

impl Mob for SnowGolemEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound_interval(&self) -> i32 {
        AMBIENT_SOUND_INTERVAL
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SNOW_GOLEM_AMBIENT)
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        // Vanilla `AbstractGolem.removeWhenFarAway` is always false.
        false
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if !item_stack.is(&vanilla_items::SHEARS) || !self.ready_for_shearing() {
            return InteractionResult::Pass;
        }

        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        self.shear(&world, &item_stack);
        // Vanilla `SnowGolem.mobInteract` sources the shear game event to the player.
        self.game_event_with_source_entity(&vanilla_game_events::SHEAR, Some(player));
        player
            .inventory
            .lock()
            .hurt_item_in_hand(hand, 1, player.has_infinite_materials());
        // Vanilla `SnowGolem.mobInteract` returns `InteractionResult.SUCCESS`.
        InteractionResult::Success
    }
}

impl PathfinderMob for SnowGolemEntity {}

impl RangedAttackMob for SnowGolemEntity {
    /// Vanilla `SnowGolem.performRangedAttack`: throw a snowball at the target, then always play
    /// `SNOW_GOLEM_SHOOT` (the sound is not conditioned on the projectile).
    fn perform_ranged_attack(&self, target: &SharedEntity, _power: f32) -> bool {
        let Some(world) = self.level() else {
            return false;
        };

        let position = self.position();
        let xd = target.position().x - position.x;
        let yd = target.get_eye_y() - 1.1;
        let zd = target.position().z - position.z;
        let yo = (xd * xd + zd * zd).sqrt() * 0.2;

        let spawned = world.get_entity_by_id(self.id()).and_then(|owner| {
            let mut item_stack = ItemStack::new(&vanilla_items::SNOWBALL);
            // Vanilla `new Snowball(ServerLevel, LivingEntity, ItemStack)` spawns at the
            // thrower's eye height minus 0.1 and renders the snowball item.
            let snowball = SnowballEntity::new(
                &vanilla_entities::SNOWBALL,
                next_entity_id(),
                DVec3::new(position.x, self.get_eye_y() - 0.1, position.z),
                Arc::downgrade(&world),
            );
            snowball.set_owner_entity(Some(&owner));
            snowball.set_item(item_stack.copy_with_count(1));

            spawn_projectile(&world, snowball, &mut item_stack, |snowball| {
                // Vanilla `projectile.shoot(xd, yd + yo - projectile.getY(), zd, 1.6F, 12.0F)`.
                snowball.shoot(
                    DVec3::new(xd, yd + yo - snowball.position().y, zd),
                    SNOWBALL_POWER,
                    RANGED_ATTACK_UNCERTAINTY,
                );
            })
        });

        // Vanilla plays `SNOW_GOLEM_SHOOT` unconditionally, whether or not the snowball spawned.
        self.play_sound(
            &sound_events::ENTITY_SNOW_GOLEM_SHOOT,
            1.0,
            SHOOT_SOUND_PITCH_SCALE
                / (rand::random::<f32>() * SHOOT_SOUND_PITCH_SCALE + SHOOT_SOUND_PITCH_BASE),
        );
        spawned.is_some()
    }
}
