//! Vanilla witch raid target goals: `NearestHealableRaiderTargetGoal` finds a raider for the
//! witch to support during a raid, and `NearestAttackableWitchTargetGoal` targets players
//! when the witch is not busy supporting.
//!
//! Deliberate difference: vanilla `Witch` keeps both goals in fields so `aiStep` can call
//! `healRaidersGoal.decrementCooldown()` and `attackPlayersGoal.setCanAttack(...)`. Steel's
//! goals live inside the goal selector without an external handle, so the witch entity owns a
//! [`WitchTargetState`] and hands a clone to both goals.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use steel_registry::vanilla_entities;

use super::nearest_attackable_target::NearestAttackableTargetGoal;
use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::{LivingEntity, PathfinderMob};
use crate::world::World;

/// Vanilla `NearestHealableRaiderTargetGoal.DEFAULT_COOLDOWN`.
const DEFAULT_COOLDOWN: i32 = 200;

/// Shared vanilla witch target state.
///
/// Vanilla `Witch` drives its two target goals from `aiStep`; Steel shares this handle
/// between the witch entity and both goals instead of storing the goals in fields.
#[derive(Clone)]
pub struct WitchTargetState {
    inner: Arc<WitchTargetStateInner>,
}

#[derive(Debug)]
struct WitchTargetStateInner {
    heal_cooldown: AtomicI32,
    can_attack: AtomicBool,
}

impl WitchTargetState {
    /// Vanilla `Witch` field defaults: no heal cooldown, player targeting enabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Arc::new(WitchTargetStateInner {
                heal_cooldown: AtomicI32::new(0),
                can_attack: AtomicBool::new(true),
            }),
        }
    }

    /// Vanilla `NearestHealableRaiderTargetGoal.getCooldown`.
    #[must_use]
    pub fn heal_cooldown(&self) -> i32 {
        self.inner.heal_cooldown.load(Ordering::Relaxed)
    }

    /// Vanilla `NearestHealableRaiderTargetGoal.decrementCooldown`, called from
    /// `Witch.aiStep`.
    pub fn decrement_heal_cooldown(&self) {
        self.inner.heal_cooldown.fetch_sub(1, Ordering::Relaxed);
    }

    /// Vanilla `NearestHealableRaiderTargetGoal.start` cooldown assignment.
    pub fn set_heal_cooldown(&self, cooldown: i32) {
        self.inner.heal_cooldown.store(cooldown, Ordering::Relaxed);
    }

    /// Vanilla `NearestAttackableWitchTargetGoal.canAttack`.
    #[must_use]
    pub fn can_attack(&self) -> bool {
        self.inner.can_attack.load(Ordering::Relaxed)
    }

    /// Vanilla `NearestAttackableWitchTargetGoal.setCanAttack`, called from `Witch.aiStep`.
    pub fn set_can_attack(&self, can_attack: bool) {
        self.inner.can_attack.store(can_attack, Ordering::Relaxed);
    }
}

impl Default for WitchTargetState {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `NearestHealableRaiderTargetGoal<T extends Raider>`.
pub struct NearestHealableRaiderTargetGoal {
    state: WitchTargetState,
    inner: NearestAttackableTargetGoal,
}

impl NearestHealableRaiderTargetGoal {
    /// Vanilla `new NearestHealableRaiderTargetGoal<>(raider, Raider.class, mustSee, subselector)`.
    ///
    /// Vanilla searches the `Raider` class, which Steel expresses through the
    /// [`Raider`](crate::entity::raid::Raider) capability. Vanilla passes
    /// `randomInterval = 500` to the base goal, which the overridden `canUse` never
    /// consults, so Steel builds the base search without an interval.
    #[must_use]
    pub fn new(
        state: WitchTargetState,
        selector: impl Fn(&dyn LivingEntity, &World) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            state,
            inner: NearestAttackableTargetGoal::new_with_interval(
                0,
                true,
                false,
                move |target, world| target.as_raider().is_some() && selector(target, world),
            ),
        }
    }

    /// Vanilla `Witch.registerGoals` subselector: any raider (the base search already
    /// restricts candidates to the `Raider` capability) that is not a witch.
    #[must_use]
    pub fn for_witch(state: WitchTargetState) -> Self {
        Self::new(state, |target, _world| {
            target.entity_type() != &vanilla_entities::WITCH
        })
    }
}

impl Goal for NearestHealableRaiderTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.state.heal_cooldown() > 0 || !rand::random::<bool>() {
            return false;
        }

        let Some(world) = mob.level() else {
            return false;
        };
        if !mob
            .as_raider()
            .is_some_and(|raider| raider.has_active_raid(&world))
        {
            return false;
        }

        self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.state
            .set_heal_cooldown(reduced_tick_delay(DEFAULT_COOLDOWN));
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }
}

/// Vanilla `NearestAttackableWitchTargetGoal<T extends LivingEntity>`.
pub struct NearestAttackableWitchTargetGoal {
    state: WitchTargetState,
    inner: NearestAttackableTargetGoal,
}

impl NearestAttackableWitchTargetGoal {
    /// Vanilla `new NearestAttackableWitchTargetGoal<>(raider, targetType, randomInterval, mustSee, mustReach, subselector)`.
    #[must_use]
    pub fn new(
        state: WitchTargetState,
        random_interval: i32,
        must_see: bool,
        must_reach: bool,
        selector: impl Fn(&dyn LivingEntity, &World) -> bool + Send + Sync + 'static,
    ) -> Self {
        Self {
            state,
            inner: NearestAttackableTargetGoal::new_with_interval(
                random_interval,
                must_see,
                must_reach,
                selector,
            ),
        }
    }

    /// Vanilla `Witch.registerGoals`:
    /// `new NearestAttackableWitchTargetGoal<>(this, Player.class, 10, true, false, null)`.
    #[must_use]
    pub fn for_witch_players(state: WitchTargetState) -> Self {
        Self {
            state,
            inner: NearestAttackableTargetGoal::new_for_players_with_interval(
                10,
                true,
                false,
                |_, _| true,
            ),
        }
    }
}

impl Goal for NearestAttackableWitchTargetGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.state.can_attack() && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use glam::DVec3;
    use steel_registry::vanilla_entities;

    use super::*;
    use crate::entity::entities::PigEntity;

    /// A raider-free stand-in mob; the goals only reach the mob through `PathfinderMob`.
    fn test_mob() -> PigEntity {
        PigEntity::new(&vanilla_entities::PIG, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn witch_target_state_mirrors_vanilla_fields() {
        let state = WitchTargetState::new();
        assert_eq!(state.heal_cooldown(), 0);
        assert!(state.can_attack());

        state.set_heal_cooldown(reduced_tick_delay(DEFAULT_COOLDOWN));
        assert_eq!(state.heal_cooldown(), 100);
        state.decrement_heal_cooldown();
        assert_eq!(state.heal_cooldown(), 99);

        state.set_can_attack(false);
        assert!(!state.can_attack());
    }

    #[test]
    fn healable_raider_goal_is_blocked_by_a_running_cooldown() {
        let state = WitchTargetState::new();
        state.set_heal_cooldown(reduced_tick_delay(DEFAULT_COOLDOWN));
        let mut goal = NearestHealableRaiderTargetGoal::for_witch(state);
        let mob = test_mob();

        assert!(!goal.can_use(&mob));
    }

    #[test]
    fn attackable_witch_goal_honours_can_attack() {
        let state = WitchTargetState::new();
        let mut goal =
            NearestAttackableWitchTargetGoal::new(state.clone(), 0, true, false, |_, _| true);
        let mob = test_mob();

        // No world: the search itself never matches.
        assert!(!goal.can_use(&mob));

        state.set_can_attack(false);
        assert!(!goal.can_use(&mob));
        assert!(!state.can_attack());
    }

    #[test]
    fn goals_claim_the_target_control_like_vanilla() {
        let state = WitchTargetState::new();
        let heal = NearestHealableRaiderTargetGoal::for_witch(state.clone());
        let attack = NearestAttackableWitchTargetGoal::new(state, 0, true, false, |_, _| true);

        assert_eq!(heal.controls(), GoalControls::TARGET);
        assert_eq!(attack.controls(), GoalControls::TARGET);
    }
}
