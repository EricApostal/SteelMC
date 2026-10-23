//! Vanilla `ProjectileItem` — the item-to-projectile bridge used by dispensers.
//!
//! Mirrors `net.minecraft.world.item.ProjectileItem`: an item that can be
//! dispensed as a projectile describes how to build the entity
//! ([`ProjectileItem::as_projectile`]) and how a dispenser should shoot it
//! ([`ProjectileItem::create_dispense_config`]). `ProjectileDispenseBehavior`
//! then spawns the entity through `spawn_projectile_using_shoot`.
//!
//! This trait is the dispenser hook: `ItemBehavior::as_projectile_item` returns
//! it for any item that implements it, so the container slice never has to know
//! concrete item types.

use std::sync::Arc;

use glam::DVec3;
use steel_registry::item_stack::ItemStack;
use steel_utils::Direction;

use crate::behavior::ItemBehavior;
use crate::entity::SharedEntity;
use crate::world::World;

/// Vanilla `ProjectileItem.DispenseConfig.DEFAULT`.
pub const DEFAULT_DISPENSE_POWER: f32 = 1.1;
/// Vanilla `ProjectileItem.DispenseConfig.DEFAULT` uncertainty.
pub const DEFAULT_DISPENSE_UNCERTAINTY: f32 = 6.0;

/// Vanilla `ProjectileItem.DispenseConfig`: how a dispenser shoots this item's
/// projectile. The dispense position is the dispenser block's business
/// (`DispenserBlock.getDispensePosition`), so only the shot parameters live here.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DispenseConfig {
    /// Shot power (vanilla `power`).
    pub power: f32,
    /// Shot uncertainty (vanilla `uncertainty`).
    pub uncertainty: f32,
    /// Vanilla `overrideDispenseEvent`, played instead of the default `1002`.
    pub override_dispense_event: Option<i32>,
}

impl DispenseConfig {
    /// Vanilla `DispenseConfig.DEFAULT`.
    pub const DEFAULT: Self = Self {
        power: DEFAULT_DISPENSE_POWER,
        uncertainty: DEFAULT_DISPENSE_UNCERTAINTY,
        override_dispense_event: None,
    };

    /// Returns the config used by thrown potions and experience bottles
    /// (vanilla `uncertainty * 0.5`, `power * 1.25`).
    #[must_use]
    pub fn throwable_potion() -> Self {
        Self {
            power: DEFAULT_DISPENSE_POWER * 1.25,
            uncertainty: DEFAULT_DISPENSE_UNCERTAINTY * 0.5,
            override_dispense_event: None,
        }
    }
}

impl Default for DispenseConfig {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Vanilla `ProjectileItem`.
pub trait ProjectileItem: ItemBehavior {
    /// Builds the projectile this item dispenses (vanilla
    /// `asProjectile(level, position, itemStack, direction)`).
    ///
    /// The returned entity must NOT be added to the world; the caller shoots and
    /// adds it. Returns `None` when the item cannot create its projectile (for
    /// example a missing item component), which the dispenser treats as a no-op.
    fn as_projectile(
        &self,
        world: &Arc<World>,
        position: DVec3,
        stack: &ItemStack,
        direction: Direction,
    ) -> Option<SharedEntity>;

    /// Returns the dispenser shot parameters (vanilla `createDispenseConfig`).
    fn create_dispense_config(&self) -> DispenseConfig {
        DispenseConfig::DEFAULT
    }
}
