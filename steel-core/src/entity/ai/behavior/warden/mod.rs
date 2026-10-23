//! Vanilla `net.minecraft.world.entity.ai.behavior.warden` package, plus the
//! `WardenAi` constants the warden activity file configures these behaviours
//! with.
//!
//! The warden entity has not landed in Steel yet, so these behaviours are
//! entity-agnostic: they only read and write [`BehaviorContext`] and the brain
//! memories vanilla's `WardenAi` registers. Two warden subsystems therefore have
//! no counterpart here and their branches are documented at the call sites:
//!
//! - Anger (`Warden.getEntityAngryAt` / `increaseAngerAt` / `AngerManagement`) is
//!   stored on the entity, not in the brain, and no Steel entity exposes it.
//! - Pose sounds and the sonic-boom particles/entity event leave through the
//!   client audio and particle paths, which the AI layer cannot reach.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::entity_type::EntityAttachment;
use steel_registry::vanilla_entities;
use steel_utils::BlockPos;

use crate::entity::ai::brain::behavior::BehaviorContext;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::brain::tracker::BlockPosTracker;
use crate::entity::{Entity as _, LivingEntity, SharedEntity};

pub mod digging;
pub mod emerging;
pub mod force_unmount;
pub mod roar;
pub mod set_roar_target;
pub mod set_warden_look_target;
pub mod sniffing;
pub mod sonic_boom;
pub mod try_to_sniff;

/// Vanilla `WardenAi.DIGGING_DURATION`, `Mth.ceil(100.0F)`.
pub const DIGGING_DURATION: i32 = 100;

/// Vanilla `WardenAi.EMERGE_DURATION`, `Mth.ceil(133.59999F)`.
pub const EMERGE_DURATION: i32 = 134;

/// Vanilla `WardenAi.ROAR_DURATION`, `Mth.ceil(84.0F)`.
pub const ROAR_DURATION: i32 = 84;

/// Vanilla `WardenAi.SNIFFING_DURATION`, `Mth.ceil(83.2F)`.
pub const SNIFFING_DURATION: i32 = 84;

/// Vanilla `WardenAi.DIGGING_COOLDOWN`.
pub const DIGGING_COOLDOWN: i64 = 1200;

/// Vanilla `WardenAi.DISTURBANCE_LOCATION_EXPIRY_TIME`.
pub const DISTURBANCE_LOCATION_EXPIRY: i64 = 100;

/// Vanilla `Entity.closerThan(other, distanceXZ, distanceY)`: horizontal and
/// vertical distances are tested independently.
#[must_use]
pub fn closer_than_with_y(
    entity: &SharedEntity,
    other: &SharedEntity,
    distance_xz: f64,
    distance_y: f64,
) -> bool {
    let delta = other.position() - entity.position();
    delta.x * delta.x + delta.z * delta.z < distance_xz * distance_xz
        && delta.y * delta.y < distance_y * distance_y
}

/// Vanilla `Warden.canTargetEntity`.
///
/// `EntitySelector.NO_CREATIVE_OR_SPECTATOR` is spelled with
/// `Player::has_infinite_materials`/`Player::is_spectator`, the same pair
/// `Mob::is_valid_target` uses.
#[must_use]
pub fn can_target_entity(ctx: &BehaviorContext<'_>, target: &SharedEntity) -> bool {
    let Some(living) = target.as_living_entity() else {
        return false;
    };

    let same_level = match (ctx.body.level(), target.level()) {
        (Some(body_level), Some(target_level)) => Arc::ptr_eq(&body_level, &target_level),
        _ => false,
    };

    same_level
        && !target
            .as_player()
            .is_some_and(|player| player.has_infinite_materials() || player.is_spectator())
        && !ctx.body.is_allied_to(target.as_ref())
        && target.entity_type() != &vanilla_entities::ARMOR_STAND
        && target.entity_type() != &vanilla_entities::WARDEN
        && !target.is_invulnerable()
        && !LivingEntity::is_dead_or_dying(living)
        && ctx
            .level
            .world_border_snapshot()
            .is_within_bounds(target.bounding_box())
}

/// Vanilla `WardenAi.setDigCooldown`: refresh `DIG_COOLDOWN` only when the body
/// already remembers it.
pub fn set_dig_cooldown(ctx: &mut BehaviorContext<'_>) {
    if ctx.has_memory_value(&mem::DIG_COOLDOWN) {
        ctx.set_memory_with_expiry(&mem::DIG_COOLDOWN, (), DIGGING_COOLDOWN);
    }
}

/// Vanilla `WardenAi.setDisturbanceLocation`.
///
/// Vanilla also guards on `!body.getEntityAngryAt().isPresent()`; Steel has no
/// warden anger subsystem, so that guard is trivially satisfied and only the
/// `ATTACK_TARGET` guard remains.
pub fn set_disturbance_location(ctx: &mut BehaviorContext<'_>, position: BlockPos) {
    if !ctx
        .level
        .world_border_snapshot()
        .is_block_within_bounds(position)
        || ctx.has_memory_value(&mem::ATTACK_TARGET)
    {
        return;
    }

    set_dig_cooldown(ctx);
    ctx.set_memory_with_expiry(&mem::SNIFF_COOLDOWN, (), DISTURBANCE_LOCATION_EXPIRY);
    ctx.set_memory_with_expiry(
        &mem::LOOK_TARGET,
        Arc::new(BlockPosTracker::from_block_pos(position)),
        DISTURBANCE_LOCATION_EXPIRY,
    );
    ctx.set_memory_with_expiry(
        &mem::DISTURBANCE_LOCATION,
        position,
        DISTURBANCE_LOCATION_EXPIRY,
    );
    ctx.erase_memory(&mem::WALK_TARGET);
}

/// The brain-visible half of vanilla `Warden.setAttackTarget`.
///
/// `Roar.stop` calls this; the entity method also updates the anger management,
/// which has no Steel counterpart.
pub fn set_attack_target(ctx: &mut BehaviorContext<'_>, target: SharedEntity) {
    ctx.erase_memory(&mem::ROAR_TARGET);
    ctx.set_memory(&mem::ATTACK_TARGET, target);
    ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
    sonic_boom::SonicBoom::set_cooldown(ctx, 200);
}

/// Vanilla `LivingEntity.getEyePosition` from a [`SharedEntity`].
#[must_use]
pub(crate) fn eye_position(entity: &SharedEntity) -> DVec3 {
    let position = entity.position();
    DVec3::new(position.x, entity.get_eye_y(), position.z)
}

/// Vanilla `Entity.getAttachments().get(EntityAttachment.WARDEN_CHEST, 0, yRot)`.
#[must_use]
pub(crate) fn warden_chest_attachment(ctx: &BehaviorContext<'_>) -> DVec3 {
    let dimensions = ctx.body.base().dimensions();
    dimensions.attachments.get_clamped(
        EntityAttachment::WardenChest,
        0,
        ctx.body.rotation().0,
        dimensions,
    )
}
