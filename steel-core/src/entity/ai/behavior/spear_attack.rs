//! Vanilla `SpearAttack`: the charging phase of the trident-wielding attack.
//!
//! The `SpearStatus` enum lives at the top of this file (per the integrator
//! contract) and is imported by `ai::brain::memory::module_type`.

use std::f64::consts::TAU;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::data_components::components::KineticWeapon;
use steel_registry::data_components::vanilla_components;
use steel_utils::types::InteractionHand;

use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryModuleTypeRef, MemoryStatus, module_type as mem};
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `SpearAttack.SpearStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpearStatus {
    Approach,
    Charging,
    Retreat,
}

/// Vanilla `SpearAttack.MIN_REPOSITION_DISTANCE`.
pub const MIN_REPOSITION_DISTANCE: i32 = 6;
/// Vanilla `SpearAttack.MAX_REPOSITION_DISTANCE`.
pub const MAX_REPOSITION_DISTANCE: i32 = 7;

/// Vanilla `SpearAttack<PathfinderMob>`.
pub struct SpearAttack {
    state: BehaviorState,
    speed_modifier_when_charging: f64,
    speed_modifier_when_repositioning: f64,
    target_in_range_radius_sqr: f64,
}

impl SpearAttack {
    /// Vanilla `SpearAttack(speedModifierWhenCharging, speedModifierWhenRepositioning,
    /// targetInRangeRadius)`.
    #[must_use]
    pub fn new(
        speed_modifier_when_charging: f64,
        speed_modifier_when_repositioning: f64,
        target_in_range_radius: f32,
    ) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(
                    &mem::SPEAR_STATUS as MemoryModuleTypeRef,
                    MemoryStatus::ValuePresent,
                )],
                1,
                1,
            ),
            speed_modifier_when_charging,
            speed_modifier_when_repositioning,
            target_in_range_radius_sqr: f64::from(target_in_range_radius)
                * f64::from(target_in_range_radius),
        }
    }

    fn attack_target(ctx: &BehaviorContext<'_>) -> Option<SharedEntity> {
        ctx.get_memory(&mem::ATTACK_TARGET)
    }

    /// Vanilla `ableToAttack`: an attack target plus a kinetic weapon in hand.
    ///
    /// The main-hand kinetic-weapon check reads the `KINETIC_WEAPON` typed component
    /// when the registry bridges it; `has_component` on the component key covers the
    /// same ground through the item's data map.
    #[expect(
        clippy::unused_self,
        reason = "kept as a method to mirror vanilla `SpearAttack.ableToAttack`"
    )]
    fn able_to_attack(&self, ctx: &BehaviorContext<'_>) -> bool {
        Self::attack_target(ctx).is_some() && ctx.body.with_weapon_item_has_kinetic_weapon()
    }

    /// Vanilla `getKineticWeaponUseDuration`; falls back to `0` without the component.
    fn kinetic_weapon_use_duration(body: &SharedEntity) -> i32 {
        let mut duration = 0;
        body.with_weapon_item(&mut |item| {
            if let Some(item) = item {
                duration = item
                    .get(vanilla_components::KINETIC_WEAPON)
                    .map_or(0, KineticWeapon::compute_damage_use_duration);
            }
        });
        duration
    }
}

/// Main-hand kinetic-weapon probe, kept beside the behavior that needs it.
trait WeaponItemKineticProbe {
    /// Returns whether the main-hand item carries the kinetic-weapon component.
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

impl Behavior for SpearAttack {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::SPEAR_STATUS)
            .is_some_and(|status| status == SpearStatus::Charging)
            && self.able_to_attack(ctx)
            && ctx
                .body
                .as_living_entity()
                .is_none_or(|living| !living.is_using_item())
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_mob() {
            mob.set_aggressive(true);
        }
        let duration = Self::kinetic_weapon_use_duration(ctx.body);
        ctx.set_memory(&mem::SPEAR_ENGAGE_TIME, duration);
        ctx.erase_memory(&mem::SPEAR_CHARGE_POSITION);
        if let Some(living) = ctx.body.as_living_entity() {
            living.start_using_item(InteractionHand::MainHand);
        }
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.get_memory(&mem::SPEAR_ENGAGE_TIME)
            .is_some_and(|ticks: i32| ticks > 0)
            && self.able_to_attack(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = Self::attack_target(ctx) else {
            return;
        };
        let target_dist_sqr = ctx.body.distance_to_sqr(target.position());

        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(SharedEntity::clone(&target), true)),
        );

        let engage = ctx.get_memory(&mem::SPEAR_ENGAGE_TIME).unwrap_or(0);
        ctx.set_memory(&mem::SPEAR_ENGAGE_TIME, engage - 1);

        let speed_modifier = 1.0;
        if let Some(away_pos) = ctx.get_memory(&mem::SPEAR_CHARGE_POSITION) {
            let away: DVec3 = away_pos;
            if let Some(mob) = ctx.body.as_pathfinder_mob() {
                mob.move_to_pos(
                    away,
                    speed_modifier * self.speed_modifier_when_repositioning,
                );
            }
            if ctx
                .body
                .as_pathfinder_mob()
                .is_some_and(|mob| mob.mob_base().navigation().lock().is_done())
            {
                ctx.erase_memory(&mem::SPEAR_CHARGE_POSITION);
            }
        } else {
            if let Some(mob) = ctx.body.as_pathfinder_mob() {
                mob.move_to_pos(
                    target.position(),
                    speed_modifier * self.speed_modifier_when_charging,
                );
            }
            let navigation_done = ctx
                .body
                .as_pathfinder_mob()
                .is_some_and(|mob| mob.mob_base().navigation().lock().is_done());
            if target_dist_sqr < self.target_in_range_radius_sqr || navigation_done {
                // Vanilla `LandRandomPos.getPosAway(mob, 6 +/- dist, 7 +/- dist, 7, target)`.
                let distance = target_dist_sqr.sqrt();
                let mount_distance = f64::from(i32::from(ctx.body.is_passenger())) * 2.0;
                let dxz = (f64::from(MIN_REPOSITION_DISTANCE) + mount_distance - distance).max(0.0);
                let range = f64::from(MAX_REPOSITION_DISTANCE) + mount_distance - distance;
                let away_pos = random_pos_away(ctx, dxz, range.max(1.0), target.position());
                ctx.set_memory(&mem::SPEAR_CHARGE_POSITION, away_pos);
            }
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(mob) = ctx.body.as_pathfinder_mob() {
            mob.mob_base().navigation().lock().stop();
        }
        if let Some(living) = ctx.body.as_living_entity() {
            living.stop_using_item();
        }
        ctx.erase_memory(&mem::SPEAR_CHARGE_POSITION);
        ctx.erase_memory(&mem::SPEAR_ENGAGE_TIME);
        ctx.set_memory(&mem::SPEAR_STATUS, SpearStatus::Retreat);
    }
}

/// Stand-in for `LandRandomPos.getPosAway` for the reposition step.
///
/// Steel's `ai::goal` random-pos helpers are `pub(crate)` to the goal module and
/// unreachable from `behavior`, so a random ring around the target (horizontal
/// distance range, fixed vertical spread) approximates the away position.
fn random_pos_away(
    _ctx: &BehaviorContext<'_>,
    min_dxz: f64,
    max_dxz: f64,
    target_pos: DVec3,
) -> DVec3 {
    let angle = rand::random_range(0.0..TAU);
    let radius = rand::random_range(min_dxz..max_dxz.max(min_dxz + 0.01));
    DVec3::new(
        target_pos.x + angle.cos() * radius,
        target_pos.y + f64::from(rand::random_range(-3..=3)),
        target_pos.z + angle.sin() * radius,
    )
}
