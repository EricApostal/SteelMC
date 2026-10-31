//! Command block minecart entity
//! (`net.minecraft.world.entity.vehicle.minecart.MinecartCommandBlock`).
//!
//! Persists the vanilla `BaseCommandBlock` fields (`Command`, `LastOutput`) and
//! mirrors them into the synced `id_command_name`/`id_last_output` slots, and
//! fires on a powered activator rail with vanilla's four-tick activation delay.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].
//!
//! Command execution mirrors `BaseCommandBlock.performCommand` with the vanilla
//! `GAMEMASTER` source attached to this cart: the string runs against the owning
//! world's server through the shared command execution queue
//! ([`crate::command::execution::perform_prefixed_command`]).

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Weak};

use chrono::Local;
use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{
    AbstractMinecartEntityData, MinecartCommandBlockEntityData,
};
use steel_registry::vanilla_game_rules::{COMMAND_BLOCK_OUTPUT, SEND_COMMAND_FEEDBACK};
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};
use text_components::{Modifier as _, TextComponent};

use crate::behavior::InteractionResult;
use crate::command::execution::{
    CommandOutputCapture, CommandResultCallback, CommandSource, perform_prefixed_command,
};
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::permission::PermissionLevel;
use crate::player::Player;
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartCommandBlock.ACTIVATION_DELAY`.
pub(crate) const ACTIVATION_DELAY: i32 = 4;
/// Vanilla `BaseCommandBlock.NO_LAST_EXECUTION`.
const NO_LAST_EXECUTION: i64 = -1;
/// Vanilla `BaseCommandBlock` easter egg command (`Searge`).
const SEARGE_COMMAND: &str = "Searge";
/// Vanilla `BaseCommandBlock` easter egg output.
const SEARGE_OUTPUT: &str = "#itzlipofutzli";

/// The `BaseCommandBlock` state a command minecart carries.
#[derive(Debug, Clone, PartialEq)]
struct CommandState {
    /// Vanilla `BaseCommandBlock.command`.
    command: String,
    /// Vanilla `BaseCommandBlock.successCount`.
    success_count: i32,
    /// Vanilla `BaseCommandBlock.lastOutput`, absent until a command runs.
    last_output: Option<TextComponent>,
    /// Vanilla `BaseCommandBlock.trackOutput`, `true` until the editor clears it.
    track_output: bool,
    /// Vanilla `BaseCommandBlock.updateLastExecution`.
    update_last_execution: bool,
    /// Vanilla `BaseCommandBlock.lastExecution`, `-1` (`NO_LAST_EXECUTION`) when unset.
    last_execution: i64,
    /// Vanilla `MinecartCommandBlock.lastActivated`, the last activation tick.
    last_activated: i32,
}

impl CommandState {
    const fn new() -> Self {
        Self {
            command: String::new(),
            success_count: 0,
            last_output: None,
            track_output: true,
            update_last_execution: true,
            last_execution: NO_LAST_EXECUTION,
            last_activated: 0,
        }
    }
}

/// A command block minecart.
#[entity_behavior(class = "MinecartCommandBlock")]
pub struct CommandBlockMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<MinecartCommandBlockEntityData>,
    state: SyncMutex<MinecartState>,
    command: SyncMutex<CommandState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CommandBlockMinecartEntity`.
unsafe impl DowncastType for CommandBlockMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/command_block_minecart");
}

impl CommandBlockMinecartEntity {
    /// Creates a new command block minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(MinecartCommandBlockEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            command: SyncMutex::new(CommandState::new()),
        }
    }

    /// Creates a command block minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(MinecartCommandBlockEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            command: SyncMutex::new(CommandState::new()),
        }
    }

    /// The stored command, vanilla `getCommandBlock().getCommand()`.
    #[must_use]
    pub fn command(&self) -> String {
        self.command.lock().command.clone()
    }

    /// Sets the command and mirrors it into synced data, vanilla `setCommand` +
    /// `MinecartCommandBase.onUpdated`.
    ///
    /// Vanilla `BaseCommandBlock.setCommand` also clears the success count.
    pub fn set_command(&self, command: String) {
        {
            let mut state = self.command.lock();
            state.command.clone_from(&command);
            state.success_count = 0;
        }
        self.entity_data.lock().id_command_name.set(command);
    }

    /// Test hook for `Minecart::activate_minecart`.
    ///
    /// The `Minecart` trait is `pub(super)`-scoped to the vehicles module, but the
    /// command-block activation tests live in `block_entity::entities::command_block`,
    /// so expose the one method they drive rather than widening the trait.
    #[cfg(test)]
    pub(crate) fn activate_minecart_for_test(&self, world: &Arc<World>, powered: bool) {
        self.activate_command_block(world, powered);
    }

    /// The last command's success count, vanilla `BaseCommandBlock.getSuccessCount`.
    #[must_use]
    pub fn success_count(&self) -> i32 {
        self.command.lock().success_count
    }

    /// The last command execution's game time, vanilla `BaseCommandBlock.lastExecution`.
    #[must_use]
    pub fn last_execution(&self) -> i64 {
        self.command.lock().last_execution
    }

    /// The last command output, vanilla `getCommandBlock().getLastOutput()`.
    #[must_use]
    pub fn last_output(&self) -> Option<TextComponent> {
        self.command.lock().last_output.clone()
    }

    /// Whether the minecart broadcasts its output, vanilla `BaseCommandBlock.isTrackOutput`.
    #[must_use]
    pub fn track_output(&self) -> bool {
        self.command.lock().track_output
    }

    /// Vanilla `BaseCommandBlock.setTrackOutput`.
    pub fn set_track_output(&self, track_output: bool) {
        self.command.lock().track_output = track_output;
    }

    /// Sets the last output and mirrors it into synced data, vanilla
    /// `setLastOutput` + `MinecartCommandBase.onUpdated`.
    pub fn set_last_output(&self, last_output: Option<TextComponent>) {
        self.command.lock().last_output.clone_from(&last_output);
        self.entity_data
            .lock()
            .id_last_output
            .set(Box::new(last_output.unwrap_or_default()));
    }

    /// Vanilla `MinecartCommandBlock.activateMinecart`: run on a powered
    /// activator rail, at most once per four ticks.
    fn activate_command_block(&self, world: &Arc<World>, powered: bool) {
        if !powered {
            return;
        }

        let tick_count = self.tick_count();
        let last_activated = self.command.lock().last_activated;
        if tick_count - last_activated < ACTIVATION_DELAY {
            return;
        }

        let _ = self.perform_command(world);
        self.command.lock().last_activated = tick_count;
    }

    /// Vanilla `BaseCommandBlock.performCommand`.
    ///
    /// The command string, the same-game-tick guard, the `Searge` easter egg and the
    /// execution bookkeeping mirror vanilla; the command itself runs with the vanilla
    /// `GAMEMASTER` source attached to this cart.
    fn perform_command(&self, world: &Arc<World>) -> bool {
        let game_time = world.game_time();
        if game_time == self.command.lock().last_execution {
            return false;
        }

        let command = self.command();
        if command.eq_ignore_ascii_case(SEARGE_COMMAND) {
            // Vanilla assigns `lastOutput` directly here: `onUpdated` (and therefore the
            // synced `id_last_output` slot) is deliberately not refreshed.
            let mut state = self.command.lock();
            state.last_output = Some(TextComponent::plain(SEARGE_OUTPUT));
            state.success_count = 1;
            return true;
        }

        let track_output = self.command.lock().track_output;
        self.command.lock().success_count = 0;
        // Vanilla `BaseCommandBlock.performCommand`: a disabled command block never runs the
        // command, and `StringUtil.isNullOrEmpty` only rejects the empty string.
        if world.is_command_block_enabled() && !command.is_empty() {
            // Vanilla clears the previous output before dispatch.
            self.command.lock().last_output = None;
            self.dispatch(world, &command, track_output);
        }

        let update_last_execution = self.command.lock().update_last_execution;
        self.command.lock().last_execution = if update_last_execution {
            game_time
        } else {
            NO_LAST_EXECUTION
        };
        true
    }

    /// Runs `command` with `MinecartCommandBase.createCommandSourceStack`'s source: the
    /// cart's position, rotation and entity at `GAMEMASTER` level.
    fn dispatch(&self, world: &Arc<World>, command: &str, track_output: bool) {
        let Some(server) = world.server() else {
            return;
        };
        let captured = Arc::new(SyncMutex::new(None::<TextComponent>));
        let output = track_output.then(|| {
            let captured = Arc::clone(&captured);
            let accepts_success = world.get_game_rule(&SEND_COMMAND_FEEDBACK);
            let informs_admins = world.get_game_rule(&COMMAND_BLOCK_OUTPUT);
            CommandOutputCapture::new(
                move |message| {
                    // Vanilla `CloseableCommandBlockSource.sendSystemMessage`.
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

        let mut source = CommandSource::for_command_block(
            server,
            Arc::clone(world),
            self.position(),
            self.rotation(),
            PermissionLevel::Gamemasters,
            output,
        );
        if let Some(entity) = world.get_entity_by_id(self.id()) {
            // Vanilla passes the cart itself as the `CommandSourceStack` entity.
            source = source.with_entity(entity);
        }
        let source = Arc::new(source);

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
        self.command.lock().success_count = successes.load(Ordering::Relaxed);

        if track_output {
            // Vanilla only refreshes the synced output when the command emitted a message.
            let captured = captured.lock().clone();
            match captured {
                Some(output) => self.set_last_output(Some(output)),
                None => self.command.lock().last_output = None,
            }
        }
    }
}

impl Minecart for CommandBlockMinecartEntity {
    fn cart_state(&self) -> &SyncMutex<MinecartState> {
        &self.state
    }

    fn with_minecart_data<R>(&self, f: impl FnOnce(&AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart())
    }

    fn with_minecart_data_mut<R>(&self, f: impl FnOnce(&mut AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart_mut())
    }

    fn default_display_block_state(&self) -> BlockStateId {
        vanilla_blocks::COMMAND_BLOCK.default_state()
    }

    fn drop_item(&self) -> Option<ItemRef> {
        // Vanilla `MinecartCommandBlock.getDropItem` drops a plain minecart.
        Some(&vanilla_items::MINECART)
    }

    fn activate_minecart(&self, world: &Arc<World>, _pos: BlockPos, powered: bool) {
        self.activate_command_block(world, powered);
    }
}

impl Entity for CommandBlockMinecartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).minecart_tick();
    }

    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        // Vanilla `MinecartCommandBlock.interact`: `super.interact` is `Entity.interact`,
        // which is `PASS` for a cart. Vanilla then opens the editor from the client's own
        // `interact` (`player.level().isClientSide()`), so the server only reports success.
        if !player.can_use_game_master_blocks() {
            return InteractionResult::Pass;
        }

        InteractionResult::Success
    }

    fn push_entity(&self, entity: &dyn Entity) {
        // Vanilla `AbstractMinecart.push`.
        self.minecart_push(entity);
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn is_on_rails(&self) -> bool {
        self.is_on_minecart_rails()
    }

    fn get_default_gravity(&self) -> f64 {
        self.minecart_default_gravity()
    }

    fn get_air_drag(&self) -> f64 {
        minecart_base::AIR_DRAG
    }

    fn dimension_changing_delay(&self) -> i32 {
        10
    }

    fn get_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        minecart_base::minecart_relative_portal_position(self, axis, portal_area)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.minecart_hurt(world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
        let state = self.command.lock();
        // Vanilla `BaseCommandBlock.save`. `CustomName` is skipped: vanilla writes the command
        // block's own name under the entity's `CustomName` key, which Steel keeps on the entity.
        nbt.insert("Command", state.command.clone());
        nbt.insert("SuccessCount", state.success_count);
        nbt.insert("TrackOutput", i8::from(state.track_output));
        if state.track_output
            && let Some(last_output) = &state.last_output
        {
            nbt.insert("LastOutput", last_output.to_codec_nbt());
        }
        nbt.insert("UpdateLastExecution", i8::from(state.update_last_execution));
        if state.update_last_execution && state.last_execution != NO_LAST_EXECUTION {
            nbt.insert("LastExecution", state.last_execution);
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        let command = nbt.string("Command").map_or_default(ToString::to_string);
        let success_count = nbt.int("SuccessCount").unwrap_or(0);
        let track_output = nbt.byte("TrackOutput").unwrap_or(1) != 0;
        // Vanilla only reads the output back while output tracking is enabled.
        let last_output = if track_output {
            nbt.get("LastOutput")
                .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()))
        } else {
            None
        };
        let update_last_execution = nbt.byte("UpdateLastExecution").unwrap_or(1) != 0;
        let last_execution = if update_last_execution {
            nbt.long("LastExecution").unwrap_or(NO_LAST_EXECUTION)
        } else {
            NO_LAST_EXECUTION
        };

        {
            let mut state = self.command.lock();
            state.command.clone_from(&command);
            state.success_count = success_count;
            state.last_output.clone_from(&last_output);
            state.track_output = track_output;
            state.update_last_execution = update_last_execution;
            state.last_execution = last_execution;
        }

        let mut entity_data = self.entity_data.lock();
        entity_data.id_command_name.set(command);
        entity_data
            .id_last_output
            .set(Box::new(last_output.unwrap_or_default()));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use simdnbt::borrow::read_compound;
    use simdnbt::owned::NbtCompound;
    use steel_registry::{init_vanilla_registry, vanilla_entities};

    use crate::entity::next_entity_id;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    use super::*;

    fn command_cart() -> CommandBlockMinecartEntity {
        CommandBlockMinecartEntity::new(
            &vanilla_entities::COMMAND_BLOCK_MINECART,
            1,
            DVec3::ZERO,
            Weak::new(),
        )
    }

    /// Re-parses saved NBT and hands the borrowed view to `load_additional`, mirroring the
    /// entity reload path.
    fn reload(nbt: &NbtCompound, load: impl FnOnce(BorrowedNbtCompoundView<'_, '_>)) {
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("test NBT should reborrow");
        load((&borrowed).into());
    }

    #[test]
    fn only_game_master_players_can_open_the_editor() {
        // Vanilla `MinecartCommandBlock.interact`: players without game-master-block rights get
        // `PASS`; otherwise the server reports success (the client opens its own editor screen).
        init_vanilla_registry();
        let world_fixture = fresh_test_world("command_minecart_interact");
        let world = &world_fixture.world;
        let cart = CommandBlockMinecartEntity::new(
            &vanilla_entities::COMMAND_BLOCK_MINECART,
            next_entity_id(),
            DVec3::ZERO,
            Arc::downgrade(world),
        );

        let player =
            TestPlayerBuilder::new(Arc::clone(world), "NotGamemaster", next_entity_id()).build();
        assert!(!player.can_use_game_master_blocks());
        assert_eq!(
            cart.interact(&player, InteractionHand::MainHand, DVec3::ZERO),
            InteractionResult::Pass
        );
    }

    #[test]
    fn set_command_clears_the_success_count_and_syncs_the_command_name() {
        init_vanilla_registry();
        let cart = command_cart();
        {
            let mut state = cart.command.lock();
            state.command = "say one".to_owned();
            state.success_count = 4;
        }

        // Vanilla `BaseCommandBlock.setCommand`: the command plus `successCount = 0`.
        cart.set_command("say two".to_owned());
        assert_eq!(cart.command(), "say two");
        assert_eq!(cart.success_count(), 0);

        // Vanilla `MinecartCommandBase.onUpdated`: the synced slot follows the state.
        assert_eq!(
            cart.entity_data.lock().id_command_name.get().as_str(),
            "say two"
        );
    }

    #[test]
    fn perform_command_runs_the_searge_egg_and_skips_repeat_ticks() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("command_minecart_perform");
        let world = &world_fixture.world;
        let cart = CommandBlockMinecartEntity::new(
            &vanilla_entities::COMMAND_BLOCK_MINECART,
            next_entity_id(),
            DVec3::ZERO,
            Arc::downgrade(world),
        );

        // Vanilla `BaseCommandBlock.performCommand` easter egg: the output is stored locally
        // (no `onUpdated`, so the synced slot stays empty) with a success count of 1.
        cart.set_command("Searge".to_owned());
        assert!(cart.perform_command(world));
        assert_eq!(cart.success_count(), 1);
        assert_eq!(
            cart.last_output(),
            Some(TextComponent::plain("#itzlipofutzli"))
        );
        assert_eq!(
            cart.last_execution(),
            NO_LAST_EXECUTION,
            "the easter egg skips the execution bookkeeping"
        );

        cart.set_command("say hi".to_owned());
        assert_eq!(cart.success_count(), 0);
        assert!(cart.perform_command(world));
        assert_eq!(cart.last_execution(), world.game_time());
        assert!(
            !cart.perform_command(world),
            "vanilla runs at most once per game tick"
        );
    }

    #[test]
    fn save_load_round_trips_base_command_block_bookkeeping() {
        init_vanilla_registry();
        let mut nbt = NbtCompound::new();
        nbt.insert("Command", "say hi");
        nbt.insert("SuccessCount", 3);
        nbt.insert("TrackOutput", 1i8);
        let mut output = NbtCompound::new();
        output.insert("text", "ran");
        nbt.insert("LastOutput", output);
        nbt.insert("UpdateLastExecution", 1i8);
        nbt.insert("LastExecution", 100i64);

        let cart = command_cart();
        reload(&nbt, |view| cart.load_additional(view));
        assert_eq!(cart.command(), "say hi");
        assert_eq!(cart.success_count(), 3);
        assert!(cart.track_output());
        assert!(cart.last_output().is_some());
        assert_eq!(cart.last_execution(), 100);

        let loaded = command_cart();
        let mut saved = NbtCompound::new();
        cart.save_additional(&mut saved);
        reload(&saved, |view| loaded.load_additional(view));
        assert_eq!(loaded.command(), "say hi");
        assert_eq!(loaded.success_count(), 3);
        assert_eq!(loaded.last_execution(), 100);
        assert_eq!(loaded.last_output(), cart.last_output());
    }

    #[test]
    fn disabled_output_tracking_drops_the_output_on_save_and_load() {
        init_vanilla_registry();
        // Vanilla `BaseCommandBlock`: `TrackOutput = 0` clears `LastOutput` and skips both the
        // write and the read.
        let mut nbt = NbtCompound::new();
        nbt.insert("Command", "say hi");
        nbt.insert("TrackOutput", 0i8);
        let mut output = NbtCompound::new();
        output.insert("text", "ran");
        nbt.insert("LastOutput", output);
        nbt.insert("UpdateLastExecution", 0i8);
        nbt.insert("LastExecution", 99i64);

        let cart = command_cart();
        reload(&nbt, |view| cart.load_additional(view));
        assert!(!cart.track_output());
        assert_eq!(cart.last_output(), None);
        assert_eq!(cart.last_execution(), NO_LAST_EXECUTION);

        let mut saved = NbtCompound::new();
        cart.save_additional(&mut saved);
        assert_eq!(saved.byte("TrackOutput"), Some(0));
        assert!(saved.get("LastOutput").is_none());
        assert!(saved.get("LastExecution").is_none());
    }
}
