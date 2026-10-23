//! Vanilla `AnimalMakeLove`: two animals of the same type walk together and breed.

use std::sync::atomic::{AtomicI64, Ordering};

use steel_registry::entity_type::EntityTypeRef;

use super::utils;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::{Mob, SharedEntity};

/// Vanilla `AnimalMakeLove.BREED_RANGE`.
const BREED_RANGE: f64 = 3.0;
/// Vanilla `AnimalMakeLove` behavior duration, `super(entryConditions, 110)`.
const DURATION: i32 = 110;
/// Vanilla `AnimalMakeLove.MIN_DURATION`, the `60 + random.nextInt(50)` base.
const MIN_DURATION: i32 = 60;
/// Vanilla `AnimalMakeLove.MAX_DURATION` jitter, `random.nextInt(50)`.
const MAX_DURATION: i32 = 50;
/// Vanilla `AnimalMakeLove.DEFAULT_CLOSE_ENOUGH_DISTANCE`.
const DEFAULT_CLOSE_ENOUGH_DISTANCE: i32 = 2;
/// Vanilla `AnimalMakeLove` default speed modifier, `1.0F`.
const DEFAULT_SPEED_MODIFIER: f32 = 1.0;

/// Vanilla `AnimalMakeLove extends Behavior<Animal>`: finds a visible, in-love
/// partner of the configured type, walks to it, and spawns a child three blocks
/// away once the breed delay elapses.
pub struct AnimalMakeLove {
    state: BehaviorState,
    partner_type: EntityTypeRef,
    speed_modifier: f32,
    close_enough_distance: i32,
    spawn_child_at_time: AtomicI64,
}

impl AnimalMakeLove {
    /// Vanilla `new AnimalMakeLove(EntityType partnerType)`, speed `1.0F` and a
    /// close-enough distance of 2.
    #[must_use]
    pub fn new(partner_type: EntityTypeRef) -> Self {
        Self::with_speed(
            partner_type,
            DEFAULT_SPEED_MODIFIER,
            DEFAULT_CLOSE_ENOUGH_DISTANCE,
        )
    }

    /// Vanilla `new AnimalMakeLove(EntityType partnerType, float speedModifier,
    /// int closeEnoughDistance)`.
    #[must_use]
    pub fn with_speed(
        partner_type: EntityTypeRef,
        speed_modifier: f32,
        close_enough_distance: i32,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (
                        &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
                        MemoryStatus::ValuePresent,
                    ),
                    (&mem::BREED_TARGET, MemoryStatus::ValueAbsent),
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                    (&mem::IS_PANICKING, MemoryStatus::ValueAbsent),
                ],
                DURATION,
                DURATION,
            ),
            partner_type,
            speed_modifier,
            close_enough_distance,
            spawn_child_at_time: AtomicI64::new(0),
        }
    }

    /// Vanilla `AnimalMakeLove.hasBreedTargetOfRightType`.
    fn has_breed_target_of_right_type(&self, ctx: &BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::BREED_TARGET)
            .is_some_and(|target| target.entity_type() == self.partner_type)
    }

    /// Vanilla `AnimalMakeLove.findValidBreedPartner`.
    fn find_valid_breed_partner(&self, ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
        let body = ctx.body;
        let body_animal = body.as_animal()?;
        let partner_type = self.partner_type;

        utils::find_visible_entity(ctx, |entity| {
            entity.entity_type() == partner_type
                && entity.as_animal().is_some_and(|animal| {
                    body_animal.can_mate(animal) && !entity_is_panicking(entity)
                })
        })
    }

    /// Vanilla `AnimalMakeLove.start`'s `partner.getBrain().setMemory(...)`:
    /// Steel brains are reachable through `Mob::brain`, but never through the
    /// body's own lock (the body brain is already borrowed for this tick).
    fn set_partner_breed_target(&self, partner: &SharedEntity, body: &SharedEntity) {
        if let Some(brain) = partner.as_mob().and_then(Mob::brain) {
            brain
                .lock()
                .set_memory(&mem::BREED_TARGET, Some(body.clone()));
        }
    }
}

/// Vanilla `Animal.isPanicking`. For any entity other than the currently ticking
/// body this reads the `IS_PANICKING` memory directly.
fn entity_is_panicking(entity: &SharedEntity) -> bool {
    entity
        .as_mob()
        .and_then(Mob::brain)
        .is_some_and(|brain| brain.lock().has_memory_value(&mem::IS_PANICKING))
}

impl Behavior for AnimalMakeLove {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.body
            .as_animal()
            .is_some_and(|animal| animal.is_in_love())
            && self.find_valid_breed_partner(ctx).is_some()
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !self.has_breed_target_of_right_type(ctx) {
            return false;
        }

        let Some(partner) = ctx.get_memory(&mem::BREED_TARGET) else {
            return false;
        };
        let body = ctx.body;
        let (Some(body_animal), Some(partner_animal)) = (body.as_animal(), partner.as_animal())
        else {
            return false;
        };

        partner.is_alive()
            && body_animal.can_mate(partner_animal)
            && utils::entity_is_visible(ctx, &partner)
            && ctx.timestamp <= self.spawn_child_at_time.load(Ordering::Relaxed)
            && !ctx.has_memory_value(&mem::IS_PANICKING)
            && !entity_is_panicking(&partner)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(partner) = self.find_valid_breed_partner(ctx) else {
            return;
        };

        let body = ctx.body;
        ctx.set_memory(&mem::BREED_TARGET, partner.clone());
        self.set_partner_breed_target(&partner, body);
        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &partner,
            self.speed_modifier,
            self.close_enough_distance,
        );

        let duration = MIN_DURATION + rand::random_range(0..MAX_DURATION);
        self.spawn_child_at_time
            .store(ctx.timestamp + i64::from(duration), Ordering::Relaxed);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(partner) = ctx.get_memory(&mem::BREED_TARGET) else {
            return;
        };

        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &partner,
            self.speed_modifier,
            self.close_enough_distance,
        );

        if !utils::closer_than(ctx.body, &partner, BREED_RANGE)
            || ctx.timestamp < self.spawn_child_at_time.load(Ordering::Relaxed)
        {
            return;
        }

        let body = ctx.body;
        if let (Some(body_animal), Some(partner_animal), Some(world)) =
            (body.as_animal(), partner.as_animal(), body.level())
        {
            body_animal.spawn_child_from_breeding(&world, partner_animal);
        }

        ctx.erase_memory(&mem::BREED_TARGET);
        if let Some(brain) = partner.as_mob().and_then(Mob::brain) {
            brain.lock().erase_memory(&mem::BREED_TARGET);
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::BREED_TARGET);
        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::LOOK_TARGET);
        self.spawn_child_at_time.store(0, Ordering::Relaxed);
    }
}
