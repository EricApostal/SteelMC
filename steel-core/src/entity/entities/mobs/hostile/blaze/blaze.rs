//! Vanilla `Blaze` (`net.minecraft.world.entity.monster.Blaze`).

use std::iter::empty;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::BlazeEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, MoveTowardsRestrictionGoal,
    NearestAttackableTargetGoal, RandomLookAroundGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::SmallFireballEntity;
use crate::entity::projectile::spawn_projectile_using_shoot;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity, next_entity_id,
};
use crate::physics::MoveResult;
use crate::world::World;
use steel_utils::Downcast as _;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 10;
/// Vanilla `Blaze.DATA_FLAGS_ID` charged bit.
const FLAG_CHARGED: i8 = 1;
/// Vanilla `Blaze.customServerAiStep` height-offset refresh interval.
const HEIGHT_OFFSET_REFRESH_TICKS: i32 = 100;
/// Vanilla `Blaze.customServerAiStep` triangle sampler.
const HEIGHT_OFFSET_TRIANGLE_MEAN: f64 = 0.5;
const HEIGHT_OFFSET_TRIANGLE_DEVIATION: f64 = 6.891;
/// Vanilla `Blaze.customServerAiStep` climb acceleration.
const CLIMB_ACCELERATION: f64 = 0.3;
/// Vanilla `BlazeAttackGoal` follow distance for the ranged phase.
const ATTACK_FOLLOW_DISTANCE: f64 = 48.0;
/// Vanilla `BlazeAttackGoal` melee range (squared).
const MELEE_RANGE_SQR: f64 = 4.0;
/// Vanilla `BlazeAttackGoal` fireball uncertainty scale.
const FIREBALL_UNCERTAINTY_SCALE: f64 = 2.297;
/// Vanilla `LevelEvent.BLAZE_SHOOT`.
const BLAZE_SHOOT_LEVEL_EVENT: i32 = 1018;

/// Vanilla `Blaze`.
#[entity_behavior(class = "Blaze")]
pub struct BlazeEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<BlazeEntityData>,
    /// Vanilla `Blaze.allowedHeightOffset`.
    allowed_height_offset: SyncMutex<f32>,
    /// Vanilla `Blaze.nextHeightOffsetChangeTick`.
    next_height_offset_change_tick: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BlazeEntity`.
unsafe impl DowncastType for BlazeEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/blaze");
}

impl BlazeEntity {
    /// Creates a new blaze entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved blaze entity.
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
        let mut entity_data = BlazeEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.flags.set(0);

        // Vanilla `Blaze` pathfinding maluses: water is impassable, lava is cheap.
        mob_base
            .pathfinding_malus()
            .lock()
            .set(PathType::Water, -1.0);
        mob_base.pathfinding_malus().lock().set(PathType::Lava, 8.0);

        {
            // Vanilla `Blaze.registerGoals`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(4, BlazeAttackGoal::new());
            goal_selector.add_goal(5, MoveTowardsRestrictionGoal::new(1.0));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());

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
            allowed_height_offset: SyncMutex::new(0.5),
            next_height_offset_change_tick: SyncMutex::new(0),
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

    /// Vanilla `Blaze.isCharged`.
    #[must_use]
    pub fn is_charged(&self) -> bool {
        self.entity_data.lock().flags.get() & FLAG_CHARGED != 0
    }

    /// Vanilla `Blaze.setCharged`.
    pub fn set_charged(&self, charged: bool) {
        let mut entity_data = self.entity_data.lock();
        let flags = *entity_data.flags.get();
        entity_data.flags.set(if charged {
            flags | FLAG_CHARGED
        } else {
            flags & !FLAG_CHARGED
        });
    }

    /// Vanilla `Blaze.customServerAiStep` server-side body.
    fn custom_step_blaze(&self, entity: &SharedEntity) {
        {
            let mut tick = self.next_height_offset_change_tick.lock();
            *tick -= 1;
            if *tick <= 0 {
                *tick = HEIGHT_OFFSET_REFRESH_TICKS;
                *self.allowed_height_offset.lock() = triangle_sample(
                    HEIGHT_OFFSET_TRIANGLE_MEAN,
                    HEIGHT_OFFSET_TRIANGLE_DEVIATION,
                ) as f32;
            }
        }

        if let Some(target) = self.target() {
            let target_eye = target.get_eye_y();
            if target_eye > self.get_eye_y() + f64::from(*self.allowed_height_offset.lock())
                && target
                    .as_living_entity()
                    .is_some_and(|living| Mob::can_attack(self, living))
            {
                // Vanilla `Blaze`: rise toward a target above its allowed height offset.
                let movement = self.velocity();
                self.set_velocity(DVec3::new(
                    movement.x,
                    movement.y + (CLIMB_ACCELERATION - movement.y) * CLIMB_ACCELERATION,
                    movement.z,
                ));
            }
        }

        Monster::monster_ai_step(self, entity);
    }
}

/// Vanilla `RandomSource.triangle(mean, deviation)` sampled from a uniform source.
fn triangle_sample(mean: f64, deviation: f64) -> f64 {
    // Vanilla `RandomSource.triangle` averages two uniform samples scaled to the deviation.
    let sample = (rand::random::<f64>() - rand::random::<f64>()) * deviation;
    mean + sample
}

/// Vanilla `Blaze.BlazeAttackGoal`.
struct BlazeAttackGoal {
    attack_step: i32,
    attack_time: i32,
    last_seen: i32,
}

impl BlazeAttackGoal {
    const fn new() -> Self {
        Self {
            attack_step: 0,
            attack_time: 0,
            last_seen: 0,
        }
    }

    fn blaze(mob: &dyn PathfinderMob) -> Option<&BlazeEntity> {
        mob.as_entity_event_source().downcast_ref::<BlazeEntity>()
    }

    /// Vanilla `Blaze.BlazeAttackGoal` fireball volley.
    fn shoot_fireballs(&self, blaze: &BlazeEntity, target: &SharedEntity, world: &Arc<World>) {
        let distance = blaze.position().distance_squared(target.position());
        let sqd = distance.sqrt().sqrt() * 0.5;
        let uncertainty = FIREBALL_UNCERTAINTY_SCALE * sqd;

        if !blaze.is_silent() {
            world.level_event(BLAZE_SHOOT_LEVEL_EVENT, blaze.block_position(), 0, None);
        }

        let origin = DVec3::new(
            blaze.position().x,
            blaze.get_eye_y() - 0.5,
            blaze.position().z,
        );
        let direction = DVec3::new(
            triangle_sample(target.position().x - blaze.position().x, uncertainty),
            triangle_sample(
                target.position().y + 0.5 - (blaze.position().y + 0.5),
                uncertainty,
            ),
            triangle_sample(target.position().z - blaze.position().z, uncertainty),
        );
        let Some(normalized) = direction.try_normalize() else {
            return;
        };

        let projectile = SmallFireballEntity::with_direction(
            &vanilla_entities::SMALL_FIREBALL,
            next_entity_id(),
            origin,
            normalized,
            Arc::downgrade(world),
        );
        let mut item_stack = ItemStack::empty();
        let _ =
            spawn_projectile_using_shoot(world, projectile, &mut item_stack, direction, 1.0, 0.0);
    }
}

impl Goal for BlazeAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let Some(target_living) = target.as_living_entity() else {
            return false;
        };
        LivingEntity::is_alive(target_living) && Mob::can_attack(mob, target_living)
    }

    fn start(&mut self, _mob: &dyn PathfinderMob) {
        self.attack_step = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(blaze) = Self::blaze(mob) {
            blaze.set_charged(false);
        }
        self.last_seen = 0;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(blaze) = Self::blaze(mob) else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };
        let Some(world) = mob.level() else {
            return;
        };

        self.attack_time -= 1;
        let has_line_of_sight = mob.has_line_of_sight_cached(target.as_ref());
        if has_line_of_sight {
            self.last_seen = 0;
        } else {
            self.last_seen += 1;
        }

        let distance = mob.position().distance_squared(target.position());
        if distance < MELEE_RANGE_SQR {
            if !has_line_of_sight {
                return;
            }

            if self.attack_time <= 0 {
                self.attack_time = 20;
                Mob::do_hurt_target(blaze, entity, &world, &target);
            }
            mob.mob_base()
                .controls()
                .lock()
                .move_control
                .set_wanted_position(target.position(), 1.0);
        } else if distance < ATTACK_FOLLOW_DISTANCE * ATTACK_FOLLOW_DISTANCE && has_line_of_sight {
            if self.attack_time <= 0 {
                self.attack_step += 1;
                if self.attack_step == 1 {
                    self.attack_time = 60;
                    blaze.set_charged(true);
                } else if self.attack_step <= 4 {
                    self.attack_time = 6;
                } else {
                    self.attack_time = 100;
                    self.attack_step = 0;
                    blaze.set_charged(false);
                }

                if self.attack_step > 1 {
                    self.shoot_fireballs(blaze, &target, &world);
                }
            }
        } else if self.last_seen < 5 {
            mob.mob_base()
                .controls()
                .lock()
                .move_control
                .set_wanted_position(target.position(), 1.0);
        }
    }
}

impl Entity for BlazeEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    /// Vanilla `Blaze.isOnFire` mirrors the charge flag.
    fn is_on_fire(&self) -> bool {
        self.is_charged()
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
    }
}

impl LivingEntity for BlazeEntity {
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
        Some(&sound_events::ENTITY_BLAZE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_BLAZE_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        // Vanilla `Blaze.customServerAiStep` then the shared mob step.
        self.custom_step_blaze(entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Monster::monster_ai_step(self, entity)
    }
}

impl Mob for BlazeEntity {
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
        Some(&sound_events::ENTITY_BLAZE_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl Monster for BlazeEntity {}

impl PathfinderMob for BlazeEntity {}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use super::BlazeEntity;
    use crate::entity::Entity;

    #[test]
    fn blaze_charge_flag_round_trips() {
        init_vanilla_registry();
        let blaze = BlazeEntity::new(&vanilla_entities::BLAZE, 1, DVec3::ZERO, Weak::new());

        assert!(!blaze.is_charged());
        assert!(!blaze.is_on_fire());
        blaze.set_charged(true);
        assert!(blaze.is_charged());
        assert!(blaze.is_on_fire());
        blaze.set_charged(false);
        assert!(!blaze.is_charged());
        assert_eq!(blaze.entity_type(), &vanilla_entities::BLAZE);
    }
}
