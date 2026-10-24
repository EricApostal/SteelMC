//! Arrow item behavior (`ArrowItem`) and its spectral sibling.
//!
//! Mirrors vanilla `ArrowItem` / `SpectralArrowItem`: both create an arrow
//! entity carrying the arrow stack (count clamped to 1) at the requested
//! position, with `Pickup::Allowed` when dispensed, and act as the ammo factory
//! used by bows, crossbows and mobs.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::item_stack::ItemStack;
use steel_registry::{vanilla_entities, vanilla_items};
use steel_utils::Direction;

use crate::behavior::ItemBehavior;
use crate::behavior::items::ProjectileItem;
use crate::entity::entities::objects::projectiles::{
    ARROW_SPAWN_EYE_OFFSET, AbstractArrow, ArrowEntity, Pickup, SpectralArrowEntity,
};
use crate::entity::{Entity as _, SharedEntity, next_entity_id};
use crate::world::World;

/// Behavior for the arrow item.
#[item_behavior(class = "ArrowItem")]
pub struct ArrowItem;

impl ItemBehavior for ArrowItem {
    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }
}

impl ProjectileItem for ArrowItem {
    fn as_projectile(
        &self,
        world: &Arc<World>,
        position: DVec3,
        stack: &ItemStack,
        _direction: Direction,
    ) -> Option<SharedEntity> {
        let arrow = ArrowEntity::new(
            &vanilla_entities::ARROW,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        );
        // Vanilla `ArrowItem.asProjectile`: pickup allowed, no firing weapon.
        arrow.init_arrow(stack.copy_with_count(1), None, None);
        arrow.abstract_arrow_base().set_pickup(Pickup::Allowed);
        Some(Arc::new(arrow))
    }
}

/// Behavior for the spectral arrow item.
#[item_behavior(class = "SpectralArrowItem")]
pub struct SpectralArrowItem;

impl ItemBehavior for SpectralArrowItem {
    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }
}

impl ProjectileItem for SpectralArrowItem {
    fn as_projectile(
        &self,
        world: &Arc<World>,
        position: DVec3,
        stack: &ItemStack,
        _direction: Direction,
    ) -> Option<SharedEntity> {
        let arrow = SpectralArrowEntity::new(
            &vanilla_entities::SPECTRAL_ARROW,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        );
        arrow.init_arrow(stack.copy_with_count(1), None, None);
        arrow.abstract_arrow_base().set_pickup(Pickup::Allowed);
        Some(Arc::new(arrow))
    }
}

/// Creates the arrow entity a bow or crossbow fires from `stack`
/// (vanilla `ArrowItem.createArrow`).
///
/// Vanilla's `ArrowItem.createArrow` builds the arrow from the shooter, so the
/// entity spawns at the shooter's eyes minus [`ARROW_SPAWN_EYE_OFFSET`]; a
/// rotation-less spawn origin would drop the arrow at the shooter's feet.
/// Tipped arrows keep their potion contents through the pickup stack; a
/// spectral arrow stack yields a [`SpectralArrowEntity`].
#[must_use]
pub fn create_arrow(
    world: &Arc<World>,
    stack: &ItemStack,
    shooter: &SharedEntity,
    fired_from_weapon: Option<&ItemStack>,
) -> SharedEntity {
    let shooter_position = shooter.position();
    let spawn_position = DVec3::new(
        shooter_position.x,
        shooter.get_eye_y() - ARROW_SPAWN_EYE_OFFSET,
        shooter_position.z,
    );

    if stack.is(&vanilla_items::SPECTRAL_ARROW) {
        let arrow = SpectralArrowEntity::new(
            &vanilla_entities::SPECTRAL_ARROW,
            next_entity_id(),
            spawn_position,
            Arc::downgrade(world),
        );
        arrow.init_arrow(stack.copy_with_count(1), fired_from_weapon, Some(shooter));
        return Arc::new(arrow);
    }

    let arrow = ArrowEntity::new(
        &vanilla_entities::ARROW,
        next_entity_id(),
        spawn_position,
        Arc::downgrade(world),
    );
    arrow.init_arrow(stack.copy_with_count(1), fired_from_weapon, Some(shooter));
    Arc::new(arrow)
}
