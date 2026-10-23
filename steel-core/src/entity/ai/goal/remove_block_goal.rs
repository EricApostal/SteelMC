//! Vanilla `RemoveBlockGoal` (`goal/RemoveBlockGoal.java`): a zombie chews through a turtle
//! egg (vanilla's only user), nudging itself up and down while it eats.
//!
//! Vanilla extends `MoveToBlockGoal`; Steel embeds the landed [`MoveToBlockGoal`] and drives
//! it through the module-private hooks, exactly as `ZombieAttackGoal` embeds `MeleeAttackGoal`.

use std::ptr;

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::particle_type::{ItemParticleOption, ParticleData};
use steel_registry::{ItemStackTemplate, vanilla_items, vanilla_particle_types};
use steel_utils::BlockPos;

use super::move_to_block::MoveToBlockGoal;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{PathfinderMob, SharedEntity};
use crate::world::LevelReader;

/// Vanilla `RemoveBlockGoal.WAIT_AFTER_BLOCK_FOUND`.
const WAIT_AFTER_BLOCK_FOUND: i32 = 20;
/// Vanilla `RemoveBlockGoal(mob, speedModifier, 24, verticalSearchRange)`: search range 24.
const SEARCH_RANGE: i32 = 24;
/// Vanilla `RemoveBlockGoal.tick` vertical nudge while eating.
const EAT_BOUNCE_VELOCITY: f64 = 0.3;
/// Vanilla `RemoveBlockGoal.tick` chew ticks before the block is removed.
const CHEW_TICKS: i32 = 60;
/// Vanilla `RemoveBlockGoal.tick` egg-particle count per chew tick.
const EAT_PARTICLE_COUNT: i32 = 3;
/// Vanilla `RemoveBlockGoal.tick` egg-particle offset scale (`(nextFloat() - 0.5) * 0.08`).
const EAT_PARTICLE_SPREAD: f32 = 0.08;
/// Vanilla `RemoveBlockGoal.tick` puff particle count.
const BREAK_PARTICLE_COUNT: i32 = 20;
/// Vanilla `RemoveBlockGoal.tick` puff particle spread (`nextGaussian() * 0.02`).
const BREAK_PARTICLE_SPREAD: f64 = 0.02;
/// Vanilla `RemoveBlockGoal.tick` particle speed for both bursts.
const PARTICLE_SPEED: f64 = 0.15;

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

/// Vanilla `RemoveBlockGoal`.
pub struct RemoveBlockGoal {
    move_to_block: MoveToBlockGoal,
    block_to_remove: BlockRef,
    ticks_since_reached_goal: i32,
}

impl RemoveBlockGoal {
    /// Vanilla `new RemoveBlockGoal(blockToRemove, mob, speedModifier, verticalSearchRange)`.
    #[must_use]
    pub fn new(block_to_remove: BlockRef, speed_modifier: f64, vertical_search_range: i32) -> Self {
        Self {
            move_to_block: MoveToBlockGoal::with_vertical_search_range(
                speed_modifier,
                SEARCH_RANGE,
                vertical_search_range,
                move |level, pos| is_valid_target(level, pos, block_to_remove),
            ),
            block_to_remove,
            ticks_since_reached_goal: 0,
        }
    }

    /// Vanilla `RemoveBlockGoal.getPosWithBlock`: the mob's own block, then its neighbors.
    fn pos_with_block(&self, pos: BlockPos, level: &dyn LevelReader) -> Option<BlockPos> {
        let matches =
            |pos: BlockPos| ptr::eq(level.get_block_state(pos).get_block(), self.block_to_remove);
        if matches(pos) {
            return Some(pos);
        }

        let neighbors = [
            pos.below(),
            pos.offset(-1, 0, 0),
            pos.offset(1, 0, 0),
            pos.offset(0, 0, -1),
            pos.offset(0, 0, 1),
            pos.below().below(),
        ];
        neighbors.into_iter().find(|neighbour| matches(*neighbour))
    }
}

/// Vanilla `RemoveBlockGoal.isValidTarget`.
fn is_valid_target(level: &dyn LevelReader, pos: BlockPos, block_to_remove: BlockRef) -> bool {
    ptr::eq(level.get_block_state(pos).get_block(), block_to_remove)
        && level.get_block_state(pos.above()).is_air()
        && level.get_block_state(pos.above_n(2)).is_air()
}

impl Goal for RemoveBlockGoal {
    fn controls(&self) -> GoalControls {
        self.move_to_block.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if !world.get_game_rule(&steel_registry::vanilla_game_rules::MOB_GRIEFING) {
            return false;
        }

        if self.move_to_block.next_start_tick() > 0 {
            self.move_to_block
                .set_next_start_tick(self.move_to_block.next_start_tick() - 1);
            return false;
        }

        if self
            .move_to_block
            .find_nearest_block_public(mob, world.as_ref())
        {
            self.move_to_block
                .set_next_start_tick(reduced_tick_delay(WAIT_AFTER_BLOCK_FOUND));
            return true;
        }

        self.move_to_block
            .set_next_start_tick(MoveToBlockGoal::default_next_start_tick(mob));
        false
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.move_to_block.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.move_to_block.start(mob);
        self.ticks_since_reached_goal = 0;
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.move_to_block.stop(mob);
        mob.set_fall_distance(1.0);
    }

    fn requires_update_every_tick(&self) -> bool {
        self.move_to_block.requires_update_every_tick()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.move_to_block.tick(mob, entity);

        let Some(world) = mob.level() else {
            return;
        };
        let Some(eat_pos) = self.pos_with_block(mob.block_position(), world.as_ref()) else {
            return;
        };
        if !self.move_to_block.is_reached_target() {
            return;
        }

        if self.ticks_since_reached_goal > 0 {
            let velocity = mob.velocity();
            mob.set_velocity(glam::DVec3::new(
                velocity.x,
                EAT_BOUNCE_VELOCITY,
                velocity.z,
            ));
            // Vanilla `ItemParticleOption(ParticleTypes.ITEM, Items.EGG)` chew spray.
            world.send_particles(
                ParticleData::new(
                    &vanilla_particle_types::ITEM,
                    ItemParticleOption::new(ItemStackTemplate::new(&vanilla_items::EGG)),
                ),
                glam::DVec3::new(
                    f64::from(eat_pos.x()) + 0.5,
                    f64::from(eat_pos.y()) + 0.7,
                    f64::from(eat_pos.z()) + 0.5,
                ),
                EAT_PARTICLE_COUNT,
                glam::DVec3::new(
                    f64::from((rand::random::<f32>() - 0.5) * EAT_PARTICLE_SPREAD),
                    f64::from((rand::random::<f32>() - 0.5) * EAT_PARTICLE_SPREAD),
                    f64::from((rand::random::<f32>() - 0.5) * EAT_PARTICLE_SPREAD),
                ),
                PARTICLE_SPEED,
            );
        }

        if self.ticks_since_reached_goal % 2 == 0 {
            let velocity = mob.velocity();
            mob.set_velocity(glam::DVec3::new(
                velocity.x,
                -EAT_BOUNCE_VELOCITY,
                velocity.z,
            ));
        }

        if self.ticks_since_reached_goal > CHEW_TICKS {
            world.remove_block(eat_pos, false);
            // Vanilla sends one `POOF` particle per gaussian offset, 20 times.
            for _ in 0..BREAK_PARTICLE_COUNT {
                world.send_particles(
                    ParticleData::simple(&vanilla_particle_types::POOF),
                    glam::DVec3::new(
                        f64::from(eat_pos.x()) + 0.5,
                        f64::from(eat_pos.y()),
                        f64::from(eat_pos.z()) + 0.5,
                    ),
                    1,
                    glam::DVec3::new(
                        gaussian() * BREAK_PARTICLE_SPREAD,
                        gaussian() * BREAK_PARTICLE_SPREAD,
                        gaussian() * BREAK_PARTICLE_SPREAD,
                    ),
                    PARTICLE_SPEED,
                );
            }
        }

        self.ticks_since_reached_goal += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remove_block_goal_uses_the_move_to_block_controls() {
        let goal = RemoveBlockGoal::new(&steel_registry::vanilla_blocks::TURTLE_EGG, 1.0, 1);

        assert_eq!(goal.controls(), GoalControls::MOVE | GoalControls::JUMP);
    }
}
