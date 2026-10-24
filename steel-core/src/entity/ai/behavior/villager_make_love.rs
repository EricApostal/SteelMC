//! Vanilla `VillagerMakeLove`.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use steel_registry::vanilla_entities;
use steel_registry::vanilla_poi_types;
use steel_utils::entity_events::EntityStatus;

use super::{poi, utils, villager};
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::{AgeableMob as _, Entity as _};
use steel_utils::Downcast as _;

/// Vanilla `VillagerMakeLove` duration, `super(..., 350, 350)`.
const DURATION: i32 = 350;
/// Vanilla `VillagerMakeLove.start` birth delay base, `275 + random.nextInt(50)`.
const BIRTH_DELAY_MIN_TICKS: i32 = 275;
/// Vanilla `VillagerMakeLove.start` birth delay jitter, `random.nextInt(50)`.
const BIRTH_DELAY_JITTER_TICKS: i32 = 50;
/// Vanilla `VillagerMakeLove.tick` heart chance, `random.nextInt(35) == 0`.
const LOVE_HEARTS_CHANCE: i32 = 35;
/// Vanilla `VillagerMakeLove.tick` interaction reach, `distanceToSqr(target) > 5.0`.
const BREED_DISTANCE_SQR: f64 = 5.0;
/// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther(..., 0.5F, 2)`.
const INTERACT_SPEED_MODIFIER: f32 = 0.5;
/// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther(..., 2)` close-enough distance.
const WALK_CLOSE_ENOUGH: i32 = 2;
/// Vanilla `VillagerMakeLove.takeVacantBed` search radius.
const VACANT_BED_SEARCH_RANGE: i32 = 48;
/// Vanilla `Villager.breed` parent age, `source.setAge(6000)`.
const ADULT_AGE: i32 = 6000;
/// Vanilla `Villager.breed` child age, `child.setAge(-24000)`.
const BABY_AGE: i32 = -24000;

/// Vanilla `VillagerMakeLove`: two adult villagers breed at a vacant bed.
pub struct VillagerMakeLove {
    state: BehaviorState,
    birth_timestamp: AtomicI64,
}

impl VillagerMakeLove {
    /// Vanilla `VillagerMakeLove()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::BREED_TARGET, MemoryStatus::ValuePresent),
                    (
                        &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                DURATION,
                DURATION,
            ),
            birth_timestamp: AtomicI64::new(0),
        }
    }

    /// Vanilla `VillagerMakeLove.tryToGiveBirth`.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `VillagerMakeLove.tryToGiveBirth`"
    )]
    fn try_to_give_birth(
        &self,
        ctx: &mut BehaviorContext<'_>,
        body: &VillagerEntity,
        target: &VillagerEntity,
    ) {
        let level = ctx.level;
        let center = ctx.body.block_position();
        // Vanilla `PoiType.validRange`, the bed's own search distance.
        let valid_range = vanilla_poi_types::HOME.search_distance as i32;

        let bed = poi::take(
            level,
            poi::PoiPredicate::Type(&vanilla_poi_types::HOME),
            |pos| utils::can_reach(ctx, pos, valid_range).is_some_and(|path| path.can_reach()),
            center,
            VACANT_BED_SEARCH_RANGE,
        );

        let Some(bed) = bed else {
            target.broadcast_entity_event(EntityStatus::VillagerAngry);
            ctx.body.broadcast_entity_event(EntityStatus::VillagerAngry);
            return;
        };

        let Some(world) = ctx.body.level() else {
            poi::release(level, bed);
            return;
        };

        let Some(child) = body.get_breed_offspring_for(&world, target) else {
            poi::release(level, bed);
            return;
        };

        body.set_age(ADULT_AGE);
        target.set_age(ADULT_AGE);
        if let Some(child_villager) = child.downcast_ref::<VillagerEntity>() {
            child_villager.set_age(BABY_AGE);
        }
        child.snap_to(body.position(), 0.0, 0.0);

        if let Err(error) = world.try_add_entity(Arc::clone(&child)) {
            log::warn!("Failed to add the villager child to the world: {error}");
        }

        child.broadcast_entity_event(EntityStatus::LoveHearts);

        // Vanilla `VillagerMakeLove.giveBedToChild` writes the bed into the child's
        // HOME memory. `VillagerEntity` keeps its brain private, so a behavior
        // cannot reach a child brain: the reserved bed ticket is left taken
        // instead of being transferred to the child (gap).
    }
}

/// Vanilla `BehaviorUtils.targetIsValid(brain, BREED_TARGET, EntityTypes.VILLAGER)`.
fn breeding_target_is_valid(ctx: &BehaviorContext<'_>) -> bool {
    ctx.get_memory(&mem::BREED_TARGET)
        .filter(|entity| entity.entity_type() == &vanilla_entities::VILLAGER)
        .filter(|entity| entity.is_alive())
        .is_some_and(|entity| utils::entity_is_visible(ctx, &entity))
}

/// Vanilla `VillagerMakeLove.isBreedingPossible`.
fn is_breeding_possible(ctx: &BehaviorContext<'_>) -> bool {
    let Some(target) = ctx.get_memory(&mem::BREED_TARGET) else {
        return false;
    };

    if target.entity_type() != &vanilla_entities::VILLAGER {
        return false;
    }

    let body = ctx.body;
    let (Some(body_villager), Some(target_villager)) =
        (villager::as_villager(body), villager::as_villager(&target))
    else {
        return false;
    };

    breeding_target_is_valid(ctx)
        && body_villager.can_breed_food()
        && target_villager.can_breed_food()
}

impl Default for VillagerMakeLove {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for VillagerMakeLove {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        is_breeding_possible(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.timestamp <= self.birth_timestamp.load(Ordering::Relaxed) && is_breeding_possible(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::BREED_TARGET) else {
            return;
        };

        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &target,
            INTERACT_SPEED_MODIFIER,
            WALK_CLOSE_ENOUGH,
        );

        target.broadcast_entity_event(EntityStatus::InLoveHearts);
        ctx.body.broadcast_entity_event(EntityStatus::InLoveHearts);

        let duration = BIRTH_DELAY_MIN_TICKS + rand::random_range(0..BIRTH_DELAY_JITTER_TICKS);
        self.birth_timestamp
            .store(ctx.timestamp + i64::from(duration), Ordering::Relaxed);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::BREED_TARGET) else {
            return;
        };

        if ctx.body.position().distance_squared(target.position()) > BREED_DISTANCE_SQR {
            return;
        }

        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &target,
            INTERACT_SPEED_MODIFIER,
            WALK_CLOSE_ENOUGH,
        );

        if ctx.timestamp >= self.birth_timestamp.load(Ordering::Relaxed) {
            let body = ctx.body;
            let (Some(body_villager), Some(target_villager)) =
                (villager::as_villager(body), villager::as_villager(&target))
            else {
                return;
            };

            body_villager.eat_and_digest_food();
            target_villager.eat_and_digest_food();
            self.try_to_give_birth(ctx, body_villager, target_villager);
        } else if rand::random_range(0..LOVE_HEARTS_CHANCE) == 0 {
            target.broadcast_entity_event(EntityStatus::LoveHearts);
            ctx.body.broadcast_entity_event(EntityStatus::LoveHearts);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::BREED_TARGET);
    }
}
