//! `DecoratedPotBlockEntity` (`net.minecraft.world.level.block.entity.DecoratedPotBlockEntity`).
//!
//! Stores the four sherd slots (vanilla `PotDecorations`: `back`, `left`, `right`, `front`) and the
//! contained item, exposes the ceramic wobble animation on data updates, and reports the
//! containment signal a comparator reads.

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{
    BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Number of sherd slots in a decorated pot.
pub const DECORATED_POT_SHERDS: usize = 4;

/// Vanilla `PotDecorations` slot order: back, left, right, front.
pub const SHERD_SLOT_NAMES: [&str; DECORATED_POT_SHERDS] = ["back", "left", "right", "front"];

/// Mutable decorated-pot state.
#[derive(Default)]
struct DecoratedPotState {
    /// Sherd item in each `SHERD_SLOT_NAMES` slot.
    sherds: [ItemStack; DECORATED_POT_SHERDS],
    /// The single stored item, if any.
    item: ItemStack,
    /// Vanilla `DecoratedPotBlockEntity.wobbleStyle`: the active wobble animation direction.
    wobble: Option<Direction>,
}

/// `DecoratedPotBlockEntity`.
pub struct DecoratedPotBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<DecoratedPotState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `DecoratedPotBlockEntity`.
unsafe impl DowncastType for DecoratedPotBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/decorated_pot");
}

impl DecoratedPotBlockEntity {
    /// Creates a decorated-pot block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::DECORATED_POT,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(DecoratedPotState {
                sherds: std::array::from_fn(|_| ItemStack::empty()),
                item: ItemStack::empty(),
                wobble: None,
            }),
        }
    }

    /// Returns the item stored inside the pot.
    #[must_use]
    pub fn item(&self) -> ItemStack {
        self.state.lock().item.clone()
    }

    /// Replaces the stored item.
    pub fn set_item(&self, item: ItemStack) {
        self.state.lock().item = item;
        BlockEntity::set_changed(self);
    }

    /// Returns the sherd in `SHERD_SLOT_NAMES[index]`.
    #[must_use]
    pub fn sherd(&self, index: usize) -> ItemStack {
        self.state
            .lock()
            .sherds
            .get(index)
            .cloned()
            .unwrap_or_else(ItemStack::empty)
    }

    /// Sets the sherd in `SHERD_SLOT_NAMES[index]`.
    pub fn set_sherd(&self, index: usize, sherd: ItemStack) {
        if let Some(slot) = self.state.lock().sherds.get_mut(index) {
            *slot = sherd;
            BlockEntity::set_changed(self);
        }
    }

    /// Vanilla `DecoratedPotBlockEntity.getAnalogOutputSignal`: 1 while an item is stored.
    #[must_use]
    pub fn analog_output_signal(&self) -> i32 {
        i32::from(!self.state.lock().item.is_empty())
    }

    /// Vanilla `DecoratedPotBlockEntity.wobble(WobbleStyle.NEGATIVE)`: starts the rejection
    /// animation and syncs it to clients.
    pub fn wobble_negative(&self) {
        self.wobble(Direction::Down);
    }

    /// Vanilla `DecoratedPotBlockEntity.wobble(WobbleStyle.POSITIVE)`.
    pub fn wobble_positive(&self) {
        self.wobble(Direction::Up);
    }

    /// Vanilla `DecoratedPotBlockEntity.wobble`: records the animation direction and marks the
    /// entity changed so the client receives the update tag.
    pub fn wobble(&self, direction: Direction) {
        self.state.lock().wobble = Some(direction);
        BlockEntity::set_changed(self);
    }

    /// Vanilla `DecoratedPotBlockEntity.getWobbleStyle`.
    #[must_use]
    pub fn wobble_direction(&self) -> Option<Direction> {
        self.state.lock().wobble
    }

    /// Vanilla `DecoratedPotBlockEntity.tryInsert`: stores one item from the interacting hand when
    /// the pot is empty.
    ///
    /// Returns `true` when the item was inserted.
    pub fn try_insert(
        &self,
        world: &std::sync::Arc<World>,
        pos: BlockPos,
        player: &crate::player::Player,
        inv: &mut crate::behavior::InventoryAccess,
    ) -> bool {
        if !self.state.lock().item.is_empty() {
            return false;
        }
        let inserted = inv.with_item(|stack| {
            if stack.is_empty() {
                return ItemStack::empty();
            }
            let one = stack.copy_with_count(1);
            stack.shrink_one();
            one
        });
        if inserted.is_empty() {
            return false;
        }
        self.set_item(inserted);
        self.wobble_positive();
        world.game_event(
            &steel_registry::vanilla_game_events::BLOCK_CHANGE,
            pos,
            &crate::world::game_event::GameEventContext::new(
                Some(player as &dyn crate::entity::Entity),
                None,
            ),
        );
        true
    }
}

impl BlockEntity for DecoratedPotBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();

        state.item = view
            .compound("item")
            .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
            .unwrap_or_else(ItemStack::empty);

        state.sherds = std::array::from_fn(|_| ItemStack::empty());
        if let Some(sherds) = view.compound("sherds") {
            for (index, name) in SHERD_SLOT_NAMES.iter().enumerate() {
                if let Some(compound) = sherds.compound(name)
                    && let Some(item) = ItemStack::from_borrowed_compound(&compound)
                {
                    state.sherds[index] = item;
                }
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        if !state.item.is_empty() {
            nbt.insert("item", state.item.to_nbt_tag_ref());
        }

        let mut sherds = NbtCompound::new();
        for (index, name) in SHERD_SLOT_NAMES.iter().enumerate() {
            let sherd = &state.sherds[index];
            if !sherd.is_empty() {
                sherds.insert(*name, sherd.to_nbt_tag_ref());
            }
        }
        if !sherds.is_empty() {
            nbt.insert("sherds", sherds);
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }
}
