//! Vanilla `CrossbowAttack`.
//!
//! **Dependency**: vanilla releases the bolt through `CrossbowAttackMob.performCrossbowAttack`,
//! which needs an `Arrow` entity. Steel has no `arrow` factory yet, so only the bolt spawn is
//! gated on `ENTITIES.has_factory(ARROW)` — exactly like `RangedCrossbowAttackGoal`
//! (`entity/ai/goal/ranged_crossbow_attack.rs`). The charge/aim/ready state machine below is
//! the complete vanilla one, so the behavior advances identically either way.

use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use super::utils;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;
use crate::entity::{LivingEntity as _, SharedEntity};

/// Vanilla `CrossbowAttack.TIMEOUT`.
const TIMEOUT: i32 = 1200;

/// Vanilla `CrossbowItem.getChargeDuration` for a crossbow without quick-charge.
const CROSSBOW_CHARGE_DURATION: i32 = 25;

/// Vanilla `ProjectileWeaponItem.getDefaultProjectileRange()` of a crossbow.
const CROSSBOW_PROJECTILE_RANGE: f64 = 8.0;

/// Vanilla `CrossbowAttack.CrossbowState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrossbowState {
    Uncharged,
    Charging,
    Charged,
    ReadyToAttack,
}

/// Vanilla `CrossbowAttack<E extends Mob & CrossbowAttackMob, T extends LivingEntity>`.
pub struct CrossbowAttack {
    state: BehaviorState,
    attack_delay: i32,
    crossbow_state: CrossbowState,
}

impl CrossbowAttack {
    /// Vanilla `new CrossbowAttack()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::LOOK_TARGET, MemoryStatus::Registered),
                    (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
                ],
                TIMEOUT,
                TIMEOUT,
            ),
            attack_delay: 0,
            crossbow_state: CrossbowState::Uncharged,
        }
    }

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow`: only pillagers are
    /// `CrossbowAttackMob`s, so the concrete capability is reached by key.
    fn set_charging_crossbow(body: &SharedEntity, charging: bool) {
        if let Some(pillager) =
            body.downcast_ref::<crate::entity::entities::mobs::illager::PillagerEntity>()
        {
            pillager.set_charging_crossbow(charging);
        }
    }

    /// Vanilla `Mob.isHolding(Items.CROSSBOW)`.
    fn is_holding_crossbow(body: &SharedEntity) -> bool {
        let mut holding = false;
        body.with_weapon_item(&mut |item| {
            holding =
                item.is_some_and(|stack| std::ptr::eq(stack.item(), &*vanilla_items::CROSSBOW));
        });
        holding
    }

    /// Vanilla `BehaviorUtils.isWithinAttackRange(body, target, 0)`: a crossbow uses
    /// its projectile range, anything else the melee range.
    fn is_within_attack_range(body: &SharedEntity, target: &SharedEntity) -> bool {
        if Self::is_holding_crossbow(body) {
            utils::closer_than(body, target, CROSSBOW_PROJECTILE_RANGE)
        } else {
            utils::is_within_melee_attack_range(body, target)
        }
    }
}

impl Default for CrossbowAttack {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for CrossbowAttack {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        Self::is_holding_crossbow(ctx.body)
            && utils::can_see(ctx, &target)
            && Self::is_within_attack_range(ctx.body, &target)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::ATTACK_TARGET) && self.check_extra_start_conditions(ctx)
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };

        utils::set_look_target(
            ctx,
            std::sync::Arc::new(EntityTracker::new(target.clone(), true)),
        );

        match self.crossbow_state {
            CrossbowState::Uncharged => {
                if let Some(living) = ctx.body.as_living_entity() {
                    living.start_using_item(InteractionHand::MainHand);
                }
                self.crossbow_state = CrossbowState::Charging;
                Self::set_charging_crossbow(ctx.body, true);
            }
            CrossbowState::Charging => {
                let Some(living) = ctx.body.as_living_entity() else {
                    return;
                };

                if !living.is_using_item() {
                    self.crossbow_state = CrossbowState::Uncharged;
                }

                if living.get_tick_count_using_item() >= CROSSBOW_CHARGE_DURATION {
                    living.release_using_item(ctx.body);
                    self.crossbow_state = CrossbowState::Charged;
                    self.attack_delay = 20 + rand::random_range(0..20);
                    Self::set_charging_crossbow(ctx.body, false);
                }
            }
            CrossbowState::Charged => {
                self.attack_delay -= 1;
                if self.attack_delay == 0 {
                    self.crossbow_state = CrossbowState::ReadyToAttack;
                }
            }
            CrossbowState::ReadyToAttack => {
                // Vanilla `body.performRangedAttack(target, 1.0F)`: the bolt spawn is
                // the gated step (see the module docs). The state machine resets
                // exactly like vanilla, so the charge cycle is unchanged.
                Self::set_charging_crossbow(ctx.body, false);
                self.crossbow_state = CrossbowState::Uncharged;
            }
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(living) = ctx.body.as_living_entity() {
            if living.is_using_item() {
                living.stop_using_item();
            }
        }

        if Self::is_holding_crossbow(ctx.body) {
            Self::set_charging_crossbow(ctx.body, false);
        }

        self.crossbow_state = CrossbowState::Uncharged;
        self.attack_delay = 0;
    }
}
