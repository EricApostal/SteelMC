//! `CommandBlockEntity` (`net.minecraft.world.level.block.entity.CommandBlockEntity`).
//!
//! Vanilla keeps the command text, success count, last output and execution bookkeeping in a
//! shared `BaseCommandBlock`, plus the `powered`/`auto`/`conditionMet` flags on the entity. The
//! three command-block kinds differ only by `Mode` (`REDSTONE`, `AUTO`, `SEQUENCE`).

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag as _, ToNbtTag as _};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, locks::SyncMutex,
};
use text_components::TextComponent;

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Vanilla `CommandBlockEntity.Mode`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandBlockMode {
    /// Impulse command block: executes once per redstone pulse.
    Redstone,
    /// Repeating command block: executes every tick while powered.
    Auto,
    /// Chain command block: executes when the previous block in the chain succeeds.
    Sequence,
}

/// Vanilla `CommandBlock instanceof CommandBlock`: all three command-block kinds.
fn is_command_block(block: steel_registry::blocks::BlockRef) -> bool {
    block == &vanilla_blocks::COMMAND_BLOCK
        || block == &vanilla_blocks::REPEATING_COMMAND_BLOCK
        || block == &vanilla_blocks::CHAIN_COMMAND_BLOCK
}

/// Vanilla `BaseCommandBlock` + `CommandBlockEntity` state.
struct CommandBlockState {
    command: String,
    /// Vanilla `CommandBlockEntity.getMode`, derived from and kept in sync with the block kind.
    mode: CommandBlockMode,
    /// Vanilla `BaseCommandBlock.successCount`.
    success_count: i32,
    /// Vanilla `BaseCommandBlock.lastOutput`.
    last_output: Option<TextComponent>,
    /// Vanilla `BaseCommandBlock.trackOutput`.
    track_output: bool,
    /// Vanilla `BaseCommandBlock.updateLastExecution`.
    update_last_execution: bool,
    /// Vanilla `BaseCommandBlock.lastExecution`.
    last_execution: i64,
    /// Whether the last redstone state was powered.
    powered: bool,
    /// Whether the block executes automatically (`chain_command_block`).
    auto: bool,
    /// Vanilla `conditionMet`.
    condition_met: bool,
}

impl Default for CommandBlockState {
    fn default() -> Self {
        Self {
            command: String::new(),
            mode: CommandBlockMode::Redstone,
            success_count: 0,
            last_output: None,
            track_output: true,
            // Vanilla `BaseCommandBlock`: `updateLastExecution = true`.
            update_last_execution: true,
            last_execution: -1,
            powered: false,
            auto: false,
            condition_met: false,
        }
    }
}

/// `CommandBlockEntity`.
pub struct CommandBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<CommandBlockState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CommandBlockEntity`.
unsafe impl DowncastType for CommandBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/command_block");
}

impl CommandBlockEntity {
    /// Creates a command-block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        // Vanilla `CommandBlock.newBlockEntity` applies the block kind: `getMode` is derived from
        // the block, and only the chain command block is registered with `automatic = true`.
        let mode = Self::mode_for_block(state.get_block());
        let automatic = state.get_block() == &vanilla_blocks::CHAIN_COMMAND_BLOCK;
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::COMMAND_BLOCK,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(CommandBlockState {
                mode,
                auto: automatic,
                ..CommandBlockState::default()
            }),
        }
    }

    /// Vanilla `CommandBlockEntity.getMode`: the block kind determines the mode.
    fn mode_for_block(block: steel_registry::blocks::BlockRef) -> CommandBlockMode {
        if block == &vanilla_blocks::REPEATING_COMMAND_BLOCK {
            CommandBlockMode::Auto
        } else if block == &vanilla_blocks::CHAIN_COMMAND_BLOCK {
            CommandBlockMode::Sequence
        } else {
            CommandBlockMode::Redstone
        }
    }

    /// Vanilla `CommandBlockEntity.setAutomatic`.
    pub fn set_automatic(&self, automatic: bool) {
        let previous_automatic = {
            let mut state = self.state.lock();
            let previous = state.auto;
            state.auto = automatic;
            previous
        };

        if previous_automatic
            || !automatic
            || self.is_powered()
            || Self::mode_for_block(self.get_block_state().get_block())
                == CommandBlockMode::Sequence
        {
            return;
        }

        self.schedule_tick();
    }

    /// Vanilla `CommandBlockEntity.scheduleTick`.
    fn schedule_tick(&self) {
        let block = self.get_block_state().get_block();
        if !is_command_block(block) {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        self.mark_condition_met();
        world.schedule_block_tick_default(self.get_block_pos(), block, 1);
    }

    /// Returns whether this block executes automatically.
    ///
    /// Vanilla registers only `chain_command_block` with `automatic = true`.
    #[must_use]
    pub fn is_automatic(&self) -> bool {
        self.state.lock().auto
    }

    /// Vanilla `CommandBlockEntity.getMode`.
    #[must_use]
    pub fn mode(&self) -> CommandBlockMode {
        self.state.lock().mode
    }

    /// Vanilla `CommandBlockEntity.setPowered`. Returns the previous state.
    #[must_use]
    pub fn set_powered(&self, powered: bool) -> bool {
        let mut state = self.state.lock();
        let previous = state.powered;
        state.powered = powered;
        previous
    }

    /// Vanilla `CommandBlockEntity.isPowered`.
    #[must_use]
    pub fn is_powered(&self) -> bool {
        self.state.lock().powered
    }

    /// Vanilla `BaseCommandBlock.getSuccessCount`.
    #[must_use]
    pub fn success_count(&self) -> i32 {
        self.state.lock().success_count
    }

    /// Vanilla `BaseCommandBlock.setSuccessCount`.
    pub fn set_success_count(&self, success_count: i32) {
        self.state.lock().success_count = success_count;
    }

    /// Vanilla `BaseCommandBlock.getCommand`.
    #[must_use]
    pub fn command(&self) -> String {
        self.state.lock().command.clone()
    }

    /// Vanilla `BaseCommandBlock.setCommand`: also clears the previous success count.
    pub fn set_command(&self, command: impl Into<String>) {
        {
            let mut state = self.state.lock();
            state.command = command.into();
            state.success_count = 0;
        }
        BlockEntity::set_changed(self);
    }

    /// Vanilla `CommandBlockEntity.getMode` after the block kind changed, set by
    /// `handleSetCommandBlock` once the block state has been swapped.
    pub fn set_mode(&self, mode: CommandBlockMode) {
        self.state.lock().mode = mode;
    }

    /// Vanilla `BaseCommandBlock.setTrackOutput`.
    pub fn set_track_output(&self, track_output: bool) {
        self.state.lock().track_output = track_output;
    }

    /// Vanilla `BaseCommandBlock.setLastOutput`.
    pub fn set_last_output(&self, last_output: Option<TextComponent>) {
        self.state.lock().last_output = last_output;
    }

    /// Vanilla `CommandBlockEntity.getCommandBlock().onUpdated`: re-sends the block and its
    /// update tag because the command data changed without a block state change.
    pub fn on_updated(&self, world: &std::sync::Arc<World>) {
        world.send_block_updated(self.get_block_pos());
    }

    /// Vanilla `CommandBlockEntity.onModeSwitch`.
    pub fn on_mode_switch(&self, _world: &std::sync::Arc<World>) {
        let (mode, powered, auto) = {
            let state = self.state.lock();
            (state.mode, state.powered, state.auto)
        };
        if mode == CommandBlockMode::Auto && (powered || auto) {
            self.schedule_tick();
        }
    }

    /// Vanilla `CommandBlockEntity.isConditional`.
    #[must_use]
    pub fn is_conditional(&self) -> bool {
        self.get_block_state()
            .try_get_value(&BlockStateProperties::CONDITIONAL)
            == Some(true)
    }

    /// Vanilla `CommandBlockEntity.markConditionMet`.
    pub fn mark_condition_met(&self) -> bool {
        let condition_met = if !self.is_conditional() {
            true
        } else if let Some(world) = self.get_level() {
            let relative = self.get_block_pos().relative(
                self.get_block_state()
                    .get_value(&BlockStateProperties::FACING)
                    .opposite(),
            );
            if is_command_block(world.get_block_state(relative).get_block()) {
                let block_entity = world.get_block_entity(relative);
                block_entity
                    .as_ref()
                    .and_then(|entity| entity.downcast_ref::<CommandBlockEntity>())
                    .is_some_and(|entity| entity.success_count() > 0)
            } else {
                false
            }
        } else {
            // Vanilla needs a level to inspect the preceding block; a detached entity can never
            // satisfy a conditional chain.
            false
        };

        self.state.lock().condition_met = condition_met;
        condition_met
    }

    /// Vanilla `CommandBlockEntity.wasConditionMet`.
    #[must_use]
    pub fn was_condition_met(&self) -> bool {
        self.state.lock().condition_met
    }

    /// Vanilla `CommandBlock.tick`: mode-dependent execution.
    ///
    /// The command dispatch itself needs an `Arc<Server>` handle that `World` does not expose, so
    /// the execute calls are the one missing foundation (see the TODOs below).
    pub fn server_tick(
        &self,
        world: &std::sync::Arc<World>,
        pos: BlockPos,
        block: steel_registry::blocks::BlockRef,
    ) {
        let (mode, was_condition_met, powered, auto) = {
            let state = self.state.lock();
            (state.mode, state.condition_met, state.powered, state.auto)
        };

        match mode {
            CommandBlockMode::Auto => {
                self.mark_condition_met();
                if was_condition_met {
                    // TODO(Foundations): `execute` — dispatch `self.command()` through
                    // `Server::submit_command` and accumulate the callback's success count. A
                    // block entity has no `Arc<Server>` handle (`World` exposes none), so the
                    // dispatcher call needs a world→server accessor (owner: Main /
                    // ServerFoundations).
                } else if self.is_conditional() {
                    self.set_success_count(0);
                }

                if powered || auto {
                    self.schedule_tick();
                }
            }
            CommandBlockMode::Redstone => {
                if was_condition_met {
                    // TODO(Foundations): same dispatcher dependency as the `Auto` branch.
                } else if self.is_conditional() {
                    self.set_success_count(0);
                }
            }
            CommandBlockMode::Sequence => {}
        }

        // Vanilla `CommandBlock.tick` ends with `updateNeighbourForOutputSignal`, not a full
        // neighbor update.
        world.update_neighbor_for_output_signal(pos, block);
    }
}

impl BlockEntity for CommandBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let automatic = view.byte("auto").unwrap_or(0) != 0;
        {
            let mut state = self.state.lock();
            // Vanilla `BaseCommandBlock.load`.
            state.command = view
                .string("Command")
                .map(|v| v.to_string())
                .unwrap_or_default();
            state.success_count = view.int("SuccessCount").unwrap_or(0);
            state.track_output = view.byte("TrackOutput").unwrap_or(1) != 0;
            state.last_output = if state.track_output {
                TextComponent::from_optional_nbt_tag(view.get("LastOutput"))
                    .ok()
                    .flatten()
            } else {
                None
            };
            state.update_last_execution = view.byte("UpdateLastExecution").unwrap_or(1) != 0;
            state.last_execution = if state.update_last_execution {
                view.long("LastExecution").unwrap_or(-1)
            } else {
                -1
            };

            // Vanilla `CommandBlockEntity.loadAdditional`.
            state.powered = view.byte("powered").unwrap_or(0) != 0;
            state.condition_met = view.byte("conditionMet").unwrap_or(0) != 0;
        }

        // Vanilla calls `setAutomatic`, which can schedule the first execution when the stored
        // block is powered and not yet running automatically.
        self.set_automatic(automatic);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        // Vanilla `BaseCommandBlock.save`: `LastOutput` is only written while output tracking is
        // enabled, and `LastExecution` only when execution bookkeeping is enabled.
        nbt.insert("Command", state.command.clone());
        nbt.insert("SuccessCount", state.success_count);
        nbt.insert("TrackOutput", i8::from(state.track_output));
        if state.track_output
            && let Some(output) = state.last_output.clone().to_optional_nbt_tag()
        {
            nbt.insert("LastOutput", output);
        }
        nbt.insert("UpdateLastExecution", i8::from(state.update_last_execution));
        if state.update_last_execution && state.last_execution != -1 {
            nbt.insert("LastExecution", state.last_execution);
        }
        // Vanilla `CommandBlockEntity.saveAdditional`.
        nbt.insert("powered", i8::from(state.powered));
        nbt.insert("conditionMet", i8::from(state.condition_met));
        nbt.insert("auto", i8::from(state.auto));
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `CommandBlockEntity` overrides neither `getUpdateTag` nor `getUpdatePacket`, so
        // the chunk packet drops its empty tag and the editor data is only sent when a
        // game-master opens the block (`ServerPlayer.openCommandBlock`).
        None
    }

    fn on_block_state_changed(&self, state: BlockStateId) {
        // Vanilla `CommandBlockEntity.getMode` derives the mode from the block kind.
        self.state.lock().mode = Self::mode_for_block(state.get_block());
    }
}
