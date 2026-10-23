//! Vanilla `monster/breeze/Shoot`.
//!
//! **Dependency**: vanilla releases the bolt through
//! `Projectile.spawnProjectileUsingShoot(new BreezeWindCharge(breeze, level), …)`,
//! which needs the breeze wind charge factory. Steel registers
//! `BreezeWindChargeEntity`, but the entity type only spawns when its factory is
//! registered (`ENTITIES.has_factory`), exactly like `CrossbowAttack` gates its
//! bolt on the arrow factory: the charge/aim/recover state machine below is
//! complete and only the projectile release is gated.

use glam::DVec3;
use steel_registry::entity_data::EntityPose;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_events;
use steel_registry::vanilla_entities;

use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::{MemoryStatus, module_type as mem};
use crate::entity::entities::objects::projectiles::BreezeWindChargeEntity;
use crate::entity::projectile::spawn_projectile_using_shoot;
use crate::entity::registry::ENTITIES;
use crate::entity::{EntityAnchor, SharedEntity, next_entity_id};

/// Vanilla `Shoot.ATTACK_RANGE_MAX_SQRT`.
const ATTACK_RANGE_MAX_SQRT: f64 = 256.0;
/// Vanilla `Shoot.PROJECTILE_MOVEMENT_SCALE`.
const PROJECTILE_MOVEMENT_SCALE: f32 = 0.7;
/// Vanilla `Shoot.UNCERTAINTY_BASE`.
const UNCERTAINTY_BASE: i32 = 5;
/// Vanilla `Shoot.UNCERTAINTY_MULTIPLIER`.
const UNCERTAINTY_MULTIPLIER: i32 = 4;
/// Vanilla `Shoot.SHOOT_INITIAL_DELAY_TICKS`, `Math.round(15.0F)`.
const SHOOT_INITIAL_DELAY_TICKS: i64 = 15;
/// Vanilla `Shoot.SHOOT_RECOVER_DELAY_TICKS`, `Math.round(4.0F)`.
const SHOOT_RECOVER_DELAY_TICKS: i64 = 4;
/// Vanilla `Shoot.SHOOT_COOLDOWN_TICKS`, `Math.round(10.0F)`.
const SHOOT_COOLDOWN_TICKS: i64 = 10;

/// Vanilla `Shoot extends Behavior<Breeze>`.
pub struct Shoot {
    state: BehaviorState,
}

impl Shoot {
    /// Vanilla `new Shoot()`.
    #[must_use]
    pub fn new() -> Self {
        let timeout = SHOOT_INITIAL_DELAY_TICKS as i32 + 1 + SHOOT_RECOVER_DELAY_TICKS as i32;
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::ATTACK_TARGET, MemoryStatus::ValuePresent),
                    (&mem::BREEZE_SHOOT_COOLDOWN, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_SHOOT_CHARGING, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_SHOOT_RECOVERING, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_SHOOT, MemoryStatus::ValuePresent),
                    (&mem::WALK_TARGET, MemoryStatus::ValueAbsent),
                    (&mem::BREEZE_JUMP_TARGET, MemoryStatus::ValueAbsent),
                ],
                timeout,
                timeout,
            ),
        }
    }
}

impl Default for Shoot {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for Shoot {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.body.pose() != EntityPose::Standing {
            return false;
        }

        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return false;
        };

        if is_target_within_range(ctx.body, &target) {
            return true;
        }

        ctx.erase_memory(&mem::BREEZE_SHOOT);
        false
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        ctx.has_memory_value(&mem::ATTACK_TARGET) && ctx.has_memory_value(&mem::BREEZE_SHOOT)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.has_memory_value(&mem::ATTACK_TARGET) {
            ctx.body.set_pose(EntityPose::Shooting);
        }
        ctx.set_memory_with_expiry(&mem::BREEZE_SHOOT_CHARGING, (), SHOOT_INITIAL_DELAY_TICKS);
        ctx.body
            .play_sound(&sound_events::ENTITY_BREEZE_INHALE, 1.0, 1.0);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.body.pose() == EntityPose::Shooting {
            ctx.body.set_pose(EntityPose::Standing);
        }

        ctx.set_memory_with_expiry(&mem::BREEZE_SHOOT_COOLDOWN, (), SHOOT_COOLDOWN_TICKS);
        ctx.erase_memory(&mem::BREEZE_SHOOT);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::ATTACK_TARGET) else {
            return;
        };

        ctx.body.look_at(EntityAnchor::Eyes, target.position());
        if ctx.has_memory_value(&mem::BREEZE_SHOOT_CHARGING)
            || ctx.has_memory_value(&mem::BREEZE_SHOOT_RECOVERING)
        {
            return;
        }

        ctx.set_memory_with_expiry(&mem::BREEZE_SHOOT_RECOVERING, (), SHOOT_RECOVER_DELAY_TICKS);

        let origin = ctx.body.position();
        let target_pos = target.position();
        let firing_y = target_pos.y;
        let direction = DVec3::new(
            target_pos.x - origin.x,
            firing_y - origin.y,
            target_pos.z - origin.z,
        );
        let uncertainty =
            UNCERTAINTY_BASE - i32::from(u8::from(ctx.level.difficulty())) * UNCERTAINTY_MULTIPLIER;

        // **Dependency**: vanilla spawns `BreezeWindCharge` through
        // `Projectile.spawnProjectileUsingShoot`; the release is gated on the
        // breeze wind charge factory being registered.
        if !ENTITIES.has_factory(&vanilla_entities::BREEZE_WIND_CHARGE) {
            return;
        }

        let Some(world) = ctx.body.level() else {
            return;
        };
        let projectile = BreezeWindChargeEntity::with_direction(
            &vanilla_entities::BREEZE_WIND_CHARGE,
            next_entity_id(),
            origin,
            direction,
            std::sync::Arc::downgrade(&world),
        );
        let mut item_stack = ItemStack::empty();
        spawn_projectile_using_shoot(
            &world,
            projectile,
            &mut item_stack,
            direction,
            PROJECTILE_MOVEMENT_SCALE,
            uncertainty as f32,
        );
        ctx.body
            .play_sound(&sound_events::ENTITY_BREEZE_SHOOT, 1.5, 1.0);
    }
}

/// Vanilla `Shoot.isTargetWithinRange`.
pub(super) fn is_target_within_range(body: &SharedEntity, target: &SharedEntity) -> bool {
    body.distance_to_sqr(target.position()) < ATTACK_RANGE_MAX_SQRT
}
