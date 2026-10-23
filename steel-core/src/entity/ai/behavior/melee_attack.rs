//! Vanilla `MeleeAttack`: hit the attack target when it is in reach.

use std::sync::Arc;

use steel_utils::types::InteractionHand;

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `MeleeAttack.create(cooldownBetweenAttacks)` predicate test.
type CanAttackPredicate = fn(&SharedEntity) -> bool;

/// Vanilla `MeleeAttack<T extends Mob>`.
///
/// The predicate is a plain function pointer (not a closure) so the trigger
/// stays `Send + Sync`, matching vanilla's `create(Predicate, int)` factories.
pub struct MeleeAttack {
    can_attack: CanAttackPredicate,
    cooldown_between_attacks: i32,
}

impl MeleeAttack {
    /// Vanilla `MeleeAttack.create(cooldownBetweenAttacks)`.
    #[must_use]
    pub fn create(cooldown_between_attacks: i32) -> OneShot<Self> {
        Self::create_with_predicate(|_| true, cooldown_between_attacks)
    }

    /// Vanilla `MeleeAttack.create(Predicate, cooldownBetweenAttacks)`.
    #[must_use]
    pub fn create_with_predicate(
        can_attack: CanAttackPredicate,
        cooldown_between_attacks: i32,
    ) -> OneShot<Self> {
        OneShot::new(Self {
            can_attack,
            cooldown_between_attacks,
        })
        .with_required_memories(vec![
            &mem::LOOK_TARGET,
            &mem::ATTACK_TARGET,
            &mem::ATTACK_COOLING_DOWN,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }

    /// Vanilla `MeleeAttack.isHoldingUsableNonMeleeWeapon`.
    ///
    /// Steel's `Mob` trait has no `canUseNonMeleeWeapon` override surface yet:
    /// vanilla's base `Mob.canUseNonMeleeWeapon` returns `false` and only
    /// pillagers/piglins override it, so this conservatively mirrors the base
    /// (`false`) until those overrides are ported.
    fn is_holding_usable_non_melee_weapon(_body: &SharedEntity) -> bool {
        false
    }
}

impl Trigger for MeleeAttack {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.check_memory(&mem::ATTACK_COOLING_DOWN, MemoryStatus::ValueAbsent) {
            return false;
        }
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };
        if !(self.can_attack)(ctx.body)
            || Self::is_holding_usable_non_melee_weapon(ctx.body)
            || !utils::is_within_melee_attack_range(ctx.body, &target)
            || !utils::entity_is_visible(ctx, &target)
        {
            return false;
        }

        ctx.set_memory(
            &mem::LOOK_TARGET,
            Arc::new(EntityTracker::new(target.clone(), true)),
        );
        if let Some(living) = ctx.body.as_living_entity() {
            living.swing_for_attack(InteractionHand::MainHand);
        }
        if let Some(mob) = ctx.body.as_mob() {
            mob.do_hurt_target(ctx.body, ctx.level, &target);
        }
        ctx.set_memory_with_expiry(
            &mem::ATTACK_COOLING_DOWN,
            true,
            i64::from(self.cooldown_between_attacks),
        );
        true
    }
}
