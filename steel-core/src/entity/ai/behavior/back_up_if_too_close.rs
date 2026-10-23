//! Vanilla `BackUpIfTooClose`: strafe backwards out of melee range.

use std::sync::Arc;

use super::utils;
use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::EntityTracker;

/// Vanilla `BackUpIfTooClose.create(int tooCloseDistance, float strafeSpeed)`:
/// while the attack target is visible and closer than `tooCloseDistance`, look at
/// it and strafe backwards.
pub struct BackUpIfTooClose {
    too_close_distance: i32,
    strafe_speed: f32,
}

impl BackUpIfTooClose {
    /// Vanilla `BackUpIfTooClose.create(int tooCloseDistance, float strafeSpeed)`.
    #[must_use]
    pub fn create(too_close_distance: i32, strafe_speed: f32) -> OneShot<Self> {
        OneShot::new(Self {
            too_close_distance,
            strafe_speed,
        })
        .with_required_memories(vec![
            &mem::WALK_TARGET,
            &mem::LOOK_TARGET,
            &mem::ATTACK_TARGET,
            &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
        ])
    }
}

impl Trigger for BackUpIfTooClose {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla entry conditions: `absent(WALK_TARGET)`, `present(ATTACK_TARGET)`,
        // `present(NEAREST_VISIBLE_LIVING_ENTITIES)`.
        if ctx.has_memory_value(&mem::WALK_TARGET) {
            return false;
        }

        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        if !utils::closer_than(ctx.body, &target, f64::from(self.too_close_distance))
            || !utils::entity_is_visible(ctx, &target)
        {
            return false;
        }

        utils::set_look_target(ctx, Arc::new(EntityTracker::new(target, true)));

        let body = ctx.body;
        let Some(mob) = body.as_mob() else {
            return false;
        };

        mob.mob_base()
            .controls()
            .lock()
            .move_control
            .strafe(-self.strafe_speed, 0.0);

        // Vanilla `body.setYRot(Mth.rotateIfNecessary(body.getYRot(), body.yHeadRot, 0.0F))`,
        // which is `yHeadRot` because the maximum angle difference is zero.
        let Some(living) = body.as_living_entity() else {
            return false;
        };
        let (_, pitch) = body.rotation();
        body.set_rotation((living.y_head_rot(), pitch));

        true
    }
}
