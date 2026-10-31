//! `CommandBlockEntity` (`net.minecraft.world.level.block.entity.CommandBlockEntity`).
//!
//! Vanilla keeps the command text, success count, last output and execution bookkeeping in a
//! shared `BaseCommandBlock`, plus the `powered`/`auto`/`conditionMet` flags on the entity. The
//! three command-block kinds differ only by `Mode` (`REDSTONE`, `AUTO`, `SEQUENCE`).

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use chrono::Local;
use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag as _, ToNbtTag as _};
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::vanilla_game_rules::{
    COMMAND_BLOCK_OUTPUT, MAX_COMMAND_SEQUENCE_LENGTH, SEND_COMMAND_FEEDBACK,
};
use steel_registry::{vanilla_block_entity_types, vanilla_blocks};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, locks::SyncMutex,
};
use text_components::{Modifier as _, TextComponent};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::command::execution::{
    CommandOutputCapture, CommandResultCallback, CommandSource, perform_prefixed_command,
};
use crate::permission::PermissionLevel;
use crate::world::World;

/// Vanilla `BaseCommandBlock` easter egg command (`Searge`).
const SEARGE_COMMAND: &str = "Searge";
/// Vanilla `BaseCommandBlock` easter egg output.
const SEARGE_OUTPUT: &str = "#itzlipofutzli";

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
fn is_command_block(block: BlockRef) -> bool {
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
    fn mode_for_block(block: BlockRef) -> CommandBlockMode {
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
    pub fn on_updated(&self, world: &Arc<World>) {
        world.send_block_updated(self.get_block_pos());
    }

    /// Vanilla `CommandBlockEntity.onModeSwitch`.
    pub fn on_mode_switch(&self, _world: &Arc<World>) {
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
    pub fn server_tick(&self, world: &Arc<World>, pos: BlockPos, block: BlockRef) {
        let (mode, was_condition_met, powered, auto) = {
            let state = self.state.lock();
            (state.mode, state.condition_met, state.powered, state.auto)
        };
        let command_set = !self.command().is_empty();

        match mode {
            CommandBlockMode::Auto => {
                self.mark_condition_met();
                if was_condition_met {
                    self.execute(world, pos, command_set);
                } else if self.is_conditional() {
                    self.set_success_count(0);
                }

                if powered || auto {
                    self.schedule_tick();
                }
            }
            CommandBlockMode::Redstone => {
                if was_condition_met {
                    self.execute(world, pos, command_set);
                } else if self.is_conditional() {
                    self.set_success_count(0);
                }
            }
            // Vanilla `CommandBlock.tick` never executes a chain block directly; its
            // predecessor's `executeChain` drives it.
            CommandBlockMode::Sequence => {}
        }

        // Vanilla `CommandBlock.tick` ends with `updateNeighbourForOutputSignal`, not a full
        // neighbor update.
        world.update_neighbor_for_output_signal(pos, block);
    }

    /// Vanilla `CommandBlock.execute`: run the command, then hand off to the chain.
    fn execute(&self, world: &Arc<World>, pos: BlockPos, command_set: bool) {
        if command_set {
            self.perform_command(world);
        } else {
            self.set_success_count(0);
        }

        let facing = self
            .get_block_state()
            .get_value(&BlockStateProperties::FACING);
        Self::execute_chain(world, pos, facing);
    }

    /// Vanilla `CommandBlock.executeChain`: walk the chain blocks in the facing direction
    /// and execute every powered or automatic one, stopping at the first block that does
    /// not run.
    fn execute_chain(world: &Arc<World>, pos: BlockPos, direction: Direction) {
        let limit = world.get_game_rule(&MAX_COMMAND_SEQUENCE_LENGTH);
        let mut remaining = limit;
        let mut cursor = pos;
        let mut direction = direction;

        while remaining > 0 {
            remaining -= 1;
            cursor = cursor.relative(direction);
            let state = world.get_block_state(cursor);
            let block = state.get_block();
            if block != &vanilla_blocks::CHAIN_COMMAND_BLOCK {
                break;
            }
            let Some(entity) = world.get_block_entity(cursor) else {
                break;
            };
            let Some(chain) = entity.downcast_ref::<CommandBlockEntity>() else {
                break;
            };
            if chain.mode() != CommandBlockMode::Sequence {
                break;
            }

            if chain.is_powered() || chain.is_automatic() {
                if chain.mark_condition_met() {
                    if !chain.perform_command(world) {
                        break;
                    }
                    world.update_neighbor_for_output_signal(cursor, block);
                } else if chain.is_conditional() {
                    chain.set_success_count(0);
                }
            }

            direction = state.get_value(&BlockStateProperties::FACING);
        }

        if remaining <= 0 {
            // Vanilla logs the same warning when the gamerule limit is exhausted.
            tracing::warn!("Command Block chain tried to execute more than {limit} steps!");
        }
    }

    /// Vanilla `BaseCommandBlock.performCommand`.
    ///
    /// Executes the stored command right here, on the tick that scheduled the block,
    /// because the success count gates the chain in the same tick. Returns `false` when
    /// the same game tick already ran this block, which stops a chain at that block.
    fn perform_command(&self, world: &Arc<World>) -> bool {
        let game_time = world.game_time();
        if game_time == self.state.lock().last_execution {
            return false;
        }

        let command = self.command();
        if command.eq_ignore_ascii_case(SEARGE_COMMAND) {
            // Vanilla assigns `lastOutput` directly: `onUpdated` is deliberately not
            // refreshed, and the execution bookkeeping below is skipped.
            let mut state = self.state.lock();
            state.last_output = Some(TextComponent::plain(SEARGE_OUTPUT));
            state.success_count = 1;
            return true;
        }

        let track_output = self.state.lock().track_output;
        self.set_success_count(0);
        // Vanilla `BaseCommandBlock.performCommand`: a disabled command block never runs
        // the command, and `StringUtil.isNullOrEmpty` only rejects the empty string.
        if world.is_command_block_enabled() && !command.is_empty() {
            // Vanilla clears the previous output before dispatch.
            self.set_last_output(None);
            self.dispatch(world, &command, track_output);
        }

        let update_last_execution = self.state.lock().update_last_execution;
        self.state.lock().last_execution = if update_last_execution { game_time } else { -1 };
        true
    }

    /// Builds the vanilla `GAMEMASTER` command source at the block center and runs the
    /// command, accumulating the callback's successes like
    /// `BaseCommandBlock.performCommand`'s `withCallback`.
    fn dispatch(&self, world: &Arc<World>, command: &str, track_output: bool) {
        let Some(server) = world.server() else {
            return;
        };
        let pos = self.get_block_pos();
        let facing = self
            .get_block_state()
            .get_value(&BlockStateProperties::FACING);
        let captured = Arc::new(SyncMutex::new(None::<TextComponent>));
        let output = track_output.then(|| {
            let captured = Arc::clone(&captured);
            let accepts_success = world.get_game_rule(&SEND_COMMAND_FEEDBACK);
            let informs_admins = world.get_game_rule(&COMMAND_BLOCK_OUTPUT);
            CommandOutputCapture::new(
                move |message| {
                    // Vanilla `CloseableCommandBlockSource.sendSystemMessage`: prefix the
                    // local time of day and keep the latest message as the last output.
                    *captured.lock() = Some(
                        TextComponent::plain(format!("[{}] ", Local::now().format("%H:%M:%S")))
                            .add_child(message.clone()),
                    );
                },
                accepts_success,
                true,
                informs_admins,
            )
        });

        let source = Arc::new(CommandSource::for_command_block(
            server,
            Arc::clone(world),
            DVec3::new(
                f64::from(pos.0.x) + 0.5,
                f64::from(pos.0.y) + 0.5,
                f64::from(pos.0.z) + 0.5,
            ),
            (facing.to_yaw(), 0.0),
            PermissionLevel::Gamemasters,
            output,
        ));
        let successes = Arc::new(AtomicI32::new(0));
        let callback = CommandResultCallback::new({
            let successes = Arc::clone(&successes);
            move |success, _result| {
                if success {
                    successes.fetch_add(1, Ordering::Relaxed);
                }
            }
        });
        perform_prefixed_command(&source, command, callback);
        self.set_success_count(successes.load(Ordering::Relaxed));

        if track_output {
            // Vanilla's source refreshes `LastOutput` and the block update only when the
            // command actually emitted a message.
            let captured = captured.lock().clone();
            match captured {
                Some(output) => {
                    self.set_last_output(Some(output));
                    self.on_updated(world);
                }
                None => self.state.lock().last_output = None,
            }
        }
    }

    /// Vanilla `BaseCommandBlock.getLastOutput`.
    #[must_use]
    pub fn last_output(&self) -> Option<TextComponent> {
        self.state.lock().last_output.clone()
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
            state.command = view.string("Command").map_or_default(ToString::to_string);
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

#[cfg(test)]
mod tests {
    use std::slice;

    use steel_registry::vanilla_game_rules::MAX_ENTITY_CRAMMING;
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::ChunkPos;
    use steel_utils::types::UpdateFlags;
    use text_components::content::Content;
    use tokio::runtime::Builder;

    use crate::behavior::init_behaviors;
    use crate::block_entity::{SharedBlockEntity, init_block_entities};
    use crate::config::ResolvedDomainConfig;
    use crate::entity::Entity as _;
    use crate::entity::entities::objects::CommandBlockMinecartEntity;
    use crate::entity::next_entity_id;
    use crate::permission::PermissionSubjectIndex;
    use crate::server::{Server, test_server_with_worlds_and_config};
    use crate::test_support::{
        TestWorld, fresh_test_world, insert_entity_ticking_chunks, settle_entity_ticking_chunk,
        test_runtime_config, tick_test_world,
    };

    use super::*;

    const POS: BlockPos = BlockPos::new(8, 64, 8);

    /// A fresh test world with its 5x5 entity-ticking neighbourhood and the server that
    /// owns it, so a block tick can reach the command dispatcher.
    fn ticking_setup(name: &'static str) -> (TestWorld, Arc<World>, Arc<Server>) {
        init_vanilla_registry();
        init_behaviors();
        init_block_entities();

        let fixture = fresh_test_world(name);
        let world = Arc::clone(&fixture.world);
        let center = ChunkPos::new(POS.0.x >> 4, POS.0.z >> 4);
        insert_entity_ticking_chunks(&world, center, 2);
        settle_entity_ticking_chunk(&world, center);

        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime should initialize");
        let server = runtime.block_on(async {
            let domain = ResolvedDomainConfig {
                name: world.domain().to_owned(),
                default_world: world.key.clone(),
                worlds: vec![world.key.clone()],
            };
            test_server_with_worlds_and_config(
                domain.name.clone(),
                slice::from_ref(&domain),
                slice::from_ref(&world),
                PermissionSubjectIndex::new(),
                test_runtime_config(1),
            )
            .await
            .expect("test server should initialize")
        });

        (fixture, world, server)
    }

    fn place_command_block(
        world: &Arc<World>,
        pos: BlockPos,
        block: BlockRef,
        facing: Direction,
        conditional: bool,
        command: &str,
    ) -> SharedBlockEntity {
        let state = block
            .default_state()
            .set_value(&BlockStateProperties::FACING, facing)
            .set_value(&BlockStateProperties::CONDITIONAL, conditional);
        assert!(
            world.set_block(pos, state, UpdateFlags::UPDATE_ALL),
            "the command block must be placed in a loaded chunk"
        );
        let entity = world
            .get_block_entity(pos)
            .expect("the command block creates its block entity");
        entity
            .downcast_ref::<CommandBlockEntity>()
            .expect("the block entity is a command block entity")
            .set_command(command);
        entity
    }

    fn power_with_redstone_block(world: &Arc<World>, pos: BlockPos) {
        assert!(
            world.set_block(
                pos,
                vanilla_blocks::REDSTONE_BLOCK.default_state(),
                UpdateFlags::UPDATE_ALL,
            ),
            "the power source must be placed in a loaded chunk"
        );
    }

    fn command_block(entity: &SharedBlockEntity) -> &CommandBlockEntity {
        entity
            .downcast_ref::<CommandBlockEntity>()
            .expect("shared block entity is a command block entity")
    }

    /// Returns whether the captured output mentions `needle` in any text or translation
    /// argument.
    ///
    /// `TextComponent`'s `Display` resolves translations through the global display
    /// resolutor, which tests never install, so it renders `[Translation: key]` and hides
    /// the arguments. Walking the component tree asserts the real message content.
    fn mentions(component: &TextComponent, needle: &str) -> bool {
        let in_content = match &component.content {
            Content::Text { text } => text.contains(needle),
            Content::Translate(message) => {
                message.key.contains(needle)
                    || message
                        .args
                        .as_ref()
                        .is_some_and(|args| args.iter().any(|arg| mentions(arg, needle)))
            }
            _ => false,
        };
        in_content
            || component
                .children
                .iter()
                .any(|child| mentions(child, needle))
    }

    fn last_output(entity: &SharedBlockEntity) -> TextComponent {
        command_block(entity)
            .last_output()
            .expect("the executed command must capture its last output")
    }

    #[test]
    fn powered_impulse_command_block_runs_its_command_once() {
        let (_fixture, world, _server) = ticking_setup("command_block_impulse");
        let entity = place_command_block(
            &world,
            POS,
            &vanilla_blocks::COMMAND_BLOCK,
            Direction::East,
            false,
            "gamerule max_entity_cramming 5",
        );
        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 24);

        power_with_redstone_block(&world, POS.west());
        // Powering schedules the block's `server_tick` for the next tick (vanilla
        // `CommandBlock.setPoweredAndUpdate` schedules with delay 1).
        tick_test_world(&world, 2, true);

        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 5);
        assert_eq!(command_block(&entity).success_count(), 1);
        assert!(
            mentions(&last_output(&entity), "max_entity_cramming"),
            "unexpected last output: {:?}",
            last_output(&entity)
        );

        // Vanilla `BaseCommandBlock.performCommand` refuses a second run in the same tick.
        assert!(!command_block(&entity).perform_command(&world));
        assert_eq!(command_block(&entity).success_count(), 1);
    }

    #[test]
    fn powered_repeating_command_block_runs_every_scheduled_tick() {
        let (_fixture, world, _server) = ticking_setup("command_block_repeating");
        let entity = place_command_block(
            &world,
            POS,
            &vanilla_blocks::REPEATING_COMMAND_BLOCK,
            Direction::East,
            false,
            "gamerule max_entity_cramming 7",
        );

        power_with_redstone_block(&world, POS.west());
        tick_test_world(&world, 1, true);
        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 7);
        assert_eq!(command_block(&entity).success_count(), 1);

        // The repeating block re-schedules itself while powered, so the next tick still
        // runs exactly one command execution.
        tick_test_world(&world, 2, true);
        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 7);
        assert_eq!(command_block(&entity).success_count(), 1);
        assert!(
            mentions(&last_output(&entity), "max_entity_cramming"),
            "unexpected last output: {:?}",
            last_output(&entity)
        );
    }

    #[test]
    fn conditional_chain_block_runs_after_its_predecessor_succeeds() {
        let (_fixture, world, _server) = ticking_setup("command_block_chain_success");
        let impulse = place_command_block(
            &world,
            POS,
            &vanilla_blocks::COMMAND_BLOCK,
            Direction::East,
            false,
            "gamerule max_entity_cramming 5",
        );
        let chain = place_command_block(
            &world,
            POS.east(),
            &vanilla_blocks::CHAIN_COMMAND_BLOCK,
            Direction::East,
            true,
            "gamerule max_entity_cramming 9",
        );

        power_with_redstone_block(&world, POS.west());
        tick_test_world(&world, 1, true);

        assert_eq!(command_block(&impulse).success_count(), 1);
        assert_eq!(command_block(&chain).success_count(), 1);
        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 9);
        assert!(command_block(&chain).was_condition_met());
        assert!(
            mentions(&last_output(&chain), "max_entity_cramming"),
            "unexpected chain last output: {:?}",
            last_output(&chain)
        );
    }

    #[test]
    fn conditional_chain_block_stays_idle_when_its_predecessor_fails() {
        let (_fixture, world, _server) = ticking_setup("command_block_chain_failure");
        let impulse = place_command_block(
            &world,
            POS,
            &vanilla_blocks::COMMAND_BLOCK,
            Direction::East,
            false,
            "this_command_does_not_exist",
        );
        let chain = place_command_block(
            &world,
            POS.east(),
            &vanilla_blocks::CHAIN_COMMAND_BLOCK,
            Direction::East,
            true,
            "gamerule max_entity_cramming 9",
        );

        power_with_redstone_block(&world, POS.west());
        tick_test_world(&world, 1, true);

        assert_eq!(command_block(&impulse).success_count(), 0);
        assert_eq!(command_block(&chain).success_count(), 0);
        assert!(!command_block(&chain).was_condition_met());
        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 24);
    }

    #[test]
    fn command_block_minecart_runs_its_command_on_activation() {
        let (_fixture, world, _server) = ticking_setup("command_block_minecart_run");
        let cart = Arc::new(CommandBlockMinecartEntity::new(
            &vanilla_entities::COMMAND_BLOCK_MINECART,
            next_entity_id(),
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(&world),
        ));
        cart.set_command("gamerule max_entity_cramming 5".to_owned());
        // Vanilla `MinecartCommandBlock.activateMinecart` waits four ticks between runs.
        for _ in 0..4 {
            cart.base().advance_tick_count();
        }

        cart.activate_minecart_for_test(&world, true);

        assert_eq!(world.get_game_rule(&MAX_ENTITY_CRAMMING), 5);
        assert_eq!(cart.success_count(), 1);
        let output = cart
            .last_output()
            .expect("activation must capture the output");
        assert!(
            mentions(&output, "max_entity_cramming"),
            "unexpected minecart last output: {output:?}"
        );
    }
}
