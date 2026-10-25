use std::borrow::Cow;
use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_utils::Direction;
use text_components::TextComponent;

use crate::behavior::ItemBehavior;
use crate::behavior::items::ProjectileItem;
use crate::entity::entities::objects::projectiles::{AbstractArrow as _, ArrowEntity, Pickup};
use crate::entity::{SharedEntity, next_entity_id};
use crate::world::World;

use super::dynamic_name::potion_name;

/// Tipped-arrow behavior providing Vanilla's potion-content-dependent name.
// TODO: Add TippedArrowItem's poison default instance when Steel has
// item-specific default-stack factories.
//
// Vanilla `TippedArrowItem extends ArrowItem`, so a tipped arrow inherits
// `ArrowItem`'s `ProjectileItem` half: the dispensed projectile is a plain
// arrow entity whose item stack still carries the potion contents.
#[item_behavior]
pub struct TippedArrowItem;

impl ItemBehavior for TippedArrowItem {
    fn get_name<'a>(&self, stack: &'a ItemStack) -> Cow<'a, TextComponent> {
        potion_name(stack)
    }

    fn as_projectile_item(&self) -> Option<&dyn ProjectileItem> {
        Some(self)
    }
}

impl ProjectileItem for TippedArrowItem {
    /// Vanilla `ArrowItem.asProjectile`: an arrow at the requested position
    /// carrying `stack` (count clamped to one) with pickup allowed.
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
        arrow.init_arrow(stack.copy_with_count(1), None, None);
        arrow.abstract_arrow_base().set_pickup(Pickup::Allowed);
        Some(Arc::new(arrow))
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::data_components::components::PotionContents;
    use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        RegistryReference, init_vanilla_registry, vanilla_entities, vanilla_items, vanilla_potions,
    };
    use steel_utils::Downcast as _;

    use crate::behavior::ItemBehavior;
    use crate::behavior::items::{DispenseConfig, ProjectileItem};
    use crate::entity::entities::objects::projectiles::{AbstractArrow as _, ArrowEntity, Pickup};

    /// Vanilla `TippedArrowItem extends ArrowItem`: the dispensed projectile
    /// carries the tipped stack (potion contents included) with pickup allowed.
    #[test]
    fn tipped_arrow_dispenses_as_a_tipped_arrow() {
        init_vanilla_registry();
        let mut stack = ItemStack::new(&vanilla_items::TIPPED_ARROW);
        stack.set(
            POTION_CONTENTS,
            PotionContents::new(
                Some(RegistryReference::new(&vanilla_potions::POISON)),
                None,
                Vec::new(),
                None,
            ),
        );

        let behavior = super::TippedArrowItem;
        let projectile_item = behavior
            .as_projectile_item()
            .expect("tipped arrows dispense as projectiles");
        // A tipped arrow inherits the plain arrow's default shoot parameters.
        assert_eq!(
            projectile_item.create_dispense_config(),
            DispenseConfig::DEFAULT
        );

        let world = crate::test_support::test_world();
        let position = glam::DVec3::new(8.5, 64.5, 8.5);
        let projectile = behavior
            .as_projectile(world, position, &stack, steel_utils::Direction::North)
            .expect("tipped arrow dispenses an entity");
        assert_eq!(projectile.entity_type(), &vanilla_entities::ARROW);
        assert_eq!(projectile.position(), position);
        let arrow = projectile
            .downcast_ref::<ArrowEntity>()
            .expect("tipped arrow dispenses the arrow entity");
        assert_eq!(arrow.abstract_arrow_base().pickup(), Pickup::Allowed);
        assert_eq!(
            arrow.abstract_arrow_base().pickup_item_stack(),
            stack.copy_with_count(1),
            "the dispensed arrow keeps the tipped stack, potion contents included"
        );
    }
}
