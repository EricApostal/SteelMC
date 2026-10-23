//! `CommandBlockEntity` (`net.minecraft.world.level.block.entity.CommandBlockEntity`).
//!
//! Vanilla keeps the command text, success count, last output and execution bookkeeping in a
//! shared `BaseCommandBlock`, plus the `powered`/`auto`/`conditionMet` flags on the entity. The
//! three command-block kinds differ only by `Mode` (`REDSTONE`, `AUTO`, `SEQUENCE`).

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

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

impl CommandBlockMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Redstone => "REDSTONE",
            Self::Auto => "AUTO",
            Self::Sequence => "SEQUENCE",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "AUTO" => Self::Auto,
            "SEQUENCE" => Self::Sequence,
            _ => Self::Redstone,
        }
    }
}

/// Vanilla `BaseCommandBlock` + `CommandBlockEntity` state.
struct CommandBlockState {
    command: String,
    /// Vanilla `CommandBlockEntity.Mode`.
    mode: CommandBlockMode,
    /// Vanilla `BaseCommandBlock.successCount`.
    success_count: i32,
    /// Vanilla `BaseCommandBlock.lastOutput`.
    last_output: Option<String>,
    /// Vanilla `BaseCommandBlock.trackOutput`.
    track_output: bool,
    /// Vanilla `BaseCommandBlock.updateLastExecution`.
    update_last_execution: bool,
    /// Vanilla `BaseCommandBlock.lastExecution`.
    last_execution: i64,
    /// Whether the last redstone state was powered.
    powered: bool,
    /// Whether the block executes automatically (`repeating_command_block`).
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
            update_last_execution: false,
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
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::COMMAND_BLOCK,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(CommandBlockState::default()),
        }
    }

    /// Vanilla `CommandBlockEntity.setAutomatic`: set from the block kind at construction.
    pub fn set_automatic(&self, automatic: bool) {
        let mut state = self.state.lock();
        state.auto = automatic;
        state.mode = if automatic {
            CommandBlockMode::Auto
        } else {
            CommandBlockMode::Redstone
        };
    }

    /// Returns whether this block executes automatically.
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

    /// Vanilla `BaseCommandBlock.getCommand`.
    #[must_use]
    pub fn command(&self) -> String {
        self.state.lock().command.clone()
    }

    /// Vanilla `BaseCommandBlock.setCommand`.
    pub fn set_command(&self, command: impl Into<String>) {
        self.state.lock().command = command.into();
        BlockEntity::set_changed(self);
    }

    /// Vanilla `CommandBlockEntity.markConditionMet`.
    pub fn mark_condition_met(&self) {
        self.state.lock().condition_met = true;
    }

    /// Vanilla `CommandBlockEntity.wasConditionMet`.
    #[must_use]
    pub fn was_condition_met(&self) -> bool {
        self.state.lock().condition_met
    }

    /// Vanilla `CommandBlockEntity.serverTick`: mode-dependent execution.
    ///
    /// The command itself is executed through Steel's existing dispatcher
    /// (`Server::submit_command`); reaching the server from a block entity is the one missing
    /// foundation (see the TODO below).
    pub fn server_tick(
        &self,
        world: &std::sync::Arc<World>,
        pos: BlockPos,
        block: steel_registry::blocks::BlockRef,
    ) {
        let (mode, was_condition_met, condition_met, powered, auto, command_set) = {
            let state = self.state.lock();
            (
                state.mode,
                state.condition_met,
                state.condition_met,
                state.powered,
                state.auto,
                !state.command.trim().is_empty(),
            )
        };
        let _ = was_condition_met;

        match mode {
            CommandBlockMode::Auto => {
                self.mark_condition_met();
                let _ = (condition_met, command_set, world, pos);
                // TODO(Foundations): execute `self.command()` via `Server::submit_command` and set
                // the success count. A block entity has no `Arc<Server>` handle (`World` exposes
                // none), so the dispatcher call needs a world→server accessor (owner: Main /
                // ServerFoundations).
                if powered || auto {
                    world.schedule_block_tick_default(pos, block, 1);
                }
            }
            CommandBlockMode::Redstone | CommandBlockMode::Sequence => {
                let _ = (condition_met, command_set, world, pos);
                // TODO(Foundations): same dispatcher dependency as the Auto branch.
            }
        }
        world.update_neighbors_at(pos, block);
    }
}

impl BlockEntity for CommandBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.command = view
            .string("Command")
            .map(|v| v.to_string())
            .unwrap_or_default();
        state.success_count = view.int("SuccessCount").unwrap_or(0);
        state.last_output = view.string("LastOutput").map(|v| v.to_string());
        state.track_output = view.byte("TrackOutput").unwrap_or(1) != 0;
        state.update_last_execution = view.byte("UpdateLastExecution").unwrap_or(0) != 0;
        state.last_execution = view.long("LastExecution").unwrap_or(-1);
        state.powered = view.byte("powered").unwrap_or(0) != 0;
        state.auto = view.byte("auto").unwrap_or(0) != 0;
        state.condition_met = view.byte("conditionMet").unwrap_or(0) != 0;
        state.mode = view
            .string("Mode")
            .map(|v| CommandBlockMode::from_str(&v.to_string()))
            .unwrap_or(if state.auto {
                CommandBlockMode::Auto
            } else {
                CommandBlockMode::Redstone
            });
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("Command", state.command.clone());
        nbt.insert("SuccessCount", state.success_count);
        if let Some(output) = &state.last_output {
            nbt.insert("LastOutput", output.clone());
        }
        nbt.insert("TrackOutput", i8::from(state.track_output));
        nbt.insert("UpdateLastExecution", i8::from(state.update_last_execution));
        nbt.insert("LastExecution", state.last_execution);
        nbt.insert("powered", i8::from(state.powered));
        nbt.insert("auto", i8::from(state.auto));
        nbt.insert("conditionMet", i8::from(state.condition_met));
        nbt.insert("Mode", state.mode.as_str());
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }
}
