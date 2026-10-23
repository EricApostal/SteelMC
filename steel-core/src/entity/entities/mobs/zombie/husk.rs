//! Vanilla `Husk` (`net.minecraft.world.entity.monster.zombie.Husk`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{vanilla_entities, vanilla_items, vanilla_mob_effects};
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::zombie::ZombieEntity;
use crate::entity::entities::mobs::zombie::zombie::{ZombieVariant, zombie_effective_difficulty};
use crate::entity::registry::ENTITIES;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::world::World;

/// Vanilla `Husk.doHurtTarget`: `140 * (int)effectiveDifficulty` hunger ticks.
const HUSK_HUNGER_BASE_TICKS: i32 = 140;
/// Vanilla `Husk.finalizeSpawn`: `random.nextFloat() < 0.1F`.
const CAMEL_HUSK_JOCKEY_CHANCE: f32 = 0.1;
/// Vanilla `Husk.HuskGroupData.triedToSpawnCamelHusk` is only `false` for a
/// natural spawn; every other reason skips the camel-husk attempt.
const CAMEL_HUSK_SPAWN_REASON: EntitySpawnReason = EntitySpawnReason::Natural;

/// Vanilla `Husk`.
///
/// Steel has no inheritance, so this owns the shared `Zombie` body
/// ([`ZombieEntity::new_with_variant`]) and adds the husk-only behaviour.
#[entity_behavior(class = "Husk")]
pub struct HuskEntity {
    zombie: ZombieEntity,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HuskEntity`.
unsafe impl DowncastType for HuskEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/husk");
}

impl HuskEntity {
    /// Creates a new husk at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a husk from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            zombie: ZombieEntity::new_with_variant(base, entity_type, ZombieVariant::Husk),
        }
    }

    /// The shared `Zombie` body carrying the husk's state.
    #[must_use]
    pub fn zombie(&self) -> &ZombieEntity {
        &self.zombie
    }

    /// Vanilla `Husk.doHurtTarget` tail: an empty-handed husk inflicts hunger
    /// scaled by the local difficulty.
    fn apply_hunger(&self, world: &World, target: &SharedEntity) {
        if !self
            .zombie
            .equipment_in_slot(EquipmentSlot::MainHand)
            .is_empty()
        {
            return;
        }
        let Some(living) = target.as_living_entity() else {
            return;
        };
        let difficulty = zombie_effective_difficulty(world) as i32;
        living.add_mob_effect(crate::entity::MobEffectInstance::with_duration(
            vanilla_mob_effects::HUNGER,
            HUSK_HUNGER_BASE_TICKS * difficulty,
            0,
        ));
    }

    /// Vanilla `Husk.finalizeSpawn` tail: a naturally spawned husk may mount a
    /// `CamelHusk` carrying a `Parched` passenger.
    ///
    /// Steel's spawner finalizes one mob at a time and does not pass spawn-group
    /// data along, so vanilla's once-per-group `HuskGroupData.triedToSpawnCamelHusk`
    /// guard is unnecessary here; only natural spawns attempt the camel.
    fn try_spawn_camel_husk(&self, world: &Arc<World>, spawn_reason: EntitySpawnReason) {
        if spawn_reason != CAMEL_HUSK_SPAWN_REASON {
            return;
        }

        // Vanilla also requires `level.noCollision(camelHusk.getSpawnAABB(...))`
        // before the roll; Steel has no AABB block-collision query, so that gate
        // is documented rather than faked.
        let mut random = LegacyRandom::from_seed(rand::random());
        if random.next_f32() >= CAMEL_HUSK_JOCKEY_CHANCE {
            return;
        }

        let Some(camel_husk) = ENTITIES.create(
            &vanilla_entities::CAMEL_HUSK,
            next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        ) else {
            return;
        };
        let Some(camel_mob) = camel_husk.as_mob() else {
            return;
        };

        // Vanilla sets the rider's spear before creating the mount.
        self.zombie.living_base().equipment().lock().set(
            EquipmentSlot::MainHand,
            ItemStack::new(&vanilla_items::IRON_SPEAR),
        );

        camel_mob.finalize_spawn(world, spawn_reason, None);
        self.start_riding(&camel_husk);
        if let Err(error) = world.try_add_entity(Arc::clone(&camel_husk)) {
            log::warn!("failed to add husk camel husk mount: {error}");
            return;
        }

        let camel_spawn_pos = camel_mob.position();
        let Some(parched) = ENTITIES.create(
            &vanilla_entities::PARCHED,
            next_entity_id(),
            camel_spawn_pos,
            Arc::downgrade(world),
        ) else {
            return;
        };
        let Some(parched_mob) = parched.as_mob() else {
            return;
        };
        parched_mob.snap_to(camel_spawn_pos, self.rotation().0, 0.0);
        parched_mob.finalize_spawn(world, spawn_reason, None);
        parched_mob.start_riding(&camel_husk);
        if let Err(error) = world.try_add_entity(Arc::clone(&parched)) {
            log::warn!("failed to add husk camel husk passenger: {error}");
        }
    }
}

impl Entity for HuskEntity {
    fn base(&self) -> &EntityBase {
        self.zombie.base()
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.zombie.entity_type()
    }

    fn base_tick(&self) {
        self.zombie.base_tick();
    }

    fn dimensions_for_pose(&self, pose: EntityPose) -> EntityDimensions {
        self.zombie.dimensions_for_pose(pose)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        // Vanilla `Husk` declares no synced data, so `HuskEntityData` matches the
        // `Zombie` layout the shared body writes to.
        self.zombie.synced_data()
    }

    fn update_data_before_sync(&self) {
        self.zombie.update_data_before_sync();
    }

    fn max_up_step(&self) -> f32 {
        self.zombie.max_up_step()
    }

    fn sound_source(&self) -> SoundSource {
        self.zombie.sound_source()
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        self.zombie.fall_sounds()
    }

    fn swim_sound(&self) -> SoundEventRef {
        self.zombie.swim_sound()
    }

    fn play_step_sound(&self, pos: BlockPos, block_state: BlockStateId) {
        self.zombie.play_step_sound(pos, block_state);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.zombie.save_additional(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.zombie.load_additional(nbt);
    }

    fn killed_entity(
        &self,
        world: &World,
        entity: &dyn LivingEntity,
        source: &DamageSource,
    ) -> bool {
        self.zombie.killed_entity(world, entity, source)
    }
}

impl LivingEntity for HuskEntity {
    fn living_base(&self) -> &LivingEntityBase {
        self.zombie.living_base()
    }

    fn get_health(&self) -> f32 {
        self.zombie.get_health()
    }

    fn set_health(&self, health: f32) {
        self.zombie.set_health(health);
    }

    fn hurt_sound(&self, source: &DamageSource) -> Option<SoundEventRef> {
        self.zombie.hurt_sound(source)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        self.zombie.death_sound()
    }

    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        self.zombie.on_about_to_be_hurt();
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn base_experience_reward(&self) -> i32 {
        self.zombie.base_experience_reward()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for HuskEntity {
    fn mob_base(&self) -> &MobBase {
        self.zombie.mob_base()
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        self.zombie.ambient_sound()
    }

    fn mob_flags(&self) -> i8 {
        self.zombie.mob_flags()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.zombie.set_mob_flags(flags);
    }

    fn can_hold_item(&self, item_stack: &ItemStack) -> bool {
        self.zombie.can_hold_item(item_stack)
    }

    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        self.zombie.wants_to_pick_up(item_stack)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        self.zombie.remove_when_far_away(dist_sqr)
    }

    fn after_hurt_target(&self, world: &World, target: &SharedEntity, was_hurt: bool) {
        Mob::after_hurt_target(&self.zombie, world, target, was_hurt);
        if was_hurt {
            self.apply_hunger(world, target);
        }
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.zombie.finalize_spawn(world, spawn_reason, group_data);
        self.try_spawn_camel_husk(world, spawn_reason);
        group_data
    }
}

impl Monster for HuskEntity {}

impl PathfinderMob for HuskEntity {}

/// Vanilla `Husk.HuskGroupData` only carries `ZombieGroupData`'s fields plus the
/// camel-husk attempt flag; Steel's [`SpawnGroupData`] has no zombie variant, so
/// the flag is unnecessary while the spawner finalizes one mob at a time.

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::HuskEntity;
    use crate::entity::Entity;

    #[test]
    fn husk_uses_zombie_conversion_target_and_its_own_baby_dimensions() {
        init_vanilla_registry();

        let husk = HuskEntity::new(&vanilla_entities::HUSK, 1, DVec3::ZERO, Weak::new());
        assert_eq!(husk.entity_type(), &vanilla_entities::HUSK);
        assert!(
            !husk
                .zombie()
                .converts_to_when_drowning()
                .key
                .path
                .is_empty()
        );
        assert_eq!(
            husk.zombie().converts_to_when_drowning().key,
            vanilla_entities::ZOMBIE.key,
            "vanilla `Husk.convertsToWhenDrowning` is `Zombie`"
        );

        husk.zombie().set_baby(true);
        let dimensions = husk.dimensions_for_pose(crate::entity::EntityPose::Standing);
        assert!((dimensions.eye_height - 0.825).abs() < f32::EPSILON);
    }
}
