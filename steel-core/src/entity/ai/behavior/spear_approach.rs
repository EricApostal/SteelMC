//! Vanilla `SpearApproach`: run into range while holding a kinetic weapon.

use std::sync::Arc;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::EntityTracker;
use crate::entity::mob::Mob as _;

use super::spear_attack::SpearStatus;

/// Vanilla `SpearApproach(speedModifierWhenRepositioning, approachDistance)`.
pub struct SpearApproach {
    state: BehaviorState,
    speed_modifier_when_repositioning: f64,
    approach_distance_sqr: f64,
}

impl SpearApproach {
    /// Vanilla `SpearApproach(speedModifierWhenRepositioning, approachDistance)`.
    #[must_use]
    pub fn new(speed_modifier_when_repositioning: f64, approach_distance: f32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(
                    &mem::SPEAR_STATUS as MemoryModuleTypeRef,
                    MemoryStatus::ValueAbsent,
                )],
                1,
                1,
            ),
            speed_modifier_when_repositioning,
            approach_distance_sqr: f64::from(approach_distance) * f64::from(approach_distance),
        }
    }

    fn attack_target(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
        ctx.get_memory(&mem::ATTACK_TARGET)
    }

    /// Vanilla `ableToAttack`.
    fn able_to_attack(&self, ctx: &BehaviorContext<'_>) -> bool {
        Self::attack_target(ctx).is_some() && ctx.body.with_weapon_item_has_kinetic_weapon()
    }

    /// Vanilla `farEnough`.
    fn far_enough(&self, ctx: &BehaviorContext<'_>) -> bool {
        let (Some(target), body) = (Self::attack_target(ctx), ctx.body) else {
            return false;
        };
        let target_pos = target.position();
        body.distance_to_sqr(target_pos) > self.approach_distance_sqr
    }
}

/// Main-hand kinetic-weapon probe: declared in `spear_attack.rs`, re-declared here
/// so each behavior file compiles standalone without cross-file private imports.
trait WeaponItemKineticProbe {
    fn with_weapon_item_has_kinetic_weapon(&self) -> bool;
}

impl WeaponItemKineticProbe for SharedEntity {
    fn with_weapon_item_has_kinetic_weapon(&self) -> bool {
        let mut has = false;
        self.with_weapon_item(&mut |item| {
            has = item.is_some_and(|item| {
                item.has(steel_registry::data_components::vanilla_components::KINETIC_WEAPON)
            });
        });
        has
    }
}

impl Behavior for SpearApproach {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.able_to_attack(ctx)
            && ctx
                .body
                .as_living_entity()
                .is_none_or(|living| !living.is_using_item())
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_mob() {
            mob.set_aggressive(true);
        }
        ctx.set_memory(&mem::SPEAR_STATUS, SpearStatus::Approach);
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        self.able_to_attack(ctx) && self.far_enough(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = Self::attack_target(ctx) else {
            return;
        };
        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(target.clone(), true)),
        );

        let vehicle_boost = ctx.body.controlled_vehicle().map_or(1.0, |vehicle| {
            vehicle
                .as_mob()
                .map_or(1.0, |mob| mob.charge_speed_modifier())
        });
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.move_to_pos(
                target.position(),
                f64::from(vehicle_boost) * f64::from(self.speed_modifier_when_repositioning),
            );
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
        ctx.set_memory(&mem::SPEAR_STATUS, SpearStatus::Charging);
    }
}
