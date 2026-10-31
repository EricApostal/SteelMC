//! Vanilla `Bogged` (`net.minecraft.world.entity.monster.skeleton.Bogged`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::BoggedEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_game_events, vanilla_items, vanilla_loot_tables,
    vanilla_mob_effects,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::RangedAttackMob;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::skeleton::abstract_skeleton::{
    AbstractSkeleton, DEFAULT_STEP_HEIGHT, SkeletonArrow, skeleton_bases,
};
use crate::entity::living_entity::shearing_loot_items_with_rng;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Bogged.getArrow`: poison effect duration in ticks.
const BOGGED_POISON_DURATION: i32 = 100;
/// Vanilla `Bogged.getHardAttackInterval` (`INCREASED_HARD_ATTACK_INTERVAL`).
const BOGGED_HARD_ATTACK_INTERVAL: i32 = 50;
/// Vanilla `Bogged.getAttackInterval` (`INCREASED_NORMAL_ATTACK_INTERVAL`).
const BOGGED_NORMAL_ATTACK_INTERVAL: i32 = 70;
/// Vanilla `Bogged.SHEARED_TAG_NAME`.
const SHEARED_TAG_NAME: &str = "sheared";

/// Vanilla `Bogged`.
#[entity_behavior(class = "Bogged")]
pub struct BoggedEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<BoggedEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BoggedEntity`.
unsafe impl DowncastType for BoggedEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/bogged");
}

impl BoggedEntity {
    /// Creates a new bogged at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a bogged from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let (living_base, mob_base) = skeleton_bases(entity_type);
        let mut entity_data = BoggedEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
        };
        entity.setup_goals();
        entity
    }

    /// Vanilla `Bogged.isSheared`.
    #[must_use]
    pub fn is_sheared(&self) -> bool {
        *self.entity_data.lock().sheared.get()
    }

    /// Vanilla `Bogged.setSheared`.
    pub fn set_sheared(&self, sheared: bool) {
        self.entity_data.lock().sheared.set(sheared);
    }

    /// Vanilla `Bogged.readyForShearing`.
    #[must_use]
    pub fn ready_for_shearing(&self) -> bool {
        !self.is_sheared()
    }

    /// Runs vanilla `Bogged.shear`: plays the shear sound, drops the shearing loot at the
    /// entity's body height, and marks the bogged sheared.
    pub fn shear(&self, world: &World, tool: &ItemStack) {
        // Vanilla's `shear` uses the entity overload of `playSound`, which sends
        // `ClientboundSoundEntityPacket` so the sound follows the entity.
        world.play_sound_from_entity(
            &sound_events::ENTITY_BOGGED_SHEAR,
            SoundSource::Players,
            self.id(),
            self.position(),
            1.0,
            1.0,
            None,
        );

        let mut rng = rand::rng();
        let body_height = self.bounding_box().height();
        for drop in shearing_loot_items_with_rng(
            self,
            &vanilla_loot_tables::SHEARING_BOGGED,
            tool,
            &mut rng,
        ) {
            // Vanilla `Bogged.spawnShearedMushrooms` spawns the drop at `getBbHeight()`.
            self.spawn_at_location(drop, body_height);
        }

        self.set_sheared(true);
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
}

impl Entity for BoggedEntity {
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

    fn play_step_sound(&self, pos: BlockPos, block_state: BlockStateId) {
        AbstractSkeleton::play_step_sound(self, pos, block_state);
    }

    fn ride_tick(self: Arc<Self>) {
        AbstractSkeleton::ride_tick(self);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert(SHEARED_TAG_NAME, i8::from(self.is_sheared()));
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_sheared(nbt.byte(SHEARED_TAG_NAME).is_some_and(|value| value != 0));

        // Vanilla `AbstractSkeleton.readAdditionalSaveData` re-evaluates the weapon goal.
        self.reassess_weapon_goal();
    }
}

impl LivingEntity for BoggedEntity {
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
        Some(&sound_events::ENTITY_BOGGED_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_BOGGED_DEATH)
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

impl Mob for BoggedEntity {
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
        Some(&sound_events::ENTITY_BOGGED_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        AbstractSkeleton::wants_to_pick_up(self, item_stack)
    }

    fn get_preferred_weapon_type(&self) -> Option<Identifier> {
        AbstractSkeleton::get_preferred_weapon_type(self)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        if item_stack.is(&vanilla_items::SHEARS) && self.ready_for_shearing() {
            let Some(world) = self.level() else {
                return InteractionResult::Pass;
            };
            self.shear(&world, &item_stack);
            // Vanilla `Bogged.mobInteract` sources the shear game event to the player.
            self.game_event_with_source_entity(&vanilla_game_events::SHEAR, Some(player));
            player
                .inventory
                .lock()
                .hurt_item_in_hand(hand, 1, player.has_infinite_materials());
            // Vanilla `Bogged.mobInteract` returns `InteractionResult.SUCCESS`.
            return InteractionResult::Success;
        }

        // Vanilla falls through to `Mob.mobInteract`, which does nothing.
        InteractionResult::Pass
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        AbstractSkeleton::finalize_spawn(self, world, spawn_reason, group_data)
    }
}

impl Monster for BoggedEntity {}

impl PathfinderMob for BoggedEntity {}

impl AbstractSkeleton for BoggedEntity {
    fn get_step_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_BOGGED_STEP
    }

    fn get_hard_attack_interval(&self) -> i32 {
        BOGGED_HARD_ATTACK_INTERVAL
    }

    fn get_attack_interval(&self) -> i32 {
        BOGGED_NORMAL_ATTACK_INTERVAL
    }

    fn get_arrow(
        &self,
        _projectile: &ItemStack,
        power: f32,
        _firing_weapon: Option<&ItemStack>,
    ) -> SkeletonArrow {
        // Vanilla `Bogged.getArrow` adds poison when the released arrow is a plain `Arrow`.
        let arrow = SkeletonArrow::base(power);
        SkeletonArrow {
            effect: Some(MobEffectInstance::with_duration(
                vanilla_mob_effects::POISON,
                BOGGED_POISON_DURATION,
                0,
            )),
            ..arrow
        }
    }
}

impl RangedAttackMob for BoggedEntity {
    fn perform_ranged_attack(&self, target: &SharedEntity, power: f32) -> bool {
        AbstractSkeleton::perform_ranged_attack(self, target, power)
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

    use super::BoggedEntity;
    use crate::entity::Entity;

    #[test]
    fn bogged_shear_state_round_trips_through_nbt() {
        init_vanilla_registry();

        let bogged = BoggedEntity::new(&vanilla_entities::BOGGED, 1, DVec3::ZERO, Weak::new());
        assert!(bogged.ready_for_shearing());

        bogged.set_sheared(true);
        assert!(!bogged.ready_for_shearing());

        let mut nbt = NbtCompound::new();
        bogged.save_additional(&mut nbt);
        assert_eq!(nbt.byte("sheared"), Some(1));

        let loaded = BoggedEntity::new(&vanilla_entities::BOGGED, 2, DVec3::ZERO, Weak::new());
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("reborrow failed: {error}"));
        loaded.load_additional((&borrowed).into());

        assert!(loaded.is_sheared());
        assert!(!loaded.ready_for_shearing());
    }
}
