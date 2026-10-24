//! Vanilla `SpearRetreat`: fall back to a safe distance after the charge.

use std::f64::consts::TAU;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::data_components::vanilla_components;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::EntityTracker;
use crate::entity::mob::Mob;

use super::spear_attack::SpearStatus;

/// Vanilla `SpearRetreat.MIN_COOLDOWN_DISTANCE`.
pub const MIN_COOLDOWN_DISTANCE: i32 = 9;
/// Vanilla `SpearRetreat.MAX_COOLDOWN_DISTANCE`.
pub const MAX_COOLDOWN_DISTANCE: i32 = 11;
/// Vanilla `SpearRetreat.MAX_FLEEING_TIME`.
pub const MAX_FLEEING_TIME: i32 = 100;

/// Vanilla `SpearRetreat(speedModifierWhenRepositioning)`.
pub struct SpearRetreat {
    state: BehaviorState,
    speed_modifier_when_repositioning: f64,
}

impl SpearRetreat {
    /// Vanilla `SpearRetreat(speedModifierWhenRepositioning)`.
    #[must_use]
    pub fn new(speed_modifier_when_repositioning: f64) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(
                    &mem::SPEAR_STATUS as MemoryModuleTypeRef,
                    MemoryStatus::ValuePresent,
                )],
                MAX_FLEEING_TIME,
                MAX_FLEEING_TIME,
            ),
            speed_modifier_when_repositioning,
        }
    }

    fn attack_target(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
        ctx.get_memory(&mem::ATTACK_TARGET)
    }

    /// Vanilla `ableToAttack`.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `SpearRetreat.ableToAttack`"
    )]
    fn able_to_attack(&self, ctx: &BehaviorContext<'_>) -> bool {
        Self::attack_target(ctx).is_some() && ctx.body.with_weapon_item_has_kinetic_weapon()
    }

    /// Vanilla's `LandRandomPos.getPosAway` for the fleeing position.
    fn fleeing_position(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> DVec3 {
        let mount_distance = f64::from(i32::from(ctx.body.is_passenger())) * 2.0;
        let target_pos = target.position();
        let distance = ctx.body.position().distance(target_pos);
        let min_dxz = (f64::from(MIN_COOLDOWN_DISTANCE) + mount_distance - distance).max(0.0);
        let max_dxz = (f64::from(MAX_COOLDOWN_DISTANCE) + mount_distance - distance).max(1.0);
        let angle = rand::random_range(0.0..TAU);
        let radius = rand::random_range(min_dxz..max_dxz.max(min_dxz + 0.01));
        DVec3::new(
            target_pos.x + angle.cos() * radius,
            target_pos.y + f64::from(rand::random_range(-3..=3)),
            target_pos.z + angle.sin() * radius,
        )
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
            has = item.is_some_and(|item| item.has(vanilla_components::KINETIC_WEAPON));
        });
        has
    }
}

impl Behavior for SpearRetreat {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !(self.able_to_attack(ctx)
            && ctx
                .body
                .as_living_entity()
                .is_none_or(|living| !living.is_using_item()))
        {
            return false;
        }
        if !ctx
            .get_memory(&mem::SPEAR_STATUS)
            .is_some_and(|status| status == SpearStatus::Retreat)
        {
            return false;
        }

        let Some(target) = Self::attack_target(ctx) else {
            return false;
        };
        let away_pos = Self::fleeing_position(ctx, &target);
        ctx.set_memory(&mem::SPEAR_FLEEING_POSITION, away_pos);
        true
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_mob() {
            mob.set_aggressive(true);
        }
        ctx.set_memory(&mem::SPEAR_FLEEING_TIME, 0);
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::SPEAR_FLEEING_TIME)
            .is_some_and(|ticks: i32| ticks < MAX_FLEEING_TIME)
            && ctx.has_memory_value(&mem::SPEAR_FLEEING_POSITION)
            && ctx
                .body
                .as_pathfinder_mob()
                .is_some_and(|mob| !mob.mob_base().navigation().lock().is_done())
            && self.able_to_attack(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = Self::attack_target(ctx) else {
            return;
        };
        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(target, true)),
        );

        let fleeing_time: i32 = ctx.get_memory(&mem::SPEAR_FLEEING_TIME).unwrap_or(0);
        ctx.set_memory(&mem::SPEAR_FLEEING_TIME, fleeing_time + 1);

        let boost = ctx.body.controlled_vehicle().map_or(1.0, |vehicle| {
            vehicle.as_mob().map_or(1.0, Mob::charge_speed_modifier)
        });
        if let (Some(flee_pos), Some(mob)) = (
            ctx.get_memory(&mem::SPEAR_FLEEING_POSITION),
            ctx.body.as_pathfinder_mob(),
        ) {
            let flee: DVec3 = flee_pos;
            mob.move_to_pos(
                flee,
                f64::from(boost) * self.speed_modifier_when_repositioning,
            );
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
        if let Some(mob) = ctx.body.as_mob() {
            mob.set_aggressive(false);
        }
        if let Some(living) = ctx.body.as_living_entity() {
            living.stop_using_item();
        }
        ctx.erase_memory(&mem::SPEAR_FLEEING_TIME);
        ctx.erase_memory(&mem::SPEAR_FLEEING_POSITION);
        ctx.erase_memory(&mem::SPEAR_STATUS);
    }
}
