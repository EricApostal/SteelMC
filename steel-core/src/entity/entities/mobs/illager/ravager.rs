//! Vanilla `Ravager` (vanilla `net.minecraft.world.entity.monster.Ravager`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_math::fast_floor;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::particle_type::{ColorParticleOption, ParticleData};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::RavagerEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_damage_types,
    vanilla_entities, vanilla_game_events, vanilla_particle_types,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{ArgbColor, BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use super::abstract_illager::RaiderKeys;
use crate::entity::ai::goal::{
    FloatGoal, GoalControl, HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal,
    NearestAttackableTargetGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::path::PathType;
use crate::entity::damage::DamageSource;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 20;

/// Vanilla `Ravager.ATTACK_DURATION`.
const ATTACK_DURATION: i32 = 10;
/// Vanilla `Ravager.STUN_DURATION`.
const STUN_DURATION: i32 = 40;
/// Vanilla `Ravager.BASE_MOVEMENT_SPEED`.
const BASE_MOVEMENT_SPEED: f64 = 0.3;
/// Vanilla `Ravager.ATTACK_MOVEMENT_SPEED`.
const ATTACK_MOVEMENT_SPEED: f64 = 0.35;
/// Vanilla `Ravager`'s movement-speed base-value lerp delta.
const MOVEMENT_SPEED_LERP: f64 = 0.1;
/// Vanilla `Ravager.ROAR_RADIUS`.
const ROAR_RADIUS: f64 = 4.0;
/// Vanilla `Ravager` roar damage.
const ROAR_DAMAGE: f32 = 6.0;
/// Vanilla `ColorParticleOption.create(ParticleTypes.ENTITY_EFFECT, 0.49803922F, 0.5137255F,
/// 0.57254905F)`, packed as opaque ARGB (`0xFF7F8392`).
const STUNNED_PARTICLE_COLOR: i32 = 0xFF7F_8392_u32 as i32;
/// Vanilla `Ravager.strongKnockback` horizontal strength.
const ROAR_KNOCKBACK_STRENGTH: f64 = 4.0;
/// Vanilla `Ravager.strongKnockback` vertical strength.
const ROAR_KNOCKBACK_Y: f64 = 0.2;

/// Vanilla ravager entity.
#[entity_behavior(class = "Ravager")]
pub struct RavagerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<RavagerEntityData>,
    attack_tick: SyncMutex<i32>,
    stunned_tick: SyncMutex<i32>,
    roar_tick: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `RavagerEntity`.
unsafe impl DowncastType for RavagerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/ravager");
}

impl RavagerEntity {
    /// Creates a new ravager entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved ravager entity.
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
        let mut entity_data = RavagerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            goals.add_goal(4, MeleeAttackGoal::new(1.0, true));
            goals.add_goal(5, WaterAvoidingRandomStrollGoal::new(0.4));
            goals.add_goal(6, LookAtPlayerGoal::new(6.0));
            goals.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(8.0, 1.0, |_, _| true),
            );

            let mut targets = mob_base.target_selector().lock();
            // Vanilla `HurtByTargetGoal(this, Raider.class).setAlertOthers()`: ignore damage
            // from every raider type and alert the rest of the raiders.
            targets.add_goal(
                2,
                HurtByTargetGoal::new()
                    .with_ignored_damage_types(RaiderKeys::ALL)
                    .set_alert_others(std::iter::empty()),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
            // Vanilla `NearestAttackableTargetGoal<>(this, AbstractVillager.class, true,
            // (target, level) -> !target.isBaby())`.
            targets.add_goal(
                4,
                NearestAttackableTargetGoal::new(true, |target, world| {
                    super::abstract_illager::is_target_villager(target, world)
                        && super::abstract_illager::can_attack_villager(target, world)
                }),
            );
            targets.add_goal(
                4,
                NearestAttackableTargetGoal::new(
                    true,
                    super::abstract_illager::is_target_iron_golem,
                ),
            );
        }

        super::raider_goals::register_raider_goals(&mob_base);

        let ravager = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            raider_base: RaiderBase::new(),
            entity_data: SyncMutex::new(entity_data),
            attack_tick: SyncMutex::new(0),
            stunned_tick: SyncMutex::new(0),
            roar_tick: SyncMutex::new(0),
        };
        // Vanilla `Ravager` constructor: `setPathfindingMalus(PathType.LEAVES, 0.0F)`.
        ravager.set_pathfinding_malus(PathType::Leaves, 0.0);
        ravager
    }

    /// Vanilla `Ravager.getAttackTick`.
    #[must_use]
    pub fn attack_tick(&self) -> i32 {
        *self.attack_tick.lock()
    }

    fn set_attack_tick(&self, tick: i32) {
        *self.attack_tick.lock() = tick;
    }

    /// Vanilla `Ravager.getStunnedTick`.
    #[must_use]
    pub fn stunned_tick(&self) -> i32 {
        *self.stunned_tick.lock()
    }

    fn set_stunned_tick(&self, tick: i32) {
        *self.stunned_tick.lock() = tick;
    }

    /// Vanilla `Ravager.getRoarTick`.
    #[must_use]
    pub fn roar_tick(&self) -> i32 {
        *self.roar_tick.lock()
    }

    fn set_roar_tick(&self, tick: i32) {
        *self.roar_tick.lock() = tick;
    }

    /// Vanilla `Ravager.strongKnockback`: shove `entity` away from this ravager.
    fn strong_knockback(&self, entity: &dyn Entity) {
        let dx = entity.position().x - self.position().x;
        let dz = entity.position().z - self.position().z;
        let dd = dx.mul_add(dx, dz * dz).max(0.001);
        entity.push_impulse(DVec3::new(
            dx / dd * ROAR_KNOCKBACK_STRENGTH,
            ROAR_KNOCKBACK_Y,
            dz / dd * ROAR_KNOCKBACK_STRENGTH,
        ));
    }

    /// Vanilla `Ravager.stunEffect`: a one-in-six chance of the stunned effect particle at
    /// the ravager's head.
    fn stun_effect(&self) {
        if rand::random_range(0..6) != 0 {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let bounds = self.bounding_box();
        let body_rot = f64::from(self.y_body_rot().to_radians());
        let width = bounds.width();
        let position = DVec3::new(
            self.position().x - width * body_rot.sin() + (rand::random::<f64>() * 0.6 - 0.3),
            self.position().y + bounds.height() - 0.3,
            self.position().z + width * body_rot.cos() + (rand::random::<f64>() * 0.6 - 0.3),
        );
        world.send_particles(
            ParticleData::new(
                &vanilla_particle_types::ENTITY_EFFECT,
                ColorParticleOption::new(ArgbColor::new(STUNNED_PARTICLE_COLOR)),
            ),
            position,
            1,
            DVec3::ZERO,
            0.0,
        );
    }

    /// Vanilla `Ravager.roar`: damage every nearby non-illager and shove everything that is
    /// not a player.
    fn roar(&self) {
        if !LivingEntity::is_alive(self) {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let griefing = world.get_game_rule(&MOB_GRIEFING);
        let bounds = self.bounding_box().inflate(ROAR_RADIUS);
        let targets = world.get_entities_in_aabb_matching(&bounds, |entity| {
            entity.entity_type() != &vanilla_entities::RAVAGER
                && entity.is_alive()
                && (griefing || entity.entity_type() != &vanilla_entities::ARMOR_STAND)
        });

        let attacker = self.raider_entity();
        let damage_source = attacker.as_ref().map(|attacker| {
            DamageSource::environment(&vanilla_damage_types::MOB_ATTACK)
                .with_causing_entity(Arc::clone(attacker))
                .with_direct_entity(Arc::clone(attacker))
        });

        for target in targets {
            let Some(living) = target.as_living_entity() else {
                continue;
            };
            let is_illager = REGISTRY
                .entity_types
                .is_in_tag(living.entity_type(), &EntityTypeTag::ILLAGER);
            if !is_illager && let Some(source) = &damage_source {
                living.hurt(&world, source, ROAR_DAMAGE);
            }

            if living.entity_type() != &vanilla_entities::PLAYER {
                self.strong_knockback(target.as_ref());
            }
        }

        self.game_event(&vanilla_game_events::ENTITY_ACTION);
        self.broadcast_entity_event(EntityStatus::RavagerRoared);
    }

    /// Vanilla `Ravager.blockedByItem`: a shield block either stuns the ravager or knocks
    /// the defender back hard.
    ///
    /// Vanilla calls this from `LivingEntity.hurtServer` when the damage was fully blocked.
    /// Steel has no shield/item blocking hook yet (`LivingEntity::hurt_server` still carries
    /// the `TODO: apply item blocking` marker), so this method is not called by the damage
    /// pipeline until that hook lands; the behaviour is implemented here so the hook can
    /// call it directly.
    pub fn blocked_by_item(
        &self,
        defender: &dyn Entity,
        _source: &DamageSource,
        _damage: f32,
        _fully_blocked: bool,
    ) {
        if self.roar_tick() != 0 {
            return;
        }

        if rand::random::<f64>() < 0.5 {
            self.set_stunned_tick(STUN_DURATION);
            self.play_sound(&sound_events::ENTITY_RAVAGER_STUNNED, 1.0, 1.0);
            self.broadcast_entity_event(EntityStatus::RavagerStunned);
            // Vanilla `defender.push(this)`.
            self.push_entity(defender);
        } else {
            self.strong_knockback(defender);
        }

        // Vanilla `defender.syncVelocity = true`.
        defender.mark_velocity_sync();
    }

    /// Vanilla `Ravager.aiStep` after `super.aiStep()`.
    fn ravager_ai_tick(&self) {
        if !LivingEntity::is_alive(self) {
            return;
        }

        let immobile = LivingEntity::is_immobile(self);
        let max_speed = if self.target().is_some() {
            ATTACK_MOVEMENT_SPEED
        } else {
            BASE_MOVEMENT_SPEED
        };
        {
            let mut attributes = self.attributes().lock();
            if immobile {
                attributes.set_base_value(vanilla_attributes::MOVEMENT_SPEED, 0.0);
            } else {
                let base = attributes
                    .get_base_value(vanilla_attributes::MOVEMENT_SPEED)
                    .unwrap_or(BASE_MOVEMENT_SPEED);
                attributes.set_base_value(
                    vanilla_attributes::MOVEMENT_SPEED,
                    base + (max_speed - base) * MOVEMENT_SPEED_LERP,
                );
            }
        }

        if self.horizontal_collision()
            && let Some(world) = self.level()
            && world.get_game_rule(&MOB_GRIEFING)
        {
            let mut destroyed_block = false;
            let bounds = self.bounding_box().inflate(0.2);
            let min = BlockPos::new(
                fast_floor(bounds.min_x()),
                fast_floor(bounds.min_y()),
                fast_floor(bounds.min_z()),
            );
            let max = BlockPos::new(
                fast_floor(bounds.max_x()),
                fast_floor(bounds.max_y()),
                fast_floor(bounds.max_z()),
            );

            for pos in BlockPos::between_closed(min, max) {
                if world
                    .get_block_state(pos)
                    .get_block()
                    .has_tag(&BlockTag::LEAVES)
                {
                    destroyed_block =
                        world.destroy_block_by_entity(pos, true, self) || destroyed_block;
                }
            }

            if !destroyed_block && self.on_ground() {
                self.jump_from_ground();
            }
        }

        if self.roar_tick() > 0 {
            let next = self.roar_tick() - 1;
            self.set_roar_tick(next);
            if next == 10 {
                self.roar();
            }
        }

        if self.attack_tick() > 0 {
            self.set_attack_tick(self.attack_tick() - 1);
        }

        if self.stunned_tick() > 0 {
            let next = self.stunned_tick() - 1;
            self.set_stunned_tick(next);
            self.stun_effect();
            if next == 0 {
                self.play_sound(&sound_events::ENTITY_RAVAGER_ROAR, 1.0, 1.0);
                self.set_roar_tick(20);
            }
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.raider_mut().mob_mut().living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }
}

impl Entity for RavagerEntity {
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

    fn save_additional(&self, nbt: &mut simdnbt::owned::NbtCompound) {
        self.save_mob(nbt);
        self.raider_base.save(nbt);
        nbt.insert("AttackTick", self.attack_tick());
        nbt.insert("StunTick", self.stunned_tick());
        nbt.insert("RoarTick", self.roar_tick());
    }

    fn load_additional(&self, nbt: simdnbt::borrow::NbtCompound<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
        self.set_attack_tick(nbt.int("AttackTick").unwrap_or(0));
        self.set_stunned_tick(nbt.int("StunTick").unwrap_or(0));
        self.set_roar_tick(nbt.int("RoarTick").unwrap_or(0));
    }

    /// Vanilla `Ravager.playStepSound`: the ravager stomp at volume `0.15`.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_RAVAGER_STEP, 0.15, 1.0);
    }
}

impl LivingEntity for RavagerEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self
            .entity_data
            .lock()
            .raider()
            .mob()
            .living_entity()
            .health
            .get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_RAVAGER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_RAVAGER_DEATH)
    }

    /// Vanilla `Ravager.isImmobile`.
    fn is_immobile(&self) -> bool {
        self.default_is_immobile()
            || self.attack_tick() > 0
            || self.stunned_tick() > 0
            || self.roar_tick() > 0
    }

    /// Vanilla `Ravager.hasLineOfSight`: no sight while stunned or roaring.
    fn has_line_of_sight(&self, target: &dyn Entity) -> bool {
        if self.stunned_tick() > 0 || self.roar_tick() > 0 {
            return false;
        }
        LivingEntity::has_line_of_sight(self, target)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Raider::raider_ai_step(self, entity);
        let result = crate::entity::Monster::monster_ai_step(self, entity);
        self.ravager_ai_tick();
        result
    }

    fn on_death(&self, source: &DamageSource) {
        Raider::raider_on_death(self, source);
    }
}

impl Mob for RavagerEntity {
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
        Some(&sound_events::ENTITY_RAVAGER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().raider().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .mob_flags
            .set(flags);
    }

    /// Vanilla `Ravager.getMaxHeadYRot`.
    fn max_head_y_rot(&self) -> f32 {
        45.0
    }

    /// Vanilla `Ravager.updateControlFlags`: a non-raider mob riding the ravager, or any
    /// boat, takes away its goal control.
    fn update_control_flags(&self) {
        let no_controller = self.controlling_passenger().is_none_or(|passenger| {
            !passenger.is_mob()
                || REGISTRY
                    .entity_types
                    .is_in_tag(passenger.entity_type(), &EntityTypeTag::RAIDERS)
        });
        let not_in_boat = self
            .vehicle()
            .is_none_or(|vehicle| !vehicle.entity_type().is_abstract_boat);

        let mut selector = self.mob_base().goal_selector().lock();
        selector.set_control(GoalControl::Move, no_controller);
        selector.set_control(GoalControl::Jump, no_controller && not_in_boat);
        selector.set_control(GoalControl::Look, no_controller);
        selector.set_control(GoalControl::Target, no_controller);
    }

    /// Vanilla `Ravager.checkSpawnObstruction`: only the liquid test.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        !crate::entity::aabb_contains_any_liquid(world, self.bounding_box())
    }

    /// Vanilla `Ravager.doHurtTarget`: start the attack animation, then attack normally.
    fn play_attack_sound(&self) {
        self.set_attack_tick(ATTACK_DURATION);
        self.broadcast_entity_event(EntityStatus::StartAttacking);
        self.play_sound(&sound_events::ENTITY_RAVAGER_ATTACK, 1.0, 1.0);
    }

    fn requires_custom_persistence(&self) -> bool {
        Raider::raider_requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        Raider::raider_remove_when_far_away(self, dist_sqr)
    }
}

impl crate::entity::Monster for RavagerEntity {}

impl PathfinderMob for RavagerEntity {}

impl Raider for RavagerEntity {
    fn raider_base(&self) -> &RaiderBase {
        &self.raider_base
    }

    fn is_celebrating(&self) -> bool {
        *self.entity_data.lock().raider().is_celebrating.get()
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.entity_data
            .lock()
            .raider_mut()
            .is_celebrating
            .set(celebrating);
    }

    /// Vanilla `Ravager.applyRaidBuffs` is empty.
    fn apply_raid_buffs(
        &self,
        _world: &Arc<World>,
        _raid: &RaidSnapshot,
        _wave: i32,
        _is_captain: bool,
    ) {
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_RAVAGER_CELEBRATE
    }

    /// Vanilla `Ravager.canBeLeader` is `false`.
    fn can_be_leader(&self) -> bool {
        false
    }
}

const _: () = {
    assert!(XP_REWARD == 20);
    assert!(ATTACK_DURATION == 10);
    assert!(STUN_DURATION == 40);
};
