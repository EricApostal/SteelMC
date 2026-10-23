//! Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach`.

use std::sync::Arc;

use super::utils;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::{EntityTracker, WalkTarget};

/// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.PROJECTILE_ATTACK_RANGE_BUFFER`.
const PROJECTILE_ATTACK_RANGE_BUFFER: i32 = 1;

/// Vanilla `Function<LivingEntity, Float>` speed source.
#[derive(Clone, Copy)]
enum SpeedModifier {
    /// Vanilla `create(float speedModifier)`.
    Constant(f32),
    /// Vanilla `create(Function<LivingEntity, Float>)`.
    Function(fn(&SharedEntity) -> f32),
}

impl SpeedModifier {
    fn apply(self, body: &SharedEntity) -> f32 {
        match self {
            Self::Constant(speed_modifier) => speed_modifier,
            Self::Function(speed_modifier) => speed_modifier(body),
        }
    }
}

/// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach`.
pub struct SetWalkTargetFromAttackTargetIfTargetOutOfReach {
    speed_modifier: SpeedModifier,
}

impl SetWalkTargetFromAttackTargetIfTargetOutOfReach {
    /// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(speedModifier)`.
    #[must_use]
    pub fn create(speed_modifier: f32) -> OneShot<Self> {
        Self::build(SpeedModifier::Constant(speed_modifier))
    }

    /// Vanilla `SetWalkTargetFromAttackTargetIfTargetOutOfReach.create(Function<LivingEntity, Float>)`.
    ///
    /// Steel takes a plain function pointer so the behavior stays `Send + Sync`.
    #[must_use]
    pub fn create_with_speed_fn(speed_modifier: fn(&SharedEntity) -> f32) -> OneShot<Self> {
        Self::build(SpeedModifier::Function(speed_modifier))
    }

    fn build(speed_modifier: SpeedModifier) -> OneShot<Self> {
        OneShot::new(Self { speed_modifier }).with_required_memories(vec![
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
            &mem::ATTACK_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }
}

/// Vanilla `BehaviorUtils.isWithinAttackRange`.
///
/// Vanilla's projectile-weapon branch needs `ProjectileWeaponItem.getDefaultProjectileRange`
/// plus `Mob.canUseNonMeleeWeapon`; neither is ported, so this is the melee branch,
/// which is what vanilla falls through to for every non-projectile-weapon holder.
fn is_within_attack_range(body: &SharedEntity, target: &SharedEntity, _range_buffer: i32) -> bool {
    utils::is_within_melee_attack_range(body, target)
}

impl Trigger for SetWalkTargetFromAttackTargetIfTargetOutOfReach {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        let Some(to_attack) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        let visible = ctx.memories().get(&mem::NEAREST_VISIBLE_LIVING_ENTITIES);
        let in_range = visible.is_some_and(|entities| {
            entities.contains(&to_attack)
                && is_within_attack_range(ctx.body, &to_attack, PROJECTILE_ATTACK_RANGE_BUFFER)
        });

        if in_range {
            ctx.erase_memory(&mem::WALK_TARGET);
        } else {
            ctx.set_memory(
                &mem::LOOK_TARGET,
                Arc::new(EntityTracker::new(to_attack.clone(), true)),
            );
            ctx.set_memory(
                &mem::WALK_TARGET,
                WalkTarget::new(
                    Arc::new(EntityTracker::new(to_attack, false)),
                    self.speed_modifier.apply(ctx.body),
                    0,
                ),
            );
        }

        true
    }
}
