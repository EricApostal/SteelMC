//! Vanilla `Vex` (vanilla `net.minecraft.world.entity.monster.Vex`).
//!
//! A vex is a flying monster summoned by an evoker: it charges at its summoner's
//! target, is leashed to a bound origin while idle and can be given a limited
//! lifetime after which it starves to death. Steel has no per-entity
//! `MoveControl` subclass, so `Vex.VexMoveControl` is reproduced by the private
//! goals writing a wanted position onto the entity and [`VexEntity::ai_step`]
//! applying the same delta-movement integration each tick.

use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::VexEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_damage_types, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Downcast as _, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    FloatGoal, Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::illager::RaiderKeys;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 3;
/// Vanilla `Vex.VexMoveControl`: the "close enough" threshold is the bounding
/// box's largest dimension (vanilla `AABB.getSize`), 0.8 for the vex hitbox.
const VEX_MOVE_ARRIVAL_EPSILON: f64 = 0.8;
/// Vanilla `Vex.VexChargeAttackGoal` movement speed modifier.
const CHARGE_SPEED_MODIFIER: f64 = 1.0;
/// Vanilla `Vex.VexRandomMoveGoal` movement speed modifier.
const RANDOM_MOVE_SPEED_MODIFIER: f64 = 0.25;
/// Vanilla `Mob.getRandom().nextInt(reducedTickDelay(7))` gate.
const fn reduced_tick_delay(ticks: i32) -> i32 {
    (ticks + 1) / 2
}

/// Vanilla vex entity.
#[entity_behavior(class = "Vex")]
pub struct VexEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<VexEntityData>,
    /// Vanilla `Vex.owner`: the summoning evoker, persisted as an entity reference.
    owner: SyncMutex<Option<crate::entity::EntityReference>>,
    /// Vanilla `Vex.boundOrigin`.
    bound_origin: SyncMutex<Option<BlockPos>>,
    /// Vanilla `Vex.hasLimitedLife`.
    has_limited_life: SyncMutex<bool>,
    /// Vanilla `Vex.limitedLifeTicks`.
    limited_life_ticks: SyncMutex<i32>,
    /// Emulated vanilla `Vex.VexMoveControl` wanted position and speed modifier.
    move_wanted: SyncMutex<Option<(DVec3, f64)>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `VexEntity`.
unsafe impl DowncastType for VexEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/vex");
}

impl VexEntity {
    /// Creates a new vex entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved vex entity.
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
        let mut entity_data = VexEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            goals.add_goal(4, VexChargeAttackGoal);
            goals.add_goal(8, VexRandomMoveGoal);
            goals.add_goal(9, LookAtPlayerGoal::new_with_probability(3.0, 1.0));
            goals.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(8.0, 0.02, |_, _| true),
            );

            let mut targets = mob_base.target_selector().lock();
            targets.add_goal(
                1,
                HurtByTargetGoal::new()
                    .with_ignored_damage_types(RaiderKeys::ALL)
                    .set_alert_others(std::iter::empty()),
            );
            targets.add_goal(2, VexCopyOwnerTargetGoal::new());
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
        }

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            owner: SyncMutex::new(None),
            bound_origin: SyncMutex::new(None),
            has_limited_life: SyncMutex::new(false),
            limited_life_ticks: SyncMutex::new(0),
            move_wanted: SyncMutex::new(None),
        };

        // Vanilla `Vex.populateDefaultEquipmentSlots`: an iron sword that never
        // drops. Loading overwrites this from the saved equipment afterwards.
        entity.set_item_in_hand(
            steel_utils::types::InteractionHand::MainHand,
            ItemStack::new(&vanilla_items::IRON_SWORD),
        );
        entity.set_equipment_drop_chance(EquipmentSlot::MainHand, 0.0);
        entity
    }

    /// Vanilla `Vex.setOwner`.
    pub fn set_owner(&self, owner: Option<&SharedEntity>) {
        *self.owner.lock() = owner.map(crate::entity::EntityReference::from_entity);
    }

    /// Resolves vanilla `Vex.getOwner`.
    #[must_use]
    pub fn owner(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        self.owner.lock().as_ref()?.get_entity(&world)
    }

    /// Vanilla `Vex.getBoundOrigin`.
    #[must_use]
    pub fn bound_origin(&self) -> Option<BlockPos> {
        *self.bound_origin.lock()
    }

    /// Vanilla `Vex.setBoundOrigin`.
    pub fn set_bound_origin(&self, origin: Option<BlockPos>) {
        *self.bound_origin.lock() = origin;
    }

    /// Vanilla `Vex.setLimitedLife`.
    pub fn set_limited_life(&self, life_ticks: i32) {
        *self.has_limited_life.lock() = true;
        *self.limited_life_ticks.lock() = life_ticks;
    }

    /// Vanilla `Vex.isCharging`.
    #[must_use]
    pub fn is_charging(&self) -> bool {
        (self.entity_data.lock().flags.get() & 1) != 0
    }

    /// Vanilla `Vex.setIsCharging`.
    pub fn set_is_charging(&self, charging: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.flags.get();
        let flags = if charging { flags | 1 } else { flags & !1 };
        entity_data.flags.set(flags);
    }

    /// Emulated `Vex.VexMoveControl.hasWanted`.
    fn has_wanted_move(&self) -> bool {
        self.move_wanted.lock().is_some()
    }

    /// Emulated `Vex.VexMoveControl.setWantedPosition`.
    fn set_wanted_position(&self, position: DVec3, speed_modifier: f64) {
        *self.move_wanted.lock() = Some((position, speed_modifier));
    }

    /// Emulated `Vex.VexMoveControl.tick`: integrate velocity toward the wanted
    /// position. Returns `true` once the vex arrives and the move is cleared.
    fn tick_move_control(&self, entity: &SharedEntity) -> bool {
        let Some((wanted, speed_modifier)) = *self.move_wanted.lock() else {
            return false;
        };

        let delta = wanted - entity.position();
        let delta_length = delta.length();
        if delta_length < VEX_MOVE_ARRIVAL_EPSILON {
            entity.set_velocity(entity.velocity() * 0.5);
            *self.move_wanted.lock() = None;
            return true;
        }

        entity.set_velocity(entity.velocity() + delta * (speed_modifier * 0.05 / delta_length));

        // Vanilla faces the movement when idle and the target when charging.
        let look_delta = self
            .mob_base()
            .target(|_| true)
            .map_or(delta, |target| target.position() - entity.position());
        let yaw = -(look_delta.x.atan2(look_delta.z) as f32).to_degrees();
        entity.set_rotation((yaw, entity.rotation().1));
        false
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.vex_mut().mob_mut().living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }
}

impl Entity for VexEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        // Vanilla `Vex.tick`: no physics while ticking, then no gravity.
        self.set_no_physics(true);
        Mob::base_tick_mob(self);
        self.set_no_physics(false);
        self.set_no_gravity(true);

        // Vanilla `Vex.tick` limited-life starvation.
        if *self.has_limited_life.lock() {
            let starved = {
                let mut ticks = self.limited_life_ticks.lock();
                *ticks -= 1;
                if *ticks <= 0 {
                    *ticks = 20;
                    true
                } else {
                    false
                }
            };
            if starved && let Some(world) = self.level() {
                self.hurt(
                    &world,
                    &DamageSource::environment(&vanilla_damage_types::STARVE),
                    1.0,
                );
            }
        }
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        if let Some(origin) = *self.bound_origin.lock() {
            // Vanilla `Vex.addAdditionalSaveData`: `bound_pos` as a `BlockPos`
            // int-array codec.
            nbt.insert(
                "bound_pos",
                NbtTag::IntArray(vec![origin.x(), origin.y(), origin.z()]),
            );
        }
        if *self.has_limited_life.lock() {
            nbt.insert("life_ticks", *self.limited_life_ticks.lock());
        }
        if let Some(owner) = self.owner.lock().as_ref() {
            owner.store(nbt, "owner");
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        *self.bound_origin.lock() =
            nbt.int_array("bound_pos")
                .and_then(|array| match array.as_slice() {
                    [x, y, z] => Some(BlockPos::new(*x, *y, *z)),
                    _ => None,
                });
        if let Some(life_ticks) = nbt.int("life_ticks") {
            self.set_limited_life(life_ticks);
        } else {
            *self.has_limited_life.lock() = false;
        }
        *self.owner.lock() = crate::entity::EntityReference::read(&nbt, "owner");
    }
}

impl LivingEntity for VexEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self
            .entity_data
            .lock()
            .vex()
            .mob()
            .living_entity()
            .health
            .get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .vex_mut()
            .mob_mut()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VEX_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VEX_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Monster::monster_ai_step(self, entity);
        self.tick_move_control(entity);
        result
    }
}

impl Mob for VexEntity {
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
        Some(&sound_events::ENTITY_VEX_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().vex().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .vex_mut()
            .mob_mut()
            .mob_flags
            .set(flags);
    }
}

impl Monster for VexEntity {}

impl PathfinderMob for VexEntity {}

/// Vanilla `Vex.VexChargeAttackGoal`.
struct VexChargeAttackGoal;

impl Goal for VexChargeAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return false;
        };
        let Some(target) = mob.target() else {
            return false;
        };
        target.is_alive()
            && !vex.has_wanted_move()
            && rand::random_range(0..reduced_tick_delay(7)) == 0
            && mob.distance_to_sqr(target.position()) > 4.0
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return false;
        };
        vex.has_wanted_move()
            && vex.is_charging()
            && mob.target().is_some_and(|target| target.is_alive())
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return;
        };
        if let Some(target) = mob.target() {
            // Vanilla targets the eye, which sits at `bounding_box.maxY + 0.5`.
            let halves = target.bounding_box().size() / 2.0;
            let eye = DVec3::new(
                target.position().x,
                target.position().y + halves + 0.5,
                target.position().z,
            );
            vex.set_wanted_position(eye, CHARGE_SPEED_MODIFIER);
        }
        vex.set_is_charging(true);
        mob.play_sound(&sound_events::ENTITY_VEX_CHARGE, 1.0, 1.0);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(vex) = mob.downcast_ref::<VexEntity>() {
            vex.set_is_charging(false);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };
        let Some(living_target) = target.as_living_entity() else {
            return;
        };
        if mob.is_within_melee_attack_range(living_target) {
            if let Some(world) = mob.level() {
                mob.do_hurt_target(entity, &world, &target);
            }
            vex.set_is_charging(false);
        } else if mob.distance_to_sqr(target.position()) < 9.0 {
            let halves = target.bounding_box().size() / 2.0;
            let eye = DVec3::new(
                target.position().x,
                target.position().y + halves + 0.5,
                target.position().z,
            );
            vex.set_wanted_position(eye, CHARGE_SPEED_MODIFIER);
        }
    }
}

/// Vanilla `Vex.VexCopyOwnerTargetGoal`.
struct VexCopyOwnerTargetGoal {
    target: Option<SharedEntity>,
}

impl VexCopyOwnerTargetGoal {
    const fn new() -> Self {
        Self { target: None }
    }
}

impl Goal for VexCopyOwnerTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.target = None;
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return false;
        };
        let Some(owner) = vex.owner() else {
            return false;
        };
        let Some(target) = owner.as_mob().and_then(|owner| owner.target()) else {
            return false;
        };
        if !target
            .as_living_entity()
            .is_some_and(|target| crate::entity::Mob::can_attack(mob, target))
        {
            return false;
        }
        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.target().is_some()
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(self.target.as_ref());
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        let _ = mob.set_target(None);
        self.target = None;
    }
}

/// Vanilla `Vex.VexRandomMoveGoal`.
struct VexRandomMoveGoal;

impl Goal for VexRandomMoveGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        mob.downcast_ref::<VexEntity>()
            .is_some_and(|vex| !vex.has_wanted_move())
            && rand::random_range(0..reduced_tick_delay(7)) == 0
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(vex) = mob.downcast_ref::<VexEntity>() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };
        let origin = vex.bound_origin().unwrap_or_else(|| mob.block_position());

        // Vanilla tries three random positions around the bound origin and
        // takes the first that is an empty block.
        for _ in 0..3 {
            let test_pos = BlockPos::new(
                origin.x() + rand::random_range(-7..=7),
                origin.y() + rand::random_range(-5..=5),
                origin.z() + rand::random_range(-7..=7),
            );
            if world.get_block_state(test_pos).is_air() {
                let position = DVec3::new(
                    f64::from(test_pos.x()) + 0.5,
                    f64::from(test_pos.y()) + 0.5,
                    f64::from(test_pos.z()) + 0.5,
                );
                vex.set_wanted_position(position, RANDOM_MOVE_SPEED_MODIFIER);
                if mob.target().is_none() {
                    mob.mob_base()
                        .controls()
                        .lock()
                        .look_control
                        .set_look_at(position, 180.0, 20.0);
                }
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::BlockPos;

    use super::*;

    fn test_vex() -> VexEntity {
        init_vanilla_registry();
        VexEntity::new(&vanilla_entities::VEX, 1, DVec3::ZERO, Weak::<World>::new())
    }

    #[test]
    fn charging_flag_round_trips() {
        let vex = test_vex();
        assert!(!vex.is_charging());
        vex.set_is_charging(true);
        assert!(vex.is_charging());
        vex.set_is_charging(false);
        assert!(!vex.is_charging());
    }

    #[test]
    fn bound_origin_and_limited_life_are_stored() {
        let vex = test_vex();
        assert_eq!(vex.bound_origin(), None);
        vex.set_bound_origin(Some(BlockPos::new(1, 2, 3)));
        assert_eq!(vex.bound_origin(), Some(BlockPos::new(1, 2, 3)));

        vex.set_limited_life(100);
        assert!(*vex.has_limited_life.lock());
        assert_eq!(*vex.limited_life_ticks.lock(), 100);
    }

    #[test]
    fn move_control_clears_the_wanted_position_on_arrival() {
        let vex = test_vex();
        vex.set_wanted_position(DVec3::new(0.0, 0.0, 0.0), 1.0);
        assert!(vex.has_wanted_move());
    }

    #[test]
    fn charges_at_a_nearby_target() {
        let vex = test_vex();
        vex.set_wanted_position(DVec3::new(1.0, 0.0, 0.0), 1.0);
        vex.set_is_charging(true);
        assert!(vex.has_wanted_move() && vex.is_charging());
    }
}
