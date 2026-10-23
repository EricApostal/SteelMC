//! Vanilla `DismountOrSkipMounting`: drop an invalid ride, or stop trying to ride.

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `DismountOrSkipMounting.create(maxWalkDistToRideTarget, dontRideIf)`.
///
/// `dontRideIf` is a plain function pointer so the trigger stays `Send + Sync`.
pub struct DismountOrSkipMounting {
    max_walk_dist_to_ride_target: i32,
    dont_ride_if: fn(&SharedEntity, &SharedEntity) -> bool,
}

impl DismountOrSkipMounting {
    /// Vanilla `DismountOrSkipMounting.create(maxWalkDistToRideTarget, dontRideIf)`.
    #[must_use]
    pub fn create(
        max_walk_dist_to_ride_target: i32,
        dont_ride_if: fn(&SharedEntity, &SharedEntity) -> bool,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            max_walk_dist_to_ride_target,
            dont_ride_if,
        })
        .with_required_memories(vec![&mem::RIDE_TARGET])
    }

    /// Vanilla `isVehicleValid`.
    fn is_vehicle_valid(body: &SharedEntity, vehicle: &SharedEntity, max_walk_dist: i32) -> bool {
        vehicle.is_alive()
            && vehicle.position().distance_squared(body.position())
                <= f64::from(max_walk_dist) * f64::from(max_walk_dist)
            && vehicle.level().map(|level| level.key.clone())
                == body.level().map(|level| level.key.clone())
    }
}

impl Trigger for DismountOrSkipMounting {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let current_vehicle = ctx.body.vehicle();
        let target_vehicle = ctx.get_memory(&mem::RIDE_TARGET);

        if current_vehicle.is_none() && target_vehicle.is_none() {
            return false;
        }

        let vehicle = current_vehicle.or(target_vehicle);
        let Some(vehicle) = vehicle else {
            return false;
        };

        let valid = Self::is_vehicle_valid(ctx.body, &vehicle, self.max_walk_dist_to_ride_target)
            && !(self.dont_ride_if)(ctx.body, &vehicle);
        if valid {
            return false;
        }

        ctx.body.stop_riding();
        ctx.erase_memory(&mem::RIDE_TARGET);
        true
    }
}
