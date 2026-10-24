//! Item display entity implementation.
//!
//! Display entities render a block, item, or text without collision.
//! They're commonly used for visual effects, holograms, and decorations.

use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::behavior::PushReaction;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entity_data::ItemDisplayEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use uuid::Uuid;

use super::display_data;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

/// NBT tag holding the displayed item stack.
const TAG_ITEM: &str = "item";
/// NBT tag holding the item display transform.
const TAG_ITEM_DISPLAY: &str = "item_display";

/// How an item display renders its item, mirroring vanilla `ItemDisplayContext`.
///
/// The synced `item_display` value stores vanilla's numeric id, while persisted
/// data stores vanilla's serialized name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ItemDisplayContext {
    /// No display context, the vanilla default.
    #[default]
    None,
    /// Rendered in a third-person left hand.
    ThirdPersonLeftHand,
    /// Rendered in a third-person right hand.
    ThirdPersonRightHand,
    /// Rendered in a first-person left hand.
    FirstPersonLeftHand,
    /// Rendered in a first-person right hand.
    FirstPersonRightHand,
    /// Rendered on a head, such as a player head.
    Head,
    /// Rendered in a GUI slot.
    Gui,
    /// Rendered as an item dropped on the ground.
    Ground,
    /// Rendered as a fixed item, such as in an item frame.
    Fixed,
    /// Rendered on a shelf.
    OnShelf,
}

impl ItemDisplayContext {
    /// Returns the vanilla numeric id stored in the synced `item_display` value.
    #[must_use]
    pub const fn id(self) -> i8 {
        match self {
            Self::None => 0,
            Self::ThirdPersonLeftHand => 1,
            Self::ThirdPersonRightHand => 2,
            Self::FirstPersonLeftHand => 3,
            Self::FirstPersonRightHand => 4,
            Self::Head => 5,
            Self::Gui => 6,
            Self::Ground => 7,
            Self::Fixed => 8,
            Self::OnShelf => 9,
        }
    }

    /// Returns the context for a vanilla numeric id.
    ///
    /// Unknown ids map to [`Self::None`], matching vanilla's out-of-bounds strategy.
    #[must_use]
    pub const fn from_id(id: i8) -> Self {
        match id {
            1 => Self::ThirdPersonLeftHand,
            2 => Self::ThirdPersonRightHand,
            3 => Self::FirstPersonLeftHand,
            4 => Self::FirstPersonRightHand,
            5 => Self::Head,
            6 => Self::Gui,
            7 => Self::Ground,
            8 => Self::Fixed,
            9 => Self::OnShelf,
            _ => Self::None,
        }
    }

    /// Returns vanilla's serialized name for this context.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ThirdPersonLeftHand => "thirdperson_lefthand",
            Self::ThirdPersonRightHand => "thirdperson_righthand",
            Self::FirstPersonLeftHand => "firstperson_lefthand",
            Self::FirstPersonRightHand => "firstperson_righthand",
            Self::Head => "head",
            Self::Gui => "gui",
            Self::Ground => "ground",
            Self::Fixed => "fixed",
            Self::OnShelf => "on_shelf",
        }
    }

    /// Parses vanilla's serialized name, returning `None` for unknown names.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "thirdperson_lefthand" => Some(Self::ThirdPersonLeftHand),
            "thirdperson_righthand" => Some(Self::ThirdPersonRightHand),
            "firstperson_lefthand" => Some(Self::FirstPersonLeftHand),
            "firstperson_righthand" => Some(Self::FirstPersonRightHand),
            "head" => Some(Self::Head),
            "gui" => Some(Self::Gui),
            "ground" => Some(Self::Ground),
            "fixed" => Some(Self::Fixed),
            "on_shelf" => Some(Self::OnShelf),
            _ => None,
        }
    }
}

/// An item display entity that renders an item stack at its position.
///
/// Item displays are purely visual entities with no collision.
/// They support transformation (translation, rotation, scale) and
/// interpolation for smooth animations.
#[entity_behavior(class = "ItemDisplay")]
pub struct ItemDisplayEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced entity data for network serialization.
    entity_data: SyncMutex<ItemDisplayEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ItemDisplayEntity`.
unsafe impl DowncastType for ItemDisplayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/item_display");
}

impl ItemDisplayEntity {
    /// Creates a new item display entity.
    ///
    /// The `id` should be obtained from `next_entity_id()`.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ItemDisplayEntityData::new()),
        }
    }

    /// Creates a new item display entity with a specific UUID.
    ///
    /// The `id` should be obtained from `next_entity_id()`.
    #[must_use]
    pub fn with_uuid(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        uuid: Uuid,
        world: Weak<World>,
    ) -> Self {
        Self {
            base: EntityBase::with_uuid(id, uuid, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ItemDisplayEntityData::new()),
        }
    }

    /// Creates an item display entity from saved data.
    ///
    /// Display entities have no physical collision, but vanilla base state is
    /// still persisted and should round-trip through the shared base.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ItemDisplayEntityData::new()),
        }
    }

    /// Gets a reference to the entity data for reading/modifying synced state.
    pub const fn entity_data(&self) -> &SyncMutex<ItemDisplayEntityData> {
        &self.entity_data
    }

    /// Returns the displayed item stack.
    #[must_use]
    pub fn item_stack(&self) -> ItemStack {
        self.entity_data.lock().item_stack.get().clone()
    }

    /// Sets the displayed item stack.
    pub fn set_item_stack(&self, item_stack: ItemStack) {
        self.entity_data.lock().item_stack.set(item_stack);
    }

    /// Returns how the displayed item is transformed for rendering.
    #[must_use]
    pub fn item_display_context(&self) -> ItemDisplayContext {
        ItemDisplayContext::from_id(*self.entity_data.lock().item_display.get())
    }

    /// Sets how the displayed item is transformed for rendering.
    pub fn set_item_display_context(&self, context: ItemDisplayContext) {
        self.entity_data.lock().item_display.set(context.id());
    }
}

impl Entity for ItemDisplayEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    // Vanilla `Display` disables physics and ignores piston pushes for every display type.
    fn is_ignoring_block_triggers(&self) -> bool {
        true
    }

    fn piston_push_reaction(&self) -> PushReaction {
        PushReaction::IgnoreEntity
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        false
    }

    fn no_physics(&self) -> bool {
        true
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let entity_data = self.entity_data.lock();
        display_data::save(&entity_data.display, nbt);
        let item_stack = entity_data.item_stack.get();
        if !item_stack.is_empty() {
            nbt.insert(TAG_ITEM, item_stack.to_nbt_tag_ref());
        }
        nbt.insert(
            TAG_ITEM_DISPLAY,
            ItemDisplayContext::from_id(*entity_data.item_display.get()).name(),
        );
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let mut entity_data = self.entity_data.lock();
        display_data::load(&mut entity_data.display, nbt);

        if let Some(item_tag) = nbt.compound(TAG_ITEM)
            && let Some(item_stack) = ItemStack::from_borrowed_compound(&item_tag)
        {
            entity_data.item_stack.set(item_stack);
        }

        let context = nbt
            .string(TAG_ITEM_DISPLAY)
            .and_then(|name| ItemDisplayContext::from_name(&name.to_str()))
            .unwrap_or_default();
        entity_data.item_display.set(context.id());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simdnbt::borrow::read_compound;
    use std::io::Cursor;
    use steel_registry::vanilla_entities;
    use steel_registry::vanilla_items;

    const TEST_POSITION: DVec3 = DVec3::new(-8.0, 128.0, 4.0);

    fn new_display() -> ItemDisplayEntity {
        ItemDisplayEntity::new(
            &vanilla_entities::ITEM_DISPLAY,
            1,
            TEST_POSITION,
            Weak::new(),
        )
    }

    #[test]
    fn item_display_round_trips_item_and_transform() {
        let display = new_display();
        display.set_item_stack(ItemStack::new(&vanilla_items::ELYTRA));
        display.set_item_display_context(ItemDisplayContext::Head);

        let mut nbt = NbtCompound::new();
        display.save_additional(&mut nbt);

        assert_eq!(
            nbt.string("item_display")
                .map(|name| name.to_str().into_owned()),
            Some("head".to_owned())
        );
        let Some(item) = nbt.compound("item") else {
            panic!("item display should save the displayed item");
        };
        assert_eq!(item.int("count"), Some(1));

        let bytes = {
            let mut bytes = Vec::new();
            nbt.write(&mut bytes);
            bytes
        };
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));

        let restored = new_display();
        restored.load_additional((&borrowed).into());
        assert_eq!(restored.item_display_context(), ItemDisplayContext::Head);
        assert!(!restored.item_stack().is_empty());
        assert_eq!(restored.item_stack().item, &*vanilla_items::ELYTRA);
    }

    #[test]
    fn item_display_omits_empty_item_and_defaults_unknown_context() {
        let display = new_display();

        let mut nbt = NbtCompound::new();
        display.save_additional(&mut nbt);
        assert!(nbt.compound("item").is_none());
        assert_eq!(
            nbt.string("item_display")
                .map(|name| name.to_str().into_owned()),
            Some("none".to_owned())
        );

        nbt.remove("item_display");
        nbt.insert("item_display", "not_a_context");

        let bytes = {
            let mut bytes = Vec::new();
            nbt.write(&mut bytes);
            bytes
        };
        let borrowed = read_compound(&mut Cursor::new(&bytes))
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));

        let restored = new_display();
        restored.load_additional((&borrowed).into());
        assert_eq!(restored.item_display_context(), ItemDisplayContext::None);
    }

    #[test]
    fn item_display_context_names_and_ids_match_vanilla() {
        for context in [
            ItemDisplayContext::None,
            ItemDisplayContext::ThirdPersonLeftHand,
            ItemDisplayContext::ThirdPersonRightHand,
            ItemDisplayContext::FirstPersonLeftHand,
            ItemDisplayContext::FirstPersonRightHand,
            ItemDisplayContext::Head,
            ItemDisplayContext::Gui,
            ItemDisplayContext::Ground,
            ItemDisplayContext::Fixed,
            ItemDisplayContext::OnShelf,
        ] {
            assert_eq!(ItemDisplayContext::from_id(context.id()), context);
            assert_eq!(ItemDisplayContext::from_name(context.name()), Some(context));
        }
        assert_eq!(ItemDisplayContext::from_id(99), ItemDisplayContext::None);
        assert_eq!(ItemDisplayContext::from_name("unknown"), None);
    }
}
