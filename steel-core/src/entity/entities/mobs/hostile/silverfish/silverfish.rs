//! Vanilla `Silverfish` (`net.minecraft.world.entity.monster.Silverfish`).

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::SilverfishEntityData;
use steel_registry::{
    sound_events, vanilla_attributes, vanilla_damage_type_tags, vanilla_game_rules,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::types::{GameType, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::behavior::blocks::InfestedBlock;
use crate::entity::ai::goal::{
    ClimbOnTopOfPowderSnowGoal, FloatGoal, Goal, GoalControls, HurtByTargetGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, RandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, RemovalReason,
    SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 3;
/// Vanilla `SilverfishWakeUpFriendsGoal.tick` neighbor scan: ±5 vertical.
const WAKE_VERTICAL_RADIUS: i32 = 5;
/// Vanilla `SilverfishWakeUpFriendsGoal.tick` neighbor scan: ±10 horizontal.
const WAKE_HORIZONTAL_RADIUS: i32 = 10;
/// Vanilla `SilverfishWakeUpFriendsGoal.notifyHurt`: `adjustedTickDelay(20)`.
const WAKE_LOOK_FOR_FRIENDS_TICKS: i32 = 20;
/// Vanilla `SilverfishMergeWithStoneGoal` griefing roll: `reducedTickDelay(10)` → 5.
const MERGE_GRIEFING_ROLL_BOUND: i32 = 5;

/// Vanilla `Silverfish`.
#[entity_behavior(class = "Silverfish")]
pub struct SilverfishEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<SilverfishEntityData>,
    /// Vanilla `Silverfish.friendsGoal.lookForFriends`, shared between the entity (which
    /// notifies on hurt) and [`SilverfishWakeUpFriendsGoal`] (which counts down and scans).
    look_for_friends: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SilverfishEntity`.
unsafe impl DowncastType for SilverfishEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/silverfish");
}

impl SilverfishEntity {
    /// Creates a new silverfish at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a silverfish from persisted base entity state.
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
        let mut entity_data = SilverfishEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Silverfish.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, ClimbOnTopOfPowderSnowGoal::new());
            goal_selector.add_goal(3, SilverfishWakeUpFriendsGoal::new());
            goal_selector.add_goal(4, MeleeAttackGoal::new(1.0, false));
            goal_selector.add_goal(5, SilverfishMergeWithStoneGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new().set_alert_others(empty()));
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            look_for_friends: SyncMutex::new(0),
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

    /// Vanilla `SilverfishWakeUpFriendsGoal.notifyHurt`, reached from `hurtServer`.
    fn notify_wake_up_friends(&self) {
        let mut look_for_friends = self.look_for_friends.lock();
        if *look_for_friends == 0 {
            *look_for_friends = WAKE_LOOK_FOR_FRIENDS_TICKS;
        }
    }

    /// Vanilla `Silverfish.tick`: `yBodyRot = getYRot()`.
    fn sync_body_rotation(&self) {
        let (yaw, _pitch) = self.rotation();
        self.set_y_body_rot(yaw);
    }

    /// Vanilla `Silverfish.checkSilverfishSpawnRules`, the `SpawnPlacements` predicate
    /// for `EntityType.SILVERFISH`.
    ///
    /// Any-light monster rules apply first; spawner reasons bypass the player
    /// exclusion, which hides silverfish from players within five blocks.
    #[must_use]
    pub fn check_silverfish_spawn_rules(
        entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        if !<Self as Monster>::check_any_light_monster_spawn_rules(
            entity_type,
            level,
            spawn_reason,
            pos,
        ) {
            return false;
        }
        if spawn_reason.is_spawner() {
            return true;
        }
        level
            .nearest_player(
                DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                ),
                5.0,
                |player| !player.is_spectator() && player.game_mode() != GameType::Creative,
            )
            .is_none()
    }
}

/// Vanilla `Silverfish.SilverfishMergeWithStoneGoal extends RandomStrollGoal`.
struct SilverfishMergeWithStoneGoal {
    inner: RandomStrollGoal,
    selected_offset: Option<(i32, i32, i32)>,
    merge: bool,
}

impl SilverfishMergeWithStoneGoal {
    /// Vanilla `new SilverfishMergeWithStoneGoal(this)` → `super(mob, 1.0, 10)`.
    const fn new() -> Self {
        Self {
            inner: RandomStrollGoal::new(1.0),
            selected_offset: None,
            merge: false,
        }
    }
}

impl Goal for SilverfishMergeWithStoneGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if mob.target().is_some() {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };

        if world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING)
            && rand::random_range(0..MERGE_GRIEFING_ROLL_BOUND) == 0
        {
            let position = mob.position();
            let offset = random_direction_offset();
            let pos = BlockPos::new(
                steel_math::fast_floor(position.x),
                steel_math::fast_floor(position.y + 0.5),
                steel_math::fast_floor(position.z),
            )
            .offset(offset.0, offset.1, offset.2);
            if InfestedBlock::is_compatible_host_block(world.get_block_state(pos)) {
                self.merge = true;
                self.selected_offset = Some(offset);
                return true;
            }
        }

        self.merge = false;
        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.merge {
            return false;
        }
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if !self.merge {
            self.inner.start(mob);
            return;
        }

        let Some(world) = mob.level() else {
            return;
        };
        let Some((dx, dy, dz)) = self.selected_offset else {
            return;
        };
        let position = mob.position();
        let pos = BlockPos::new(
            steel_math::fast_floor(position.x),
            steel_math::fast_floor(position.y + 0.5),
            steel_math::fast_floor(position.z),
        )
        .offset(dx, dy, dz);

        let state = world.get_block_state(pos);
        if let Some(infested) = InfestedBlock::infested_state_by_host(state) {
            world.set_block(pos, infested, UpdateFlags::UPDATE_ALL);
            mob.broadcast_entity_event(EntityStatus::SilverfishMergeAnim);
            mob.set_removed(RemovalReason::Discarded);
        }
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if !self.merge {
            self.inner.stop(mob);
        }
        self.merge = false;
        self.selected_offset = None;
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        if !self.merge {
            self.inner.tick(mob, entity);
        }
    }
}

/// Vanilla `Silverfish.SilverfishWakeUpFriendsGoal`.
struct SilverfishWakeUpFriendsGoal;

impl SilverfishWakeUpFriendsGoal {
    /// Vanilla `new SilverfishWakeUpFriendsGoal(this)`; the counter lives on the entity.
    const fn new() -> Self {
        Self
    }

    fn look_for_friends(mob: &dyn PathfinderMob) -> Option<&SilverfishEntity> {
        mob.as_entity_event_source()
            .downcast_ref::<SilverfishEntity>()
    }
}

impl Goal for SilverfishWakeUpFriendsGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        Self::look_for_friends(mob)
            .is_some_and(|silverfish| *silverfish.look_for_friends.lock() > 0)
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(silverfish) = Self::look_for_friends(mob) else {
            return;
        };

        {
            let mut look_for_friends = silverfish.look_for_friends.lock();
            *look_for_friends -= 1;
            if *look_for_friends > 0 {
                return;
            }
        }

        let Some(world) = mob.level() else {
            return;
        };
        let base_pos = mob.block_position();
        for y_off in (-WAKE_VERTICAL_RADIUS)..=WAKE_VERTICAL_RADIUS {
            for x_off in (-WAKE_HORIZONTAL_RADIUS)..=WAKE_HORIZONTAL_RADIUS {
                for z_off in (-WAKE_HORIZONTAL_RADIUS)..=WAKE_HORIZONTAL_RADIUS {
                    let test_pos = base_pos.offset(x_off, y_off, z_off);
                    let state = world.get_block_state(test_pos);
                    if !InfestedBlock::is_infested_block_state(state) {
                        continue;
                    }

                    if world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
                        world.destroy_block(test_pos, true);
                    } else if let Some(host) = InfestedBlock::host_state_by_infested(state) {
                        world.set_block(test_pos, host, UpdateFlags::UPDATE_ALL);
                    }

                    if rand::random::<bool>() {
                        return;
                    }
                }
            }
        }
    }
}

/// Vanilla `Direction.getRandom(random)` for the merge goal's neighbor pick.
fn random_direction_offset() -> (i32, i32, i32) {
    use steel_utils::Direction;

    let direction = match rand::random_range(0..6) {
        0 => Direction::Down,
        1 => Direction::Up,
        2 => Direction::North,
        3 => Direction::South,
        4 => Direction::West,
        _ => Direction::East,
    };
    direction.offset()
}

impl Entity for SilverfishEntity {
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
        // Vanilla `Silverfish.tick` forces the body rotation to the head rotation first.
        self.sync_body_rotation();
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
        self.play_sound(&sound_events::ENTITY_SILVERFISH_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for SilverfishEntity {
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
        Some(&sound_events::ENTITY_SILVERFISH_HURT)
    }

    fn before_actually_hurt(&self, source: &DamageSource, _amount: f32) {
        // Vanilla `Silverfish.hurtServer` arms `SilverfishWakeUpFriendsGoal` before
        // calling `super.hurtServer`: any attacker, or a damage type in
        // `#minecraft:always_triggers_silverfish`. Steel's `hurtServer` body is not
        // callable from an override, so the wake-up happens in this hook instead —
        // same tick, after vanilla's invulnerability/reduction checks.
        if source.causing_entity().is_some()
            || source.is(&vanilla_damage_type_tags::DamageTypeTag::ALWAYS_TRIGGERS_SILVERFISH)
        {
            self.notify_wake_up_friends();
        }
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SILVERFISH_DEATH)
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

impl Mob for SilverfishEntity {
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
        Some(&sound_events::ENTITY_SILVERFISH_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn ambient_sound_interval(&self) -> i32 {
        120
    }
}

impl Monster for SilverfishEntity {}

impl PathfinderMob for SilverfishEntity {
    /// Vanilla `Silverfish.getWalkTargetValue` favors infested-compatible host blocks below.
    fn get_walk_target_value(&self, pos: BlockPos) -> f32 {
        if let Some(world) = self.level()
            && InfestedBlock::is_compatible_host_block(world.get_block_state(pos.offset(0, -1, 0)))
        {
            return 10.0;
        }
        Monster::get_walk_target_value_monster(self, pos)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::SilverfishEntity;
    use crate::entity::damage::DamageSource;
    use crate::entity::entities::mobs::hostile::enderman::EndermanEntity;
    use crate::entity::{Entity, LivingEntity, SharedEntity};
    use steel_registry::vanilla_damage_types;

    #[test]
    fn silverfish_notifies_friends_once_per_quiet_period() {
        init_vanilla_registry();
        let silverfish =
            SilverfishEntity::new(&vanilla_entities::SILVERFISH, 1, DVec3::ZERO, Weak::new());

        assert_eq!(*silverfish.look_for_friends.lock(), 0);
        silverfish.notify_wake_up_friends();
        assert_eq!(*silverfish.look_for_friends.lock(), 20);
        *silverfish.look_for_friends.lock() = 5;
        silverfish.notify_wake_up_friends();
        assert_eq!(
            *silverfish.look_for_friends.lock(),
            5,
            "vanilla only arms the search when the counter is zero"
        );
        assert_eq!(silverfish.entity_type(), &vanilla_entities::SILVERFISH);
    }

    #[test]
    fn hurt_arms_the_wake_up_search_like_vanilla() {
        init_vanilla_registry();
        let silverfish =
            SilverfishEntity::new(&vanilla_entities::SILVERFISH, 1, DVec3::ZERO, Weak::new());

        // An attacker's damage arms the search (`source.getEntity() != null`).
        let attacker: SharedEntity = Arc::new(EndermanEntity::new(
            &vanilla_entities::ENDERMAN,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));
        let attacker_source = DamageSource::direct(
            &vanilla_damage_types::MOB_ATTACK,
            SharedEntity::clone(&attacker),
        );
        silverfish.before_actually_hurt(&attacker_source, 1.0);
        assert_eq!(*silverfish.look_for_friends.lock(), 20);

        // `#minecraft:always_triggers_silverfish` arms it without an attacker.
        *silverfish.look_for_friends.lock() = 0;
        let magic_source = DamageSource::environment(&vanilla_damage_types::MAGIC);
        silverfish.before_actually_hurt(&magic_source, 1.0);
        assert_eq!(*silverfish.look_for_friends.lock(), 20);

        // Neither an entity nor the tag: vanilla leaves the counter alone.
        *silverfish.look_for_friends.lock() = 0;
        let generic_source = DamageSource::environment(&vanilla_damage_types::GENERIC);
        silverfish.before_actually_hurt(&generic_source, 1.0);
        assert_eq!(*silverfish.look_for_friends.lock(), 0);
    }
}
