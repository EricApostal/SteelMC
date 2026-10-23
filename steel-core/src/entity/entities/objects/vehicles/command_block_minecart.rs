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
//! Missing dependency: command execution. Vanilla calls
//! `BaseCommandBlock.performCommand(ServerLevel)` which builds a `GAMEMASTER`
//! `CommandSourceStack` and runs `Commands.performPrefixedCommand`. Steel builds
//! its dispatcher in `steel-core/src/command/builtins/mod.rs` but exposes no
//! entity-facing "run this string" entry point, so
//! [`CommandBlockMinecartEntity::perform_command`] is gated and documented rather
//! than stubbed; the storage, synced data, and trigger timing are complete.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{
    AbstractMinecartEntityData, MinecartCommandBlockEntityData,
};
use steel_registry::{vanilla_blocks, vanilla_items};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};
use text_components::TextComponent;

use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartCommandBlock.ACTIVATION_DELAY`.
const ACTIVATION_DELAY: i32 = 4;

/// The `BaseCommandBlock` state a command minecart carries.
#[derive(Debug, Clone, PartialEq)]
struct CommandState {
    /// Vanilla `BaseCommandBlock.command`.
    command: String,
    /// Vanilla `BaseCommandBlock.lastOutput`, absent until a command runs.
    last_output: Option<TextComponent>,
    /// Vanilla `MinecartCommandBlock.lastActivated`, the last activation tick.
    last_activated: i32,
}

impl CommandState {
    fn new() -> Self {
        Self {
            command: String::new(),
            last_output: None,
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
    pub fn set_command(&self, command: String) {
        self.command.lock().command = command.clone();
        self.entity_data.lock().id_command_name.set(command);
    }

    /// The last command output, vanilla `getCommandBlock().getLastOutput()`.
    #[must_use]
    pub fn last_output(&self) -> Option<TextComponent> {
        self.command.lock().last_output.clone()
    }

    /// Sets the last output and mirrors it into synced data, vanilla
    /// `setLastOutput` + `MinecartCommandBase.onUpdated`.
    pub fn set_last_output(&self, last_output: Option<TextComponent>) {
        self.command.lock().last_output = last_output.clone();
        self.entity_data
            .lock()
            .id_last_output
            .set(Box::new(last_output.unwrap_or_default()));
    }

    /// Vanilla `MinecartCommandBlock.activateMinecart`: run on a powered
    /// activator rail, at most once per four ticks.
    fn activate_command_block(&self, world: &World, powered: bool) {
        if !powered {
            return;
        }

        let tick_count = self.tick_count();
        let last_activated = self.command.lock().last_activated;
        if tick_count - last_activated < ACTIVATION_DELAY {
            return;
        }

        self.perform_command(world);
        self.command.lock().last_activated = tick_count;
    }

    /// Vanilla `BaseCommandBlock.performCommand`.
    ///
    /// The command string and its four-tick activation gate are stored exactly as
    /// vanilla; only the dispatch into the command dispatcher is unavailable (see
    /// the module docs). `LastOutput` therefore stays at its previous value until
    /// the dispatch API lands.
    fn perform_command(&self, _world: &World) {
        let command = self.command();
        if command.trim().is_empty() {
            return;
        }

        // MISSING API: `Commands.performPrefixedCommand(source, command)`.
        // `steel-core/src/command/builtins/mod.rs:96` builds the dispatcher, but no
        // entity-facing execute-string entry point exists yet.
        let _ = command;
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
        nbt.insert("Command", state.command.clone());
        if let Some(last_output) = &state.last_output {
            nbt.insert("LastOutput", last_output.to_codec_nbt());
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        let command = nbt
            .string("Command")
            .map(|value| value.to_string())
            .unwrap_or_default();
        let last_output = nbt
            .get("LastOutput")
            .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()));

        {
            let mut state = self.command.lock();
            state.command = command.clone();
            state.last_output = last_output.clone();
        }

        let mut entity_data = self.entity_data.lock();
        entity_data.id_command_name.set(command);
        entity_data
            .id_last_output
            .set(Box::new(last_output.unwrap_or_default()));
    }
}
