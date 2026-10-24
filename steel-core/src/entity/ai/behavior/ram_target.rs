//! Vanilla `RamTarget`: charge along the remembered `RAM_TARGET` and hit whatever
//! is in the way.

use glam::DVec3;
use steel_registry::registry::TaggedRegistryExt as _;
use steel_utils::BlockPos;
use steel_utils::entity_events::EntityStatus;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::value_providers::UniformIntProvider;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::WalkTarget;
use crate::entity::ai::targeting::TargetingConditions;
use crate::entity::living_entity::LivingEntity;

/// Vanilla `RamTarget.TIME_OUT_DURATION`.
pub const TIME_OUT_DURATION: i32 = 200;
/// Vanilla `RamTarget.RAM_SPEED_FORCE_FACTOR`.
pub const RAM_SPEED_FORCE_FACTOR: f32 = 1.65;
/// Vanilla `Mth.clamp` bounds on the ram speed factor.
const RAM_SPEED_FACTOR_MIN: f32 = 0.2;
const RAM_SPEED_FACTOR_MAX: f32 = 3.0;

/// Vanilla `RamTarget<Goat>`.
///
/// Steel's goat-specific hooks (`dropHorn`, horn-break sound) are not ported, so
/// the horn-breaking-block branch reports `false`; the charge, hit and cooldown
/// half is complete.
pub struct RamTarget {
    state: BehaviorState,
    time_between_rams: UniformIntProvider,
    ram_targeting: TargetingConditions,
    speed: f32,
    knockback_force: f64,
    ram_direction: DVec3,
}

impl RamTarget {
    /// Vanilla `RamTarget(getTimeBetweenRams, ramTargeting, speed, getKnockbackForce,
    /// getImpactSound, getHornBreakSound)`.
    #[must_use]
    pub fn new(
        time_between_rams: UniformIntProvider,
        ram_targeting: TargetingConditions,
        speed: f32,
        knockback_force: f64,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::RAM_COOLDOWN_TICKS as MemoryModuleTypeRef,
                        MemoryStatus::ValueAbsent,
                    ),
                    (
                        &mem::RAM_TARGET as MemoryModuleTypeRef,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                TIME_OUT_DURATION,
                TIME_OUT_DURATION,
            ),
            time_between_rams,
            ram_targeting,
            speed,
            knockback_force,
            ram_direction: DVec3::ZERO,
        }
    }

    fn sample_time_between_rams(&self) -> i32 {
        let mut random = LegacyRandom::from_seed(rand::random());
        self.time_between_rams.sample(&mut random)
    }

    /// Vanilla `finishRam`.
    fn finish_ram(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.body.broadcast_entity_event(EntityStatus::EndRam);
        ctx.set_memory(&mem::RAM_COOLDOWN_TICKS, self.sample_time_between_rams());
        ctx.erase_memory(&mem::RAM_TARGET);
    }

    /// Vanilla `hasRammedHornBreakingBlock`.
    ///
    /// The block tag test is ported; `Goat.dropHorn` is goat-specific and is not
    /// available on Steel's generic mob surface, so no horn can be dropped here.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `Goat.hasRammedHornBreakingBlock`"
    )]
    fn has_rammed_horn_breaking_block(&self, ctx: &BehaviorContext<'_>) -> bool {
        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        use steel_registry::vanilla_block_tags::BlockTag;

        let horizontal_movement = {
            let velocity = ctx.body.velocity();
            DVec3::new(velocity.x, 0.0, velocity.z).normalize_or_zero()
        };
        let facing = ctx.body.position() + horizontal_movement;
        let facing_pos = BlockPos::containing(facing.x, facing.y, facing.z);

        let is_snappable = |pos: BlockPos| {
            steel_registry::REGISTRY.blocks.is_in_tag(
                ctx.level.get_block_state(pos).get_block(),
                &BlockTag::SNAPS_GOAT_HORN,
            )
        };
        is_snappable(facing_pos) || is_snappable(facing_pos.above())
    }
}

impl Behavior for RamTarget {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::RAM_TARGET)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::RAM_TARGET)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let cur_pos = ctx.body.block_position();
        let Some(ram_target_pos) = ctx.get_memory(&mem::RAM_TARGET) else {
            return;
        };

        self.ram_direction = DVec3::new(
            f64::from(cur_pos.x()) - ram_target_pos.x,
            0.0,
            f64::from(cur_pos.z()) - ram_target_pos.z,
        )
        .normalize_or_zero();
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::from_vec(ram_target_pos, self.speed, 0),
        );
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let body_box = ctx.body.bounding_box();
        let body = ctx.body;
        let ram_targeting = &self.ram_targeting;
        let level = ctx.level;
        let nearby_entities = level.get_entities_in_aabb_matching(&body_box, |entity| {
            entity
                .as_living_entity()
                .is_some_and(|living| ram_targeting.test(level, body.as_living_entity(), living))
        });

        if let Some(ram_target) = nearby_entities.first() {
            if let Some(mob) = ctx.body.as_mob() {
                let _ = mob.do_hurt_target(ctx.body, ctx.level, ram_target);
            }

            let speed_boost_power = 0.0;
            let speed_factor = ((ctx
                .body
                .as_living_entity()
                .map_or(1.0, LivingEntity::get_speed)
                * RAM_SPEED_FORCE_FACTOR)
                .clamp(RAM_SPEED_FACTOR_MIN, RAM_SPEED_FACTOR_MAX))
                + speed_boost_power;
            if let Some(living) = ram_target.as_living_entity() {
                living.knockback(
                    f64::from(speed_factor) * self.knockback_force,
                    self.ram_direction.x,
                    self.ram_direction.z,
                );
            }

            self.finish_ram(ctx);
            return;
        }

        if self.has_rammed_horn_breaking_block(ctx) {
            self.finish_ram(ctx);
            return;
        }

        let walk_target = ctx.get_memory(&mem::WALK_TARGET);
        let ram_target = ctx.get_memory(&mem::RAM_TARGET);
        let lost_or_reached_target = walk_target.is_none()
            || ram_target.is_none()
            || walk_target.as_ref().is_some_and(|walk_target| {
                walk_target
                    .target()
                    .current_position()
                    .distance(ram_target.unwrap_or_default())
                    < 0.25
            });
        if lost_or_reached_target {
            self.finish_ram(ctx);
        }
    }
}
