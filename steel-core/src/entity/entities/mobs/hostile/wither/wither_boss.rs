//! Vanilla `WitherBoss` (`net.minecraft.world.entity.boss.wither.WitherBoss`).
//!
//! The wither is a flying `Monster` whose three heads shoot `WitherSkull`s: the main head
//! aims through vanilla's `RangedAttackGoal`, the two side heads re-acquire targets every
//! `10 + rand(10)` ticks. While its synced `Invul` counter runs down it is invulnerable,
//! heals, and finally explodes; below half health it gains a blue aura and deflects arrows
//! and wind charges.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_math::DEGREE_90;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{ColorParticleOption, ParticleData};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::WitherBossEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, level_events, sound_events, vanilla_attributes,
    vanilla_entities, vanilla_items, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::Difficulty;
use steel_utils::{
    ArgbColor, BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey,
};
use text_components::TextComponent;
use uuid::Uuid;

use crate::entity::ai::control::rotate_towards;
use crate::entity::ai::goal::{
    Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    RandomLookAroundGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::ai::targeting::TargetingConditions;
use steel_protocol::packets::game::{BossBarColor, BossBarOverlay, CBossEvent};

use crate::entity::boss::event::ServerBossEvent;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::WitherSkullEntity;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, MobEffectInstance, Monster, PathfinderMob, Projectile,
    RemovalReason, SharedEntity, next_entity_id,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `WitherBoss.INVULNERABLE_TICKS`.
const INVULNERABLE_TICKS: i32 = 220;
/// Vanilla `WitherBoss` constructor `this.xpReward = 50`.
const XP_REWARD: i32 = 50;
/// Vanilla `WitherBoss.performRangedAttack(int, LivingEntity)` dangerous-skull chance.
const DANGEROUS_SKULL_CHANCE: f32 = 0.001;
/// Vanilla `WitherBoss.destroyBlocksTick` reset value after being hurt.
const DESTROY_BLOCKS_TICK: i32 = 20;
/// Vanilla `ItemEntity.setExtendedLifetime` age.
const EXTENDED_LIFETIME_AGE: i32 = -6000;
/// Vanilla `WitherBoss.TARGETING_CONDITIONS` combat range.
const HEAD_TARGETING_RANGE: f64 = 20.0;
/// Vanilla `WitherBoss.customServerAiStep` head search box inflation.
const HEAD_SEARCH_HORIZONTAL: f64 = 20.0;
const HEAD_SEARCH_VERTICAL: f64 = 8.0;
/// Vanilla `WitherBoss.customServerAiStep` `distanceToSqr(current) > 900.0` gate.
const HEAD_TARGET_MAX_DISTANCE_SQR: f64 = 900.0;
/// Vanilla `WitherBoss.customServerAiStep` idle-shot target offsets.
const IDLE_SHOT_HORIZONTAL_RANGE: f64 = 10.0;
const IDLE_SHOT_VERTICAL_RANGE: f64 = 5.0;
/// Vanilla `WitherBoss.customServerAiStep` side-head re-target intervals.
const HEAD_TICK_MIN: i32 = 10;
const HEAD_TICK_RANDOM: i32 = 10;
const HEAD_TARGET_COOLDOWN_MIN: i32 = 40;
const HEAD_TARGET_COOLDOWN_RANDOM: i32 = 20;
/// Vanilla `WitherBoss.customServerAiStep` `idleHeadUpdates[i]++ > 15`.
const IDLE_HEAD_UPDATE_THRESHOLD: i32 = 15;
/// Vanilla `WitherBoss.hurtServer` bump applied to every idle head counter.
const HURT_IDLE_HEAD_UPDATE_BUMP: i32 = 3;
/// Vanilla `WitherBoss.aiStep` head offsets (`getHeadX`/`getHeadY`/`getHeadZ`).
const SIDE_HEAD_HORIZONTAL_OFFSET: f64 = 1.3;
const SIDE_HEAD_ANGLE_STEP: f32 = 180.0;
const MAIN_HEAD_HEIGHT: f32 = 3.0;
const SIDE_HEAD_HEIGHT: f32 = 2.2;
/// Vanilla `WitherBoss.aiStep` head-rotation clamps (`rotlerp`).
const HEAD_X_ROT_CLAMP: f32 = 40.0;
const HEAD_Y_ROT_CLAMP: f32 = 10.0;
/// Vanilla `WitherBoss.aiStep` particle radius factor (`0.3F * getScale()`).
const HEAD_PARTICLE_RADIUS: f32 = 0.3;
/// Vanilla `WitherBoss.aiStep` spawn-effect column height (`3.3F * getScale()`).
const SPAWN_EFFECT_HEIGHT: f32 = 3.3;
/// Vanilla `WitherBoss` goal `RangedAttackGoal(this, 1.0, 40, 20.0F)`.
const RANGED_ATTACK_SPEED_MODIFIER: f64 = 1.0;
const RANGED_ATTACK_INTERVAL: i32 = 40;
const RANGED_ATTACK_RADIUS: f32 = 20.0;
const RANGED_ATTACK_SEE_TIME: i32 = 5;
/// Vanilla `RangedAttackGoal.tick` look control speeds.
const RANGED_ATTACK_LOOK_SPEED: f32 = 30.0;
/// Vanilla `Mob.getMaxUpStep` fallback when the step-height attribute is absent.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `WitherBoss` initial synced alternative-target ids.
const DEFAULT_TARGET_ID: i32 = 0;

/// Vanilla `WitherBoss` powered-state particle color `(0.7F, 0.7F, 0.5F)` packed the way
/// `ARGB.colorFromFloat` does (`floor(channel * 255)`): `0xFFB2B27F`.
const POWERED_EFFECT_COLOR: ArgbColor = ArgbColor::new(0xFFB2_B27F_u32 as i32);
/// Vanilla `WitherBoss` spawn-invulnerability particle color `(0.7F, 0.7F, 0.9F)` packed the
/// same way: `0xFFB2B2E5`.
const SPAWN_EFFECT_COLOR: ArgbColor = ArgbColor::new(0xFFB2_B2E5_u32 as i32);

/// Vanilla `Random.nextGaussian` (Marsaglia polar method) for the thread-local RNG.
fn gaussian() -> f64 {
    loop {
        let d = 2.0 * rand::random::<f64>() - 1.0;
        let e = 2.0 * rand::random::<f64>() - 1.0;
        let f = d * d + e * e;

        if f < 1.0 && f != 0.0 {
            return d * (-2.0 * f.ln() / f).sqrt();
        }
    }
}

/// Vanilla `WitherBoss`.
#[entity_behavior(class = "WitherBoss")]
pub struct WitherBossEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<WitherBossEntityData>,
    /// Vanilla `WitherBoss.bossEvent` (purple `Progress` bar, darkening the screen).
    boss_event: SyncMutex<ServerBossEvent>,
    head_x_rots: SyncMutex<[f32; 2]>,
    head_y_rots: SyncMutex<[f32; 2]>,
    /// Vanilla `WitherBoss.nextHeadUpdate` for the two side heads.
    next_head_update: SyncMutex<[i32; 2]>,
    /// Vanilla `WitherBoss.idleHeadUpdates` for the two side heads.
    idle_head_updates: SyncMutex<[i32; 2]>,
    /// Vanilla `WitherBoss.destroyBlocksTick`.
    destroy_blocks_tick: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WitherBossEntity`.
unsafe impl DowncastType for WitherBossEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wither_boss");
}

impl WitherBossEntity {
    /// Creates a new wither at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a wither from persisted base entity state.
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
        let mut entity_data = WitherBossEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.target_a.set(DEFAULT_TARGET_ID);
        entity_data.target_b.set(DEFAULT_TARGET_ID);
        entity_data.target_c.set(DEFAULT_TARGET_ID);
        entity_data.id_inv.set(0);

        {
            // Vanilla `WitherBoss.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, WitherDoNothingGoal::new());
            goal_selector.add_goal(2, WitherRangedAttackGoal::new());
            goal_selector.add_goal(5, WaterAvoidingRandomStrollGoal::new(1.0));
            goal_selector.add_goal(6, LookAtPlayerGoal::new(8.0));
            goal_selector.add_goal(7, RandomLookAroundGoal::new());

            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, HurtByTargetGoal::new());
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new_with_interval(
                    0,
                    false,
                    false,
                    living_entity_selector,
                ),
            );
        }

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            boss_event: SyncMutex::new(ServerBossEvent::new(
                Uuid::new_v4(),
                TextComponent::new(),
                BossBarColor::Purple,
                BossBarOverlay::Progress,
            )),
            head_x_rots: SyncMutex::new([0.0; 2]),
            head_y_rots: SyncMutex::new([0.0; 2]),
            next_head_update: SyncMutex::new([0; 2]),
            idle_head_updates: SyncMutex::new([0; 2]),
            destroy_blocks_tick: SyncMutex::new(0),
        };

        // Vanilla `WitherBoss` constructs its bar with `getDisplayName()`; Steel resolves names
        // through `Entity::display_name`, so the bar is named once the entity exists. No player
        // tracks a freshly constructed entity, so the intermediate operation is dropped.
        entity.boss_event.lock().set_darken_screen(true);
        entity.refresh_boss_bar_name();
        let _ = entity.boss_event.lock().drain_packets();
        entity
    }

    /// Vanilla `WitherBoss` `bossEvent.setName(this.getDisplayName())`.
    fn refresh_boss_bar_name(&self) {
        self.boss_event.lock().set_name(self.display_name());
    }

    /// The wither's boss bar, drained by the owning server glue.
    #[must_use]
    pub const fn boss_event(&self) -> &SyncMutex<ServerBossEvent> {
        &self.boss_event
    }

    /// Drains queued boss-bar wire operations.
    pub fn drain_boss_bar_packets(&self) -> Vec<CBossEvent> {
        self.boss_event.lock().drain_packets()
    }

    /// Vanilla `WitherBoss.startSeenByPlayer`.
    pub fn start_seen_by_player(&self, player: Arc<Player>) {
        self.boss_event.lock().add_player(player);
    }

    /// Vanilla `WitherBoss.stopSeenByPlayer`.
    pub fn stop_seen_by_player(&self, player: &Player) {
        self.boss_event.lock().remove_player(&player.uuid());
    }

    /// Vanilla `WitherBoss.getInvulnerableTicks` (`Invul` synced counter).
    #[must_use]
    pub fn invulnerable_ticks(&self) -> i32 {
        *self.entity_data.lock().id_inv.get()
    }

    /// Vanilla `WitherBoss.setInvulnerableTicks`.
    pub fn set_invulnerable_ticks(&self, invulnerable_ticks: i32) {
        self.entity_data.lock().id_inv.set(invulnerable_ticks);
    }

    /// Vanilla `WitherBoss.getAlternativeTarget`.
    #[must_use]
    pub fn alternative_target(&self, head_index: usize) -> i32 {
        let data = self.entity_data.lock();
        match head_index {
            0 => *data.target_a.get(),
            1 => *data.target_b.get(),
            _ => *data.target_c.get(),
        }
    }

    /// Vanilla `WitherBoss.setAlternativeTarget`.
    pub fn set_alternative_target(&self, head_index: usize, entity_id: i32) {
        let mut data = self.entity_data.lock();
        match head_index {
            0 => data.target_a.set(entity_id),
            1 => data.target_b.set(entity_id),
            _ => data.target_c.set(entity_id),
        }
    }

    /// Vanilla `WitherBoss.isPowered`: half health or less.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.get_health() <= self.get_max_health() / 2.0
    }

    /// Vanilla `WitherBoss.makeInvulnerable`.
    ///
    /// The wither-summoning ritual calls this when the T-shaped soul sand and three wither
    /// skulls complete.
    pub fn make_invulnerable(&self) {
        self.set_invulnerable_ticks(INVULNERABLE_TICKS);
        self.boss_event.lock().set_progress(0.0);
        self.set_health(self.get_max_health() / 3.0);
    }

    /// Vanilla `WitherBoss.getHeadYRots`.
    #[must_use]
    pub fn head_y_rots(&self) -> [f32; 2] {
        *self.head_y_rots.lock()
    }

    /// Vanilla `WitherBoss.getHeadXRots`.
    #[must_use]
    pub fn head_x_rots(&self) -> [f32; 2] {
        *self.head_x_rots.lock()
    }

    /// Vanilla `WitherBoss.getHeadX`.
    #[must_use]
    pub fn head_x(&self, index: i32) -> f64 {
        if index <= 0 {
            return self.position().x;
        }

        self.position().x
            + self.side_head_angle(index).cos()
                * SIDE_HEAD_HORIZONTAL_OFFSET
                * f64::from(self.get_scale())
    }

    /// Vanilla `WitherBoss.getHeadY`.
    #[must_use]
    pub fn head_y(&self, index: i32) -> f64 {
        let height = if index <= 0 {
            MAIN_HEAD_HEIGHT
        } else {
            SIDE_HEAD_HEIGHT
        };
        self.position().y + f64::from(height) * f64::from(self.get_scale())
    }

    /// Vanilla `WitherBoss.getHeadZ`.
    #[must_use]
    pub fn head_z(&self, index: i32) -> f64 {
        if index <= 0 {
            return self.position().z;
        }

        self.position().z
            + self.side_head_angle(index).sin()
                * SIDE_HEAD_HORIZONTAL_OFFSET
                * f64::from(self.get_scale())
    }

    /// The shared `WitherBoss.getHeadX`/`getHeadZ` side-head angle, converted to radians.
    fn side_head_angle(&self, index: i32) -> f64 {
        f64::from(self.y_body_rot() + SIDE_HEAD_ANGLE_STEP * (index - 1) as f32) * f64::consts::PI
            / 180.0
    }

    /// Vanilla `WitherBoss.canDestroy`: every non-air block outside `WITHER_IMMUNE`.
    #[must_use]
    pub fn can_destroy(state: BlockStateId) -> bool {
        !state.is_air() && !state.get_block().has_tag(&BlockTag::WITHER_IMMUNE)
    }

    fn destroy_blocks_tick(&self) -> i32 {
        *self.destroy_blocks_tick.lock()
    }

    fn set_destroy_blocks_tick(&self, ticks: i32) {
        *self.destroy_blocks_tick.lock() = ticks;
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

    /// Vanilla `WitherBoss.performRangedAttack(int, LivingEntity)`: fire from `head` at the
    /// target's lower torso, with the main head's rare dangerous-skull roll.
    fn perform_ranged_attack_living(&self, head: i32, target: &dyn Entity) {
        let position = target.position();
        self.perform_ranged_attack(
            head,
            position.x,
            position.y + target.get_eye_height() * 0.5,
            position.z,
            head == 0 && rand::random::<f32>() < DANGEROUS_SKULL_CHANCE,
        );
    }

    /// Vanilla `WitherBoss.performRangedAttack(int, double, double, double, boolean)`.
    fn perform_ranged_attack(&self, head: i32, tx: f64, ty: f64, tz: f64, dangerous: bool) {
        self.fire_skull(head, DVec3::new(tx, ty, tz), dangerous);
    }

    /// Vanilla `RangedAttackMob.performRangedAttack(LivingEntity, float)`: the main head shoots,
    /// ignoring the (vanilla-unused) charge power.
    fn perform_ranged_attack_target(&self, target: &dyn Entity) {
        self.perform_ranged_attack_living(0, target);
    }

    /// Vanilla `WitherBoss.performRangedAttack` projectile spawn — the single place the wither
    /// constructs a `WitherSkull`.
    ///
    /// Vanilla `new WitherSkull(level, this, direction)` places the skull at the wither's
    /// position, makes the wither its owner, copies the wither's rotation, and assigns
    /// `direction.normalize() * 0.1` as its initial movement; the wither then moves the skull to
    /// the firing head and adds it to the level.
    fn fire_skull(&self, head: i32, target: DVec3, dangerous: bool) {
        let Some(world) = self.level() else {
            return;
        };

        if !self.is_silent() {
            world.level_event(
                level_events::SOUND_WITHER_BOSS_SHOOT,
                self.block_position(),
                0,
                None,
            );
        }

        let head_position = DVec3::new(self.head_x(head), self.head_y(head), self.head_z(head));
        let direction = target - head_position;

        // Vanilla `new WitherSkull(level, this, direction)` — the constructor places the skull at
        // `head_position` and applies `AbstractHurtingProjectile.assignDirectionalMovement(
        // direction, 0.1)`; the wither then re-sets the owner and the dangerous flag.
        let skull = WitherSkullEntity::with_direction(
            &vanilla_entities::WITHER_SKULL,
            next_entity_id(),
            head_position,
            direction.normalize_or_zero(),
            Arc::downgrade(&world),
        );
        skull.set_owner_uuid(Some(self.uuid()));
        if dangerous {
            skull.set_dangerous(true);
        }

        let skull: SharedEntity = Arc::new(skull);
        if let Err(error) = world.try_add_entity(skull) {
            log::warn!("Failed to spawn wither skull: {error}");
        }
    }

    /// Vanilla `WitherBoss.aiStep` flight block: damp vertical momentum and steer toward the
    /// main head's alternative target.
    fn apply_flight_toward_target(&self) {
        let Some(world) = self.level() else {
            return;
        };

        let position = self.position();
        let mut movement = self.velocity() * DVec3::new(1.0, 0.6, 1.0);
        let alternative_target = self.alternative_target(0);

        if alternative_target > 0
            && let Some(target) = world.get_entity_by_id(alternative_target)
        {
            let target_position = target.position();
            let mut vertical = movement.y;
            if position.y < target_position.y
                || (!self.is_powered() && position.y < target_position.y + 5.0)
            {
                vertical = vertical.max(0.0);
                vertical += 0.3 - vertical * 0.6;
            }

            movement = DVec3::new(movement.x, vertical, movement.z);
            let delta = DVec3::new(
                target_position.x - position.x,
                0.0,
                target_position.z - position.z,
            );
            if delta.x * delta.x + delta.z * delta.z > 9.0 {
                let scale = delta.normalize();
                movement += DVec3::new(
                    scale.x * 0.3 - movement.x * 0.6,
                    0.0,
                    scale.z * 0.3 - movement.z * 0.6,
                );
            }
        }

        self.set_velocity(movement);
        if movement.x * movement.x + movement.z * movement.z > 0.05 {
            let yaw = (movement.z.atan2(movement.x) as f32).to_degrees() - DEGREE_90;
            self.set_rotation((yaw, self.rotation().1));
        }
    }

    /// Vanilla `WitherBoss.aiStep` head-aim block, run after the shared AI step.
    fn update_head_rotations(&self, world: &World) {
        let mut x_rots = self.head_x_rots.lock();
        let mut y_rots = self.head_y_rots.lock();

        for index in 0..2 {
            let entity_id = self.alternative_target(index + 1);
            let target = (entity_id > 0)
                .then(|| world.get_entity_by_id(entity_id))
                .flatten();

            if let Some(target) = target {
                let head = index as i32 + 1;
                let xd = target.position().x - self.head_x(head);
                let yd = target.get_eye_y() - self.head_y(head);
                let zd = target.position().z - self.head_z(head);
                let distance = (xd * xd + zd * zd).sqrt();
                let target_y_rot = (zd.atan2(xd) as f32).to_degrees() - DEGREE_90;
                let target_x_rot = -(yd.atan2(distance) as f32).to_degrees();
                x_rots[index] = rotate_towards(x_rots[index], target_x_rot, HEAD_X_ROT_CLAMP);
                y_rots[index] = rotate_towards(y_rots[index], target_y_rot, HEAD_Y_ROT_CLAMP);
            } else {
                y_rots[index] = rotate_towards(y_rots[index], self.y_body_rot(), HEAD_Y_ROT_CLAMP);
            }
        }
    }

    /// Vanilla `WitherBoss.aiStep` particles: head smoke, the powered blue aura, and the
    /// spawn-invulnerability column.
    fn spawn_head_particles(&self, world: &World) {
        let scale = self.get_scale();
        let radius = HEAD_PARTICLE_RADIUS * scale;
        let powered = self.is_powered();
        let smoke = ParticleData::simple(&vanilla_particle_types::SMOKE);
        let powered_effect = ParticleData::new(
            &vanilla_particle_types::ENTITY_EFFECT,
            ColorParticleOption::new(POWERED_EFFECT_COLOR),
        );

        for head in 0..3 {
            let position = DVec3::new(self.head_x(head), self.head_y(head), self.head_z(head));
            let jittered = position
                + DVec3::new(
                    gaussian() * f64::from(radius),
                    gaussian() * f64::from(radius),
                    gaussian() * f64::from(radius),
                );
            world.send_particles(smoke.clone(), jittered, 1, DVec3::ZERO, 0.0);

            if powered && rand::random_range(0..4) == 0 {
                world.send_particles(powered_effect.clone(), jittered, 1, DVec3::ZERO, 0.0);
            }
        }

        if self.invulnerable_ticks() > 0 {
            let height = f64::from(SPAWN_EFFECT_HEIGHT * scale);
            let position = self.position();
            let spawn_effect = ParticleData::new(
                &vanilla_particle_types::ENTITY_EFFECT,
                ColorParticleOption::new(SPAWN_EFFECT_COLOR),
            );

            for _ in 0..3 {
                let jittered = DVec3::new(
                    position.x + gaussian(),
                    position.y + rand::random::<f64>() * height,
                    position.z + gaussian(),
                );
                world.send_particles(spawn_effect.clone(), jittered, 1, DVec3::ZERO, 0.0);
            }
        }
    }

    /// Vanilla `WitherBoss.customServerAiStep` side-head targeting and shooting.
    fn tick_side_heads(&self, world: &Arc<World>) {
        for index in 1..3 {
            let array_index = (index - 1) as usize;
            if self.tick_count() < self.next_head_update.lock()[array_index] {
                continue;
            }

            self.next_head_update.lock()[array_index] =
                self.tick_count() + HEAD_TICK_MIN + rand::random_range(0..HEAD_TICK_RANDOM);

            if matches!(world.difficulty(), Difficulty::Normal | Difficulty::Hard) {
                let idle_updates = {
                    let mut updates = self.idle_head_updates.lock();
                    let previous = updates[array_index];
                    updates[array_index] = previous.wrapping_add(1);
                    previous
                };
                if idle_updates > IDLE_HEAD_UPDATE_THRESHOLD {
                    let position = self.position();
                    let target = DVec3::new(
                        rand::random_range(
                            (position.x - IDLE_SHOT_HORIZONTAL_RANGE)
                                ..(position.x + IDLE_SHOT_HORIZONTAL_RANGE),
                        ),
                        rand::random_range(
                            (position.y - IDLE_SHOT_VERTICAL_RANGE)
                                ..(position.y + IDLE_SHOT_VERTICAL_RANGE),
                        ),
                        rand::random_range(
                            (position.z - IDLE_SHOT_HORIZONTAL_RANGE)
                                ..(position.z + IDLE_SHOT_HORIZONTAL_RANGE),
                        ),
                    );
                    self.fire_skull(index + 1, target, true);
                    self.idle_head_updates.lock()[array_index] = 0;
                }
            }

            let head_target = self.alternative_target(index as usize);
            if head_target > 0 {
                let current = world.get_entity_by_id(head_target);
                let valid = current.as_ref().is_some_and(|entity| {
                    entity.as_living_entity().is_some_and(|living| {
                        Mob::can_attack(self, living)
                            && self.position().distance_squared(entity.position())
                                <= HEAD_TARGET_MAX_DISTANCE_SQR
                            && self.has_line_of_sight(entity.as_ref())
                    })
                });

                if valid {
                    if let Some(current) = current {
                        self.perform_ranged_attack_living(index + 1, current.as_ref());
                    }
                    self.next_head_update.lock()[array_index] = self.tick_count()
                        + HEAD_TARGET_COOLDOWN_MIN
                        + rand::random_range(0..HEAD_TARGET_COOLDOWN_RANDOM);
                    self.idle_head_updates.lock()[array_index] = 0;
                } else {
                    self.set_alternative_target(index as usize, 0);
                }
                continue;
            }

            let search_box = self.bounding_box().inflate_xyz(
                HEAD_SEARCH_HORIZONTAL,
                HEAD_SEARCH_VERTICAL,
                HEAD_SEARCH_HORIZONTAL,
            );
            let conditions = TargetingConditions::for_combat()
                .range(HEAD_TARGETING_RANGE)
                .selector(living_entity_selector);
            let candidates = world.get_entities_in_aabb_matching(&search_box, |entity| {
                entity
                    .as_living_entity()
                    .is_some_and(|target| conditions.test(world.as_ref(), Some(self), target))
            });

            if !candidates.is_empty() {
                let chosen = rand::random_range(0..candidates.len());
                self.set_alternative_target(index as usize, candidates[chosen].id());
            }
        }
    }

    /// Vanilla `WitherBoss.customServerAiStep` block destruction after being hurt.
    fn tick_block_destruction(&self, world: &Arc<World>) {
        if self.destroy_blocks_tick() <= 0 {
            return;
        }

        let remaining = self.destroy_blocks_tick() - 1;
        self.set_destroy_blocks_tick(remaining);
        if remaining != 0 || !world.get_game_rule(&MOB_GRIEFING) {
            return;
        }

        let bounding_box = self.bounding_box();
        let width = (bounding_box.width() / 2.0 + 1.0).floor() as i32;
        let height = bounding_box.height().floor() as i32;
        let block_position = self.block_position();
        let mut destroyed = false;

        for position in BlockPos::between_closed(
            BlockPos::new(
                block_position.x() - width,
                block_position.y(),
                block_position.z() - width,
            ),
            BlockPos::new(
                block_position.x() + width,
                block_position.y() + height,
                block_position.z() + width,
            ),
        ) {
            let state = world.get_block_state(position);
            if Self::can_destroy(state) {
                destroyed = world.destroy_block_by_entity(position, true, self) || destroyed;
            }
        }

        if destroyed {
            world.level_event(
                level_events::SOUND_WITHER_BLOCK_BREAK,
                self.block_position(),
                0,
                None,
            );
        }
    }
}

/// Vanilla `WitherBoss.LIVING_ENTITY_SELECTOR`: never a `WITHER_FRIENDS` member, always attackable.
fn living_entity_selector(target: &dyn LivingEntity, _world: &World) -> bool {
    target.attackable()
        && !REGISTRY
            .entity_types
            .is_in_tag(target.entity_type(), &EntityTypeTag::WITHER_FRIENDS)
}

impl Entity for WitherBossEntity {
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

    /// Vanilla `WitherBoss.makeStuckInBlock` is a no-op.
    fn make_stuck_in_block(&self, _state: BlockStateId, _speed_multiplier: DVec3) {}

    /// Vanilla `WitherBoss.canRide` refuses every vehicle.
    fn can_ride(&self, _vehicle: &dyn Entity) -> bool {
        false
    }

    /// Vanilla `WitherBoss.checkDespawn`: a Peaceful world discards the wither outright,
    /// otherwise it never accrues no-action time and never despawns.
    fn check_despawn(&self) {
        let peaceful = self
            .level()
            .is_some_and(|world| world.difficulty() == Difficulty::Peaceful);

        if peaceful && !self.entity_type().allowed_in_peaceful {
            self.set_removed(RemovalReason::Discarded);
        } else {
            self.set_no_action_time(0);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("Invul", self.invulnerable_ticks());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_invulnerable_ticks(nbt.int("Invul").unwrap_or(0));
        if self.custom_name().is_some() {
            self.refresh_boss_bar_name();
        }
    }

    /// Vanilla `WitherBoss.setCustomName` keeps the boss bar in sync.
    fn set_custom_name(&self, custom_name: Option<TextComponent>) {
        EntityBase::set_custom_name(&self.base, custom_name.clone());
        if let Some(synced_data) = self.synced_data() {
            synced_data.set_custom_name(custom_name);
        }
        self.refresh_boss_bar_name();
    }
}

impl LivingEntity for WitherBossEntity {
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
        Some(&sound_events::ENTITY_WITHER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WITHER_DEATH)
    }

    fn should_drop_experience(&self) -> bool {
        Monster::should_drop_experience_monster(self)
    }

    fn should_drop_loot(&self, world: &World) -> bool {
        Monster::should_drop_loot_monster(self, world)
    }

    /// Vanilla `WitherBoss.canBeAffected` rejects the wither effect.
    fn can_be_affected(&self, effect: &MobEffectInstance) -> bool {
        if effect.effect() == vanilla_mob_effects::WITHER {
            return false;
        }

        self.default_can_be_affected(effect)
    }

    /// Vanilla `WitherBoss.addEffect` refuses every effect.
    fn add_mob_effect(&self, _effect: MobEffectInstance) -> bool {
        false
    }

    /// Vanilla `WitherBoss.hurtServer` rejection gates.
    ///
    /// Vanilla returns `false` before `super.hurtServer` for a `WITHER_IMMUNE_TO` source, another
    /// wither, a spawn-invulnerable wither hit by anything that does not bypass invulnerability,
    /// a powered wither hit by an arrow or wind charge, and any `WITHER_FRIENDS` attacker. Steel's
    /// [`LivingEntity::hurt_server`] consults this hook before it runs the damage pipeline, and
    /// `actually_hurt` consults it again, so a gated source never reduces health.
    fn default_is_invulnerable_to(&self, source: &DamageSource) -> bool {
        if source.is(&DamageTypeTag::WITHER_IMMUNE_TO)
            || source
                .causing_entity()
                .is_some_and(|entity| entity.entity_type() == &vanilla_entities::WITHER)
        {
            return true;
        }

        if self.invulnerable_ticks() > 0 && !source.bypasses_invulnerability() {
            return true;
        }

        if self.is_powered()
            && source.direct_entity().is_some_and(|entity| {
                entity.entity_type().is_abstract_arrow
                    || entity.entity_type() == &vanilla_entities::WIND_CHARGE
            })
        {
            return true;
        }

        if source.causing_entity().is_some_and(|entity| {
            REGISTRY
                .entity_types
                .is_in_tag(entity.entity_type(), &EntityTypeTag::WITHER_FRIENDS)
        }) {
            return true;
        }

        self.is_invulnerable_to_base(source)
    }

    /// Vanilla `WitherBoss.hurtServer` side effects: every accepted hit restarts the
    /// `destroyBlocksTick` countdown and speeds up the side heads' idle shots.
    ///
    /// Steel has no hook before the damage-cooldown gate, so the updates run in
    /// [`LivingEntity::before_actually_hurt`] — the vanilla pre-`super` position within the same
    /// tick.
    fn before_actually_hurt(&self, _source: &DamageSource, _amount: f32) {
        if self.destroy_blocks_tick() <= 0 {
            self.set_destroy_blocks_tick(DESTROY_BLOCKS_TICK);
        }

        for update in self.idle_head_updates.lock().iter_mut() {
            *update += HURT_IDLE_HEAD_UPDATE_BUMP;
        }
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `WitherBoss.aiStep` steers before `super.aiStep()` and aims the heads after.
        self.apply_flight_toward_target();
        let result = Monster::monster_ai_step(self, entity);
        if let Some(world) = self.level() {
            self.update_head_rotations(&world);
            self.spawn_head_particles(&world);
        }
        result
    }

    /// Vanilla `WitherBoss.dropCustomDeathLoot`: an extended-lifetime nether star.
    ///
    /// Equipment drops are the separate `Mob::drop_custom_death_loot_mob` step
    /// vanilla runs after `dropCustomDeathLoot`.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        let Some(nether_star) =
            self.spawn_at_location(ItemStack::new(&vanilla_items::NETHER_STAR), 0.0)
        else {
            return;
        };

        // Vanilla `ItemEntity.setExtendedLifetime` sets `age = -6000`.
        nether_star.set_age(EXTENDED_LIFETIME_AGE);
    }
}

impl Mob for WitherBossEntity {
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
        Some(&sound_events::ENTITY_WITHER_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    /// Vanilla `WitherBoss.customServerAiStep`.
    fn custom_server_ai_step(&self) {
        let Some(world) = self.level() else {
            return;
        };

        let invulnerable_ticks = self.invulnerable_ticks();
        if invulnerable_ticks > 0 {
            let remaining = invulnerable_ticks - 1;
            self.boss_event
                .lock()
                .set_progress(1.0 - remaining as f32 / INVULNERABLE_TICKS as f32);

            if remaining <= 0 {
                // Vanilla `level.explode(this, getX(), getEyeY(), getZ(), 7.0F, false,
                // ExplosionInteraction.MOB)`.
                // BLOCKED(`World::explode`) — Steel has no explosion foundation yet (see the
                // same gap recorded in `mobs/hostile/creeper/creeper.rs`).
                if !self.is_silent() {
                    world.global_level_event(
                        level_events::SOUND_WITHER_BOSS_SPAWN,
                        self.block_position(),
                        0,
                    );
                }
            }

            self.set_invulnerable_ticks(remaining);
            if self.tick_count() % 10 == 0 {
                self.heal(10.0);
            }
            return;
        }

        self.tick_side_heads(&world);

        if let Some(target) = self.target() {
            self.set_alternative_target(0, target.id());
        } else {
            self.set_alternative_target(0, DEFAULT_TARGET_ID);
        }

        self.tick_block_destruction(&world);

        if self.tick_count() % 20 == 0 {
            self.heal(1.0);
        }

        let max_health = self.get_max_health();
        if max_health > 0.0 {
            self.boss_event
                .lock()
                .set_progress(self.get_health() / max_health);
        }
    }
}

impl Monster for WitherBossEntity {}

impl PathfinderMob for WitherBossEntity {}

/// Vanilla `WitherBoss.WitherDoNothingGoal`: the wither holds every movement control while its
/// spawn invulnerability runs.
struct WitherDoNothingGoal;

impl WitherDoNothingGoal {
    const fn new() -> Self {
        Self
    }
}

impl Goal for WitherDoNothingGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::JUMP | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        wither_of(mob).is_some_and(|wither| wither.invulnerable_ticks() > 0)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.can_use(mob)
    }
}

/// Vanilla `RangedAttackGoal(this, 1.0, 40, 20.0F)` for the wither's main head.
struct WitherRangedAttackGoal {
    target: Option<SharedEntity>,
    attack_time: i32,
    see_time: i32,
}

impl WitherRangedAttackGoal {
    const fn new() -> Self {
        Self {
            target: None,
            attack_time: -1,
            see_time: 0,
        }
    }
}

/// Recovers the concrete wither behind a goal context.
fn wither_of(mob: &dyn PathfinderMob) -> Option<&WitherBossEntity> {
    mob.as_entity_event_source()
        .downcast_ref::<WitherBossEntity>()
}

impl Goal for WitherRangedAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        let alive = target
            .as_living_entity()
            .is_some_and(LivingEntity::is_alive);
        if !alive {
            return false;
        }

        self.target = Some(target);
        true
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.can_use(mob) {
            return true;
        }

        self.target.as_ref().is_some_and(|target| target.is_alive())
            && !mob.mob_base().navigation().lock().is_done()
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.target = None;
        self.see_time = 0;
        self.attack_time = -1;
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let Some(wither) = wither_of(mob) else {
            return;
        };

        let target_position = target.position();
        let distance_sqr = mob.position().distance_squared(target_position);
        let has_line_of_sight = mob.has_line_of_sight_cached(target.as_ref());
        self.see_time = if has_line_of_sight {
            self.see_time + 1
        } else {
            0
        };

        let attack_radius_sqr = f64::from(RANGED_ATTACK_RADIUS) * f64::from(RANGED_ATTACK_RADIUS);
        if distance_sqr > attack_radius_sqr || self.see_time < RANGED_ATTACK_SEE_TIME {
            mob.move_to_pos(target_position, RANGED_ATTACK_SPEED_MODIFIER);
        } else {
            mob.mob_base().navigation().lock().stop();
        }

        mob.mob_base().controls().lock().look_control.set_look_at(
            target_position,
            RANGED_ATTACK_LOOK_SPEED,
            RANGED_ATTACK_LOOK_SPEED,
        );

        self.attack_time -= 1;
        if self.attack_time == 0 {
            if has_line_of_sight {
                wither.perform_ranged_attack_target(target.as_ref());
            }
            self.attack_time = RANGED_ATTACK_INTERVAL;
        } else if self.attack_time < 0 {
            self.attack_time = RANGED_ATTACK_INTERVAL;
        }
    }
}
