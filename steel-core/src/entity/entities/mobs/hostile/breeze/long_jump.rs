//! Vanilla `monster/breeze/LongJump` and the `BreezeUtil` geometry it shares
//! with `Slide`.
//!
//! The launch itself is pure motion (pose swap, friction flag, `setDeltaMovement`
//! through `Entity::set_velocity`), so nothing here is gated; only the projectile
//! release in [`super::shoot`] needs the breeze wind charge factory.

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_data::EntityPose;
use steel_registry::sound_events;
use steel_registry::vanilla_blocks;
use steel_utils::BlockPos;

use crate::entity::ai::behavior::long_jump_util::calculate_jump_vector_for_angle;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::{LivingEntity, SharedEntity};
use crate::world::World;
use crate::world::{ClipBlockShape, ClipFluid};

/// Vanilla `LongJump.REQUIRED_AIR_BLOCKS_ABOVE`.
const REQUIRED_AIR_BLOCKS_ABOVE: i32 = 4;
/// Vanilla `LongJump.JUMP_COOLDOWN_TICKS`.
const JUMP_COOLDOWN_TICKS: i64 = 10;
/// Vanilla `LongJump.JUMP_COOLDOWN_WHEN_HURT_TICKS`.
const JUMP_COOLDOWN_WHEN_HURT_TICKS: i64 = 2;
/// Vanilla `LongJump.INHALING_DURATION_TICKS`, `Math.round(10.0F)`.
const INHALING_DURATION_TICKS: i64 = 10;
/// Vanilla `LongJump.DEFAULT_MAX_JUMP_VELOCITY`.
const DEFAULT_MAX_JUMP_VELOCITY: f32 = 1.4;
/// Vanilla `LongJump.MAX_JUMP_VELOCITY_MULTIPLIER`.
const MAX_JUMP_VELOCITY_MULTIPLIER: f32 = 0.058_333_334;
/// Vanilla `LongJump.ALLOWED_ANGLES`.
const ALLOWED_ANGLES: [i32; 5] = [40, 55, 60, 75, 80];
/// Vanilla `LongJump` timeout, 200 ticks.
const TIMEOUT: i32 = 200;
/// Vanilla `LongJump` `BREEZE_SHOOT` memory refresh after landing.
const SHOOT_REMEMBER_TICKS: i64 = 100;
/// Vanilla `BreezeUtil.MAX_LINE_OF_SIGHT_TEST_RANGE`.
const MAX_LINE_OF_SIGHT_TEST_RANGE: f64 = 50.0;
/// Vanilla `BreezeUtil.randomPointBehindTarget` spread, in degrees.
const BEHIND_TARGET_SPREAD_DEGREES: f32 = 90.0;
/// Vanilla `BreezeUtil.randomPointBehindTarget` radius, `Mth.lerp(t, 4, 8)`.
const BEHIND_TARGET_MIN_RADIUS: f64 = 4.0;
/// Vanilla `BreezeUtil.randomPointBehindTarget` radius end.
const BEHIND_TARGET_MAX_RADIUS: f64 = 8.0;

/// Vanilla `LongJump extends Behavior<Breeze>`.
pub struct LongJump {
    state: BehaviorState,
}

impl LongJump {
    /// Vanilla `new LongJump()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
                    (&mem::BREEZE_JUMP_COOLDOWN, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_JUMP_INHALING, MemoryStatus::Registered),
                    (&mem::BREEZE_JUMP_TARGET, MemoryStatus::Registered),
                    (&mem::BREEZE_SHOOT, MemoryStatus::ValueAbsent),
                    (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_LEAVING_WATER, MemoryStatus::Registered),
                ],
                TIMEOUT,
                TIMEOUT,
            ),
        }
    }

    /// Vanilla `LongJump.canRun`: the body must be on the ground (or in shallow
    /// floatable fluid/lava is rejected), have a jump target or an in-range
    /// attack target, room above, and a safe surface to land on.
    fn can_run(ctx: &mut BehaviorContext<'_>) -> bool {
        let body = ctx.body;
        if !body.on_ground() && !body.is_in_water() {
            return false;
        }

        let in_deep_floatable_fluid = body.is_in_water()
            && body.fluid_contact().water_height() > body.get_fluid_jump_threshold();
        if in_deep_floatable_fluid || body.is_in_lava() {
            return false;
        }

        if ctx.has_memory_value(&mem::BREEZE_JUMP_TARGET) {
            return true;
        }

        let Some(attack_target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        if out_of_aggro_range(body, &attack_target) {
            ctx.erase_memory(&mem::ATTACK_TARGET);
            return false;
        }

        if too_close_for_jump(body, &attack_target) {
            return false;
        }

        if !can_jump_from_current_position(ctx.level, body) {
            return false;
        }

        let Some(world) = body.level() else {
            return false;
        };
        let raw = random_point_behind_target(&attack_target);
        let Some(target_pos) = snap_to_surface(world.as_ref(), raw) else {
            return false;
        };

        if is_block_dangerous(world.as_ref(), target_pos.below()) {
            return false;
        }

        let center = block_center(target_pos);
        if !has_line_of_sight(body, center)
            && !has_line_of_sight(
                body,
                block_center(target_pos.above_n(REQUIRED_AIR_BLOCKS_ABOVE)),
            )
        {
            return false;
        }

        ctx.set_memory(&mem::BREEZE_JUMP_TARGET, target_pos);
        true
    }
}

impl Default for LongJump {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for LongJump {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::can_run(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body.pose() != EntityPose::Standing && !ctx.has_memory_value(&mem::BREEZE_JUMP_COOLDOWN)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if !ctx.has_memory_value(&mem::BREEZE_JUMP_INHALING) {
            ctx.set_memory_with_expiry(&mem::BREEZE_JUMP_INHALING, (), INHALING_DURATION_TICKS);
        }

        ctx.body.set_pose(EntityPose::Inhaling);
        ctx.body
            .play_sound(&sound_events::ENTITY_BREEZE_CHARGE, 1.0, 1.0);
        if let Some(target_pos) = ctx.get_memory(&mem::BREEZE_JUMP_TARGET) {
            ctx.body
                .look_at(crate::entity::EntityAnchor::Eyes, block_center(target_pos));
        }
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let in_water = ctx.body.is_in_water();
        if !in_water && ctx.has_memory_value(&mem::BREEZE_LEAVING_WATER) {
            ctx.erase_memory(&mem::BREEZE_LEAVING_WATER);
        }

        if is_finished_inhaling(ctx) {
            let Some(body) = ctx.body.as_mob() else {
                return;
            };
            let Some(target_pos) = ctx.get_memory(&mem::BREEZE_JUMP_TARGET) else {
                ctx.body.set_pose(EntityPose::Standing);
                return;
            };

            let Some(velocity) = calculate_optimal_jump_vector(ctx.body, body, target_pos) else {
                ctx.body.set_pose(EntityPose::Standing);
                return;
            };

            if in_water {
                ctx.set_memory(&mem::BREEZE_LEAVING_WATER, ());
            }

            ctx.body
                .play_sound(&sound_events::ENTITY_BREEZE_JUMP, 1.0, 1.0);
            ctx.body.set_pose(EntityPose::LongJumping);
            let (yaw, pitch) = ctx.body.rotation();
            let body_yaw = ctx
                .body
                .as_living_entity()
                .map_or(yaw, LivingEntity::y_body_rot);
            ctx.body.set_rotation((body_yaw, pitch));
            if let Some(living) = ctx.body.as_living_entity() {
                living.set_discard_friction(true);
            }
            ctx.body.set_velocity(velocity);
        } else if is_finished_jumping(ctx) {
            ctx.body
                .play_sound(&sound_events::ENTITY_BREEZE_LAND, 1.0, 1.0);
            ctx.body.set_pose(EntityPose::Standing);
            if let Some(living) = ctx.body.as_living_entity() {
                living.set_discard_friction(false);
            }
            let was_hurt = ctx.has_memory_value(&mem::HURT_BY);
            ctx.set_memory_with_expiry(
                &mem::BREEZE_JUMP_COOLDOWN,
                (),
                if was_hurt {
                    JUMP_COOLDOWN_WHEN_HURT_TICKS
                } else {
                    JUMP_COOLDOWN_TICKS
                },
            );
            ctx.set_memory_with_expiry(&mem::BREEZE_SHOOT, (), SHOOT_REMEMBER_TICKS);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if matches!(
            ctx.body.pose(),
            EntityPose::LongJumping | EntityPose::Inhaling
        ) {
            ctx.body.set_pose(EntityPose::Standing);
        }

        ctx.erase_memory(&mem::BREEZE_JUMP_TARGET);
        ctx.erase_memory(&mem::BREEZE_JUMP_INHALING);
        ctx.erase_memory(&mem::BREEZE_LEAVING_WATER);
    }
}

/// Vanilla `LongJump.isFinishedInhaling`.
fn is_finished_inhaling(ctx: &BehaviorContext<'_>) -> bool {
    !ctx.has_memory_value(&mem::BREEZE_JUMP_INHALING) && ctx.body.pose() == EntityPose::Inhaling
}

/// Vanilla `LongJump.isFinishedJumping`.
fn is_finished_jumping(ctx: &BehaviorContext<'_>) -> bool {
    let is_jumping = ctx.body.pose() == EntityPose::LongJumping;
    let landed_on_ground = ctx.body.on_ground();
    let landed_in_water =
        ctx.body.is_in_water() && !ctx.has_memory_value(&mem::BREEZE_LEAVING_WATER);
    is_jumping && (landed_on_ground || landed_in_water)
}

/// Vanilla `LongJump.outOfAggroRange`.
fn out_of_aggro_range(body: &SharedEntity, target: &SharedEntity) -> bool {
    let follow_range = body
        .as_living_entity()
        .and_then(|living| {
            living
                .attributes()
                .lock()
                .get_value(steel_registry::vanilla_attributes::FOLLOW_RANGE)
        })
        .unwrap_or(f64::from(DEFAULT_MAX_JUMP_VELOCITY));
    body.distance_to_sqr(target.position()) > follow_range * follow_range
}

/// Vanilla `LongJump.tooCloseForJump`.
fn too_close_for_jump(body: &SharedEntity, target: &SharedEntity) -> bool {
    body.position().distance(target.position()) - 4.0 <= 0.0
}

/// Vanilla `LongJump.canJumpFromCurrentPosition`: not on a honey block, and four
/// air-or-water blocks of clearance above.
fn can_jump_from_current_position(world: &World, body: &SharedEntity) -> bool {
    let current_pos = body.block_position();
    if world.get_block_state(current_pos).get_block() == &vanilla_blocks::HONEY_BLOCK {
        return false;
    }

    for i in 1..=REQUIRED_AIR_BLOCKS_ABOVE {
        let offset_pos = current_pos.offset(0, i, 0);
        let state = world.get_block_state(offset_pos);
        if !state.is_air() && state.get_block() != &vanilla_blocks::WATER {
            return false;
        }
    }

    true
}

/// Vanilla `LongJump.calculateOptimalJumpVector`: shuffle the allowed angles and
/// take the first one that lands on the target, adding the jump-boost bonus when
/// the body has it.
fn calculate_optimal_jump_vector(
    entity: &SharedEntity,
    _mob: &dyn crate::entity::Mob,
    target_pos: BlockPos,
) -> Option<DVec3> {
    let follow_range = entity
        .as_living_entity()
        .and_then(|living| {
            living
                .attributes()
                .lock()
                .get_value(steel_registry::vanilla_attributes::FOLLOW_RANGE)
        })
        .unwrap_or(0.0);
    let max_jump_velocity = MAX_JUMP_VELOCITY_MULTIPLIER * follow_range as f32;

    let mut angles = ALLOWED_ANGLES;
    shuffle(&mut angles);
    let target = block_center(target_pos);
    for angle in angles {
        if let Some(vector) =
            calculate_jump_vector_for_angle(entity, target, max_jump_velocity, angle, false)
        {
            if let Some(jump_boost) = jump_boost_power(entity) {
                let bonus = vector.normalize().y * jump_boost;
                return Some(vector + DVec3::new(0.0, bonus, 0.0));
            }
            return Some(vector);
        }
    }

    None
}

/// Vanilla `LongJump.calculateOptimalJumpVector`'s `MobEffects.JUMP_BOOST`
/// branch, via `LivingEntity.getJumpBoostPower`.
fn jump_boost_power(entity: &SharedEntity) -> Option<f64> {
    let living = entity.as_living_entity()?;
    let effect = living.mob_effect(steel_registry::vanilla_mob_effects::JUMP_BOOST)?;
    Some(0.1 * f64::from(effect.amplifier() + 1))
}

/// Vanilla `Util.shuffledCopy`: Fisher-Yates over the static angle list.
fn shuffle(values: &mut [i32; 5]) {
    for i in (1..values.len()).rev() {
        let j = rand::random_range(0..=i);
        values.swap(i, j);
    }
}

/// Vanilla `Vec3.atCenterOf(BlockPos)`.
pub(super) fn block_center(pos: BlockPos) -> DVec3 {
    DVec3::new(
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    )
}

/// Vanilla `BreezeUtil.randomPointBehindTarget`.
pub(super) fn random_point_behind_target(target: &SharedEntity) -> DVec3 {
    let head_yaw = target
        .as_living_entity()
        .map_or(0.0, crate::entity::LivingEntity::y_head_rot);
    let spread = BEHIND_TARGET_SPREAD_DEGREES / 2.0;
    let view_angle = head_yaw + 180.0 + (rand::random::<f32>() * 2.0 - 1.0) * spread;
    let radius = BEHIND_TARGET_MIN_RADIUS
        + rand::random::<f64>() * (BEHIND_TARGET_MAX_RADIUS - BEHIND_TARGET_MIN_RADIUS);
    let yaw = f64::from(view_angle).to_radians();
    target.position() + DVec3::new(-yaw.sin(), 0.0, yaw.cos()) * radius
}

/// Vanilla `BreezeUtil.hasLineOfSight`: a collider-only clip from the breeze.
pub(super) fn has_line_of_sight(body: &SharedEntity, target: DVec3) -> bool {
    let from = body.position();
    if target.distance(from) > MAX_LINE_OF_SIGHT_TEST_RANGE {
        return false;
    }

    let Some(world) = body.level() else {
        return false;
    };

    world
        .clip(from, target, ClipBlockShape::Collider, ClipFluid::None)
        .is_miss()
}

/// Vanilla `BreezeUtil.snapToSurface`: the collider below `target` within 10
/// blocks, else the one above.
pub(super) fn snap_to_surface(world: &World, target: DVec3) -> Option<BlockPos> {
    let below = world.clip(
        target,
        target + DVec3::new(0.0, -10.0, 0.0),
        ClipBlockShape::Collider,
        ClipFluid::None,
    );
    if !below.is_miss() {
        return Some(below.block_pos.above());
    }

    let above = world.clip(
        target,
        target + DVec3::new(0.0, 10.0, 0.0),
        ClipBlockShape::Collider,
        ClipFluid::None,
    );
    if !above.is_miss() {
        return Some(above.block_pos.above());
    }

    None
}

/// Vanilla `Breeze.getType().isBlockDangerous(BlockState)` for the blocks a
/// breeze refuses to leap onto.
pub(super) fn is_block_dangerous(world: &World, pos: BlockPos) -> bool {
    let block = world.get_block_state(pos).get_block();
    block == &vanilla_blocks::FIRE
        || block == &vanilla_blocks::SOUL_FIRE
        || block == &vanilla_blocks::LAVA
        || block == &vanilla_blocks::MAGMA_BLOCK
        || block == &vanilla_blocks::CAMPFIRE
        || block == &vanilla_blocks::SOUL_CAMPFIRE
        || block == &vanilla_blocks::SWEET_BERRY_BUSH
        || block == &vanilla_blocks::WITHER_ROSE
        || block == &vanilla_blocks::POWDER_SNOW
}
