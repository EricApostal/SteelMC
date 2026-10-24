//! Vanilla `AnimalPanic`: panic-flee toward a random land position.

use glam::DVec3;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_damage_type_tags;
use steel_utils::{BlockPos, Direction, Identifier};

use super::random_pos::land_random_pos;
use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::entity::PathfinderMob;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::WalkTarget;
use crate::fluid::FluidStateExt as _;

/// Vanilla `AnimalPanic.PANIC_MIN_DURATION`.
const PANIC_MIN_DURATION: i32 = 100;
/// Vanilla `AnimalPanic.PANIC_MAX_DURATION`.
const PANIC_MAX_DURATION: i32 = 120;
/// Vanilla `AnimalPanic.PANIC_DISTANCE_HORIZONTAL`.
const PANIC_DISTANCE_HORIZONTAL: i32 = 5;
/// Vanilla `AnimalPanic.PANIC_DISTANCE_VERTICAL`.
const PANIC_DISTANCE_VERTICAL: i32 = 4;
/// Vanilla `AnimalPanic.lookForWater`'s vertical search radius.
const WATER_CHECK_DISTANCE_VERTICAL: i32 = 1;
/// Vanilla `AnimalPanic.lookForWater`'s `Mth.ceil(mob.getBbWidth()) == 2` branch.
const WIDE_MOB_BB_WIDTH: f64 = 2.0;

/// Vanilla `AnimalPanic<E extends PathfinderMob>`: sets `IS_PANICKING` for 100-120
/// ticks and keeps re-targeting a random land position (or nearby water while on
/// fire) until the panic burst ends.
///
/// Vanilla's third and fourth constructors take a `Function<PathfinderMob,
/// TagKey<DamageType>>` position getter; Steel keeps the default `LandRandomPos`
/// getter and exposes the damage-tag choice through [`AnimalPanic::with_damage_tag`].
pub struct AnimalPanic {
    state: BehaviorState,
    speed_multiplier: f32,
    panic_causing_damage_types: &'static Identifier,
}

impl AnimalPanic {
    /// Vanilla `new AnimalPanic(float speedMultiplier)`, panicking on
    /// `DamageTypeTags.PANIC_CAUSES`.
    #[must_use]
    pub fn new(speed_multiplier: f32) -> Self {
        const PANIC_CAUSES: &steel_utils::Identifier =
            &vanilla_damage_type_tags::DamageTypeTag::PANIC_CAUSES;
        Self::with_damage_tag(speed_multiplier, PANIC_CAUSES)
    }

    /// Vanilla `new AnimalPanic(float speedMultiplier, Function<PathfinderMob,
    /// TagKey<DamageType>> panicCausingDamageTypes)`.
    #[must_use]
    pub fn with_damage_tag(
        speed_multiplier: f32,
        panic_causing_damage_types: &'static Identifier,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::IS_PANICKING, MemoryStatus::Registered),
                    (&mem::HURT_BY, MemoryStatus::Registered),
                ],
                PANIC_MIN_DURATION,
                PANIC_MAX_DURATION,
            ),
            speed_multiplier,
            panic_causing_damage_types,
        }
    }

    /// Vanilla `AnimalPanic.getPanicPos`.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror the vanilla `AnimalPanic` instance call site"
    )]
    fn get_panic_pos(&self, ctx: &BehaviorContext<'_>) -> Option<DVec3> {
        let body = ctx.body;
        let mob = body.as_pathfinder_mob()?;

        if body.is_on_fire()
            && let Some(water_pos) = look_for_water(ctx, mob, PANIC_DISTANCE_HORIZONTAL)
        {
            let (x, y, z) = water_pos.get_bottom_center();
            return Some(DVec3::new(x, y, z));
        }

        land_random_pos(mob, PANIC_DISTANCE_HORIZONTAL, PANIC_DISTANCE_VERTICAL)
    }
}

/// Vanilla `AnimalPanic.lookForWater`: the closest water block within 5 blocks
/// horizontally and 1 vertically, skipping the search when the mob's own block is
/// solid. Mobs two blocks wide need their south/east/south-east neighbors wet too.
fn look_for_water(
    ctx: &BehaviorContext<'_>,
    mob: &dyn PathfinderMob,
    xz_dist: i32,
) -> Option<BlockPos> {
    let world = ctx.level;
    let mob_position = mob.block_position();
    let block_state = world.get_block_state(mob_position);
    let behavior = BLOCK_BEHAVIORS.get_behavior(block_state.get_block());
    if !behavior
        .get_collision_shape(
            block_state,
            world,
            mob_position,
            BlockCollisionContext::empty(),
        )
        .is_empty()
    {
        return None;
    }

    let is_water = |pos: BlockPos| world.get_block_state(pos).get_fluid_state().is_water();
    if mob.bounding_box().width().ceil() == WIDE_MOB_BB_WIDTH {
        mob_position.find_closest_match(xz_dist, WATER_CHECK_DISTANCE_VERTICAL, |pos| {
            is_water(Direction::South.relative(pos))
                && is_water(Direction::East.relative(pos))
                && is_water(Direction::East.relative(Direction::South.relative(pos)))
        })
    } else {
        mob_position.find_closest_match(xz_dist, WATER_CHECK_DISTANCE_VERTICAL, is_water)
    }
}

impl Behavior for AnimalPanic {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::HURT_BY)
            .is_some_and(|source| source.is(self.panic_causing_damage_types))
            || ctx.has_memory_value(&mem::IS_PANICKING)
    }

    fn can_still_use(&self, _ctx: &mut BehaviorContext<'_>) -> bool {
        true
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.set_memory(&mem::IS_PANICKING, true);
        ctx.erase_memory(&mem::WALK_TARGET);
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::IS_PANICKING);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(mob) = ctx.body.as_pathfinder_mob() else {
            return;
        };
        if !mob.mob_base().navigation().lock().is_done() {
            return;
        }

        let Some(panic_to_pos) = self.get_panic_pos(ctx) else {
            return;
        };
        ctx.set_memory(
            &mem::WALK_TARGET,
            WalkTarget::from_vec(panic_to_pos, self.speed_multiplier, 0),
        );
    }
}
