//! `DecoratedPotBlockEntity` (`net.minecraft.world.level.block.entity.DecoratedPotBlockEntity`).
//!
//! Stores the four sherd slots (vanilla `PotDecorations`: `back`, `left`, `right`, `front`) and the
//! contained item, exposes the ceramic wobble animation on data updates, and reports the
//! containment signal a comparator reads.

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag, ToNbtTag};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::{
    CONTAINER, POT_DECORATIONS, PotDecorations,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::{ItemStackTemplate, vanilla_block_entity_types};
use steel_utils::{
    BlockPos, BlockStateId, Direction, DowncastType, DowncastTypeKey, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Number of sherd slots in a decorated pot.
pub const DECORATED_POT_SHERDS: usize = 4;

/// Vanilla `PotDecorations` slot order: back, left, right, front.
pub const SHERD_SLOT_NAMES: [&str; DECORATED_POT_SHERDS] = ["back", "left", "right", "front"];

/// Vanilla `DecoratedPotBlockEntity.EVENT_POT_WOBBLES`.
const EVENT_POT_WOBBLES: i32 = 1;
/// Vanilla `DecoratedPotBlockEntity.WobbleStyle.POSITIVE` ordinal.
const WOBBLE_POSITIVE: i32 = 0;
/// Vanilla `DecoratedPotBlockEntity.WobbleStyle.NEGATIVE` ordinal.
const WOBBLE_NEGATIVE: i32 = 1;
/// Number of vanilla `WobbleStyle` values.
const WOBBLE_STYLE_COUNT: i32 = 2;

/// Vanilla `ItemStackTemplate.create`: the decoration's item stack, or an empty stack when the
/// decoration side is unset.
fn decoration_stack(decoration: Option<&ItemStackTemplate>) -> ItemStack {
    decoration.map_or_else(ItemStack::empty, ItemStackTemplate::create)
}

/// Mutable decorated-pot state.
#[derive(Default)]
struct DecoratedPotState {
    /// Sherd item in each `SHERD_SLOT_NAMES` slot.
    sherds: [ItemStack; DECORATED_POT_SHERDS],
    /// The single stored item, if any.
    item: ItemStack,
    /// Vanilla `DecoratedPotBlockEntity.lastWobbleStyle`: the active wobble animation direction.
    wobble: Option<Direction>,
    /// Vanilla `DecoratedPotBlockEntity.wobbleStartedAtTick`.
    wobble_started_at_tick: i64,
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
                wobble_started_at_tick: 0,
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

    /// Vanilla `DecoratedPotBlockEntity.getAnalogOutputSignal`, read through
    /// `AbstractContainerMenu.getRedstoneSignalFromBlockEntity`: the single-slot
    /// container's fill ratio is mapped by `Mth.lerpDiscrete(ratio, 0, 15)`.
    #[must_use]
    pub fn analog_output_signal(&self) -> i32 {
        let item = self.state.lock().item.clone();
        if item.is_empty() {
            return 0;
        }
        let max_stack_size = item.max_stack_size();
        if max_stack_size <= 0 {
            return 0;
        }
        let ratio = item.count() as f32 / max_stack_size as f32;
        // `Mth.lerpDiscrete(alpha, 0, 15)` = floor(alpha * 14) + (alpha > 0 ? 1 : 0).
        (ratio * 14.0).floor() as i32 + i32::from(ratio > 0.0)
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

    /// Vanilla `DecoratedPotBlockEntity.wobble`: queues block event 1 with the
    /// `WobbleStyle` ordinal. The animation itself is applied in
    /// [`BlockEntity::trigger_event`] when the event is dispatched.
    pub fn wobble(&self, direction: Direction) {
        let Some(world) = self.get_level() else {
            return;
        };
        let style = match direction {
            Direction::Up => WOBBLE_POSITIVE,
            _ => WOBBLE_NEGATIVE,
        };
        world.block_event(
            self.get_block_pos(),
            self.get_block_state().get_block(),
            EVENT_POT_WOBBLES,
            style,
        );
    }

    /// Vanilla `DecoratedPotBlockEntity.getWobbleStyle`.
    #[must_use]
    pub fn wobble_direction(&self) -> Option<Direction> {
        self.state.lock().wobble
    }

    /// Vanilla `DecoratedPotBlockEntity.wobbleStartedAtTick`: the game time the
    /// current wobble animation started at.
    #[must_use]
    pub fn wobble_started_at_tick(&self) -> i64 {
        self.state.lock().wobble_started_at_tick
    }

    /// Vanilla `DecoratedPotBlock.useItemOn`: stores one item from the interacting hand when the
    /// pot is empty, or stacks a matching item up to the pot item's max stack size, then plays
    /// the insert sound and dust plume and wobbles positively.
    ///
    /// Returns `true` when the item was inserted.
    pub fn try_insert(
        &self,
        world: &std::sync::Arc<World>,
        pos: BlockPos,
        player: &crate::player::Player,
        inv: &mut crate::behavior::InventoryAccess,
    ) -> bool {
        let pot_item = self.item();
        let hand_item = inv.with_item(|stack| stack.copy_with_count(stack.count()));
        if hand_item.is_empty() {
            return false;
        }

        let pot_is_empty = pot_item.is_empty();
        if !pot_is_empty
            && !(ItemStack::is_same_item_same_components(&pot_item, &hand_item)
                && pot_item.count() < pot_item.max_stack_size())
        {
            return false;
        }

        let awarded =
            inv.with_item(|stack| stack.consume_and_return(1, player.has_infinite_materials()));
        if awarded.is_empty() {
            return false;
        }

        let pitch_bend = if pot_is_empty {
            let count = awarded.count();
            let max_stack_size = awarded.max_stack_size();
            self.set_item(awarded);
            count as f32 / max_stack_size as f32
        } else {
            let mut stacked = pot_item;
            stacked.grow(1);
            let count = stacked.count();
            let max_stack_size = stacked.max_stack_size();
            self.set_item(stacked);
            count as f32 / max_stack_size as f32
        };

        world.play_sound(
            &steel_registry::sound_events::BLOCK_DECORATED_POT_INSERT,
            SoundSource::Blocks,
            pos,
            1.0,
            0.7 + 0.5 * pitch_bend,
            None,
        );
        world.send_particles(
            ParticleData::simple(&steel_registry::vanilla_particle_types::DUST_PLUME),
            glam::DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 1.2,
                f64::from(pos.z()) + 0.5,
            ),
            7,
            glam::DVec3::ZERO,
            0.0,
        );

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

    /// Vanilla `BlockEntity.preRemoveSideEffects` (the `Container` branch): breaking a pot drops
    /// the item stored inside it, in addition to the block's own drops.
    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let item = std::mem::take(&mut self.state.lock().item);
        let Some(world) = self.get_level() else {
            return;
        };
        world.drop_item_stack(pos, item);
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        // Vanilla `input.read("sherds", PotDecorations.CODEC).orElse(PotDecorations.EMPTY)`: the
        // whole decorations value is discarded when any side fails to decode.
        let decorations = view
            .get("sherds")
            .and_then(PotDecorations::from_nbt_tag)
            .unwrap_or(PotDecorations::EMPTY);

        let mut state = self.state.lock();

        state.item = view
            .compound("item")
            .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
            .unwrap_or_else(ItemStack::empty);

        // `SHERD_SLOT_NAMES` is vanilla `PotDecorations` order: back, left, right, front.
        state.sherds = [
            decoration_stack(decorations.back()),
            decoration_stack(decorations.left()),
            decoration_stack(decorations.right()),
            decoration_stack(decorations.front()),
        ];
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        if !state.item.is_empty() {
            nbt.insert("item", state.item.to_nbt_tag_ref());
        }

        let mut sherds = NbtCompound::new();
        for (index, name) in SHERD_SLOT_NAMES.iter().enumerate() {
            let sherd = &state.sherds[index];
            if sherd.is_empty() {
                continue;
            }
            // Vanilla stores `PotDecorations` as `ItemStackTemplate`s, whose codec omits
            // `count` when it is 1.
            if let Ok(template) = ItemStackTemplate::from_stack(sherd) {
                sherds.insert(*name, template.to_nbt_tag());
            }
        }
        if !sherds.is_empty() {
            nbt.insert("sherds", sherds);
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `DecoratedPotBlockEntity.getUpdateTag`: `saveCustomOnly`.
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    /// Vanilla `DecoratedPotBlockEntity.applyImplicitComponents`: the placed pot takes its
    /// decorations from the `minecraft:pot_decorations` component and its stored item from the
    /// first slot of the `minecraft:container` component (`ItemContainerContents.copyOne`).
    fn apply_components_from_item(&self, item: &ItemStack) {
        let decorations = item.get_or_default(POT_DECORATIONS, PotDecorations::EMPTY);
        let stored_item = item
            .get(CONTAINER)
            .and_then(|contents| contents.items().first())
            .and_then(Option::as_ref)
            .map_or_else(ItemStack::empty, ItemStackTemplate::create);

        let mut state = self.state.lock();
        // `SHERD_SLOT_NAMES` is vanilla `PotDecorations` order: back, left, right, front.
        state.sherds = [
            decoration_stack(decorations.back()),
            decoration_stack(decorations.left()),
            decoration_stack(decorations.right()),
            decoration_stack(decorations.front()),
        ];
        state.item = stored_item;
    }

    /// Vanilla `DecoratedPotBlockEntity.triggerEvent`: event 1 starts the wobble
    /// animation at the current game time and returns `true` so the server
    /// broadcasts the block event to nearby clients.
    fn trigger_event(&self, event: i32, data: i32) -> bool {
        let Some(world) = self.get_level() else {
            return false;
        };
        if event != EVENT_POT_WOBBLES || !(0..WOBBLE_STYLE_COUNT).contains(&data) {
            return false;
        }

        let game_time = world.game_time();
        let mut state = self.state.lock();
        state.wobble_started_at_tick = game_time;
        state.wobble = Some(if data == WOBBLE_POSITIVE {
            Direction::Up
        } else {
            Direction::Down
        });
        true
    }
}
