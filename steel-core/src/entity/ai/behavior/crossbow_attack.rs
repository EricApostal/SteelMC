//! Vanilla `CrossbowAttack`.
//!
//! The bolt is released through [`RangedAttackMob::perform_ranged_attack`], the Steel
//! capability behind vanilla's `CrossbowAttackMob.performCrossbowAttack` /
//! `RangedAttackMob` bound. Any crossbow mob Steel models (`PillagerEntity`,
//! `PiglinEntity`) implements the capability and fires its crossbow at the attack target.

use std::ptr;
use std::sync::Arc;

use steel_registry::vanilla_items;
use steel_utils::Downcast as _;
use steel_utils::types::InteractionHand;

use super::utils;
use crate::behavior::items::get_charge_duration;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;
use crate::entity::entities::mobs::hostile::piglin::PiglinEntity;
use crate::entity::entities::mobs::illager::PillagerEntity;

/// Vanilla `CrossbowAttack.TIMEOUT`.
const TIMEOUT: i32 = 1200;

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

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` for the mobs Steel models
    /// as `CrossbowAttackMob`s: the pillager and the piglin both read the flag
    /// for their crossbow arm pose (`IllagerArmPose.CROSSBOW_CHARGE`,
    /// `PiglinArmPose.CROSSBOW_CHARGE`).
    fn set_charging_crossbow(body: &SharedEntity, charging: bool) {
        if let Some(pillager) = body.downcast_ref::<PillagerEntity>() {
            pillager.set_charging_crossbow(charging);
        } else if let Some(piglin) = body.downcast_ref::<PiglinEntity>() {
            piglin.set_charging_crossbow(charging);
        }
    }

    /// Vanilla `CrossbowItem.getChargeDuration(body.getUseItem(), body)`: the
    /// use hand is the weapon-holding hand the charge started in, so Quick
    /// Charge shortens the release threshold exactly as it shortens the charge
    /// itself (25 ticks without it).
    fn charge_duration(body: &SharedEntity) -> i32 {
        body.as_living_entity().map_or(0, |living| {
            get_charge_duration(&living.get_item_in_hand(Self::weapon_holding_hand(body)))
        })
    }

    /// Vanilla `ProjectileUtil.getWeaponHoldingHand`: the main hand when it holds the
    /// crossbow, otherwise the off hand.
    fn weapon_holding_hand(body: &SharedEntity) -> InteractionHand {
        let holds_crossbow_in_main_hand = body.as_living_entity().is_some_and(|living| {
            living
                .get_item_in_hand(InteractionHand::MainHand)
                .is(&vanilla_items::CROSSBOW)
        });
        if holds_crossbow_in_main_hand {
            InteractionHand::MainHand
        } else {
            InteractionHand::OffHand
        }
    }

    /// Vanilla `Mob.isHolding(Items.CROSSBOW)`.
    fn is_holding_crossbow(body: &SharedEntity) -> bool {
        let mut holding = false;
        body.with_weapon_item(&mut |item| {
            holding = item
                .is_some_and(|stack| ptr::eq(stack.item(), &raw const *vanilla_items::CROSSBOW));
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
            Arc::new(EntityTracker::new(SharedEntity::clone(&target), true)),
        );

        match self.crossbow_state {
            CrossbowState::Uncharged => {
                if let Some(living) = ctx.body.as_living_entity() {
                    living.start_using_item(Self::weapon_holding_hand(ctx.body));
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

                if living.get_tick_count_using_item() >= Self::charge_duration(ctx.body) {
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
                // Vanilla `body.performRangedAttack(target, 1.0F)`.
                if let Some(ranged_attack_mob) = ctx.body.as_ranged_attack_mob() {
                    ranged_attack_mob.perform_ranged_attack(&target, 1.0);
                }
                self.crossbow_state = CrossbowState::Uncharged;
            }
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(living) = ctx.body.as_living_entity()
            && living.is_using_item()
        {
            living.stop_using_item();
        }

        if Self::is_holding_crossbow(ctx.body) {
            Self::set_charging_crossbow(ctx.body, false);
            // Vanilla also clears `DataComponents.CHARGED_PROJECTILES` on the use stack
            // here, but it reads that stack through `getUseItem()` after
            // `stopUsingItem()` has already reset `useItem` to `ItemStack.EMPTY`, so the
            // clear never reaches the crossbow in hand and there is nothing to clear.
        }

        // Vanilla keeps `crossbowState`/`attackDelay` across stop; the next `tick` resumes
        // the same charge phase, so Steel does too.
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use glam::DVec3;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::Identifier;
    use steel_utils::types::InteractionHand;

    use super::CrossbowAttack;
    use crate::entity::SharedEntity;
    use crate::entity::entities::mobs::hostile::piglin::PiglinEntity;
    use crate::entity::entities::mobs::illager::PillagerEntity;
    use steel_utils::Downcast as _;

    /// Vanilla `CrossbowAttackMob.setChargingCrossbow` has to reach every
    /// crossbow mob, not just the pillager: the piglin reads the same flag for
    /// its `PiglinArmPose.CROSSBOW_CHARGE`, so a pillager-only downcast left the
    /// piglin's charge pose unreachable.
    #[test]
    fn charging_flag_reaches_both_crossbow_mobs() {
        init_vanilla_registry();
        let pillager: SharedEntity = Arc::new(PillagerEntity::new(
            &vanilla_entities::PILLAGER,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));
        let piglin: SharedEntity = Arc::new(PiglinEntity::new(
            &vanilla_entities::PIGLIN,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));

        CrossbowAttack::set_charging_crossbow(&pillager, true);
        CrossbowAttack::set_charging_crossbow(&piglin, true);
        assert!(
            pillager
                .downcast_ref::<PillagerEntity>()
                .expect("the pillager body")
                .is_charging_crossbow()
        );
        assert!(
            piglin
                .downcast_ref::<PiglinEntity>()
                .expect("the piglin body")
                .is_charging_crossbow(),
            "the piglin's charge pose needs this flag"
        );

        CrossbowAttack::set_charging_crossbow(&pillager, false);
        CrossbowAttack::set_charging_crossbow(&piglin, false);
        assert!(
            !piglin
                .downcast_ref::<PiglinEntity>()
                .expect("the piglin body")
                .is_charging_crossbow()
        );
    }

    /// Vanilla releases on `getTicksUsingItem() >= CrossbowItem.getChargeDuration
    /// (body.getUseItem(), body)`, so Quick Charge shortens the brain's release
    /// threshold instead of leaving it pinned at the plain 25 ticks.
    #[test]
    fn release_threshold_uses_the_held_crossbows_charge_duration() {
        init_vanilla_registry();
        let piglin: SharedEntity = Arc::new(PiglinEntity::new(
            &vanilla_entities::PIGLIN,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));
        let living = piglin.as_living_entity().expect("the piglin is living");

        living.set_item_in_hand(
            InteractionHand::MainHand,
            ItemStack::new(&vanilla_items::CROSSBOW),
        );
        assert_eq!(CrossbowAttack::charge_duration(&piglin), 25);

        let mut quick = ItemStack::new(&vanilla_items::CROSSBOW);
        quick.set_enchantments(&[(Identifier::vanilla_static("quick_charge"), 2)], false);
        living.set_item_in_hand(InteractionHand::MainHand, quick);
        assert_eq!(CrossbowAttack::charge_duration(&piglin), 15);

        // A body with no crossbow at all reports the vanilla default duration.
        let bare: SharedEntity = Arc::new(PiglinEntity::new(
            &vanilla_entities::PIGLIN,
            3,
            DVec3::ZERO,
            Weak::new(),
        ));
        assert_eq!(CrossbowAttack::charge_duration(&bare), 25);
    }
}
