//! Ender chest block entity implementation.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::BaseNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::{sound_events, vanilla_block_entity_types};
use steel_utils::types::GameType;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::entities::openers_counter::{
    ContainerOpenersCounter, ContainerViewers, count_viewers,
};
use crate::block_entity::{
    BlockEntity, BlockEntityBase, BlockEntityLifecycleExt as _, ContainerOpenersHost,
};
use crate::inventory::lock::ContainerRef;
use crate::player::Player;
use crate::world::World;

/// Ender chest block entity.
///
/// Holds no inventory of its own: the items live in each player's
/// [`crate::inventory::ender_chest::PlayerEnderChestContainer`]. The block
/// entity only tracks who has it open so it can sound off and emit its block
/// event, exactly like vanilla `EnderChestBlockEntity`.
pub struct EnderChestBlockEntity {
    base: Arc<BlockEntityBase>,
    /// Vanilla `EnderChestBlockEntity.openersCounter`.
    openers: SyncMutex<ContainerOpenersCounter>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EnderChestBlockEntity`.
unsafe impl DowncastType for EnderChestBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/ender_chest");
}

impl EnderChestBlockEntity {
    /// Creates a new ender chest block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: Arc::new(BlockEntityBase::new(
                &vanilla_block_entity_types::ENDER_CHEST,
                level,
                pos,
                state,
            )),
            openers: SyncMutex::new(ContainerOpenersCounter::new()),
        }
    }

    /// Vanilla `EnderChestBlockEntity.openersCounter` sound: `ENDER_CHEST_OPEN`
    /// and `ENDER_CHEST_CLOSE` at the block center.
    fn play_sound(world: &Arc<World>, pos: BlockPos, sound: SoundEventRef) {
        world.play_sound_at(
            sound,
            SoundSource::Blocks,
            DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            ),
            0.5,
            rand::random::<f32>() * 0.1 + 0.9,
            None,
        );
    }
}

impl BlockEntity for EnderChestBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, _nbt: &BaseNbtCompound<'_>) {}

    fn save_additional(&self, _nbt: &mut NbtCompound) {}

    /// Vanilla `EnderChestBlockEntity.triggerEvent` accepts event 1 and hands it
    /// to the client, which drives the lid animation. Steel renders `OPEN` from
    /// the broadcast block event, so accepting it is what publishes it.
    fn trigger_event(&self, param_a: i32, _param_b: i32) -> bool {
        param_a == 1
    }

    fn container_viewers(&self) -> Option<&dyn ContainerViewers> {
        Some(self)
    }
}

impl ContainerOpenersHost for EnderChestBlockEntity {
    /// Vanilla `EnderChestBlockEntity.openersCounter.onOpen`.
    fn on_open(&self, world: &Arc<World>, pos: BlockPos, _state: BlockStateId) {
        Self::play_sound(world, pos, &sound_events::BLOCK_ENDER_CHEST_OPEN);
    }

    /// Vanilla `EnderChestBlockEntity.openersCounter.onClose`.
    fn on_close(&self, world: &Arc<World>, pos: BlockPos, _state: BlockStateId) {
        Self::play_sound(world, pos, &sound_events::BLOCK_ENDER_CHEST_CLOSE);
    }

    /// Vanilla `EnderChestBlockEntity.openersCounter.openerCountChanged`, which
    /// broadcasts block event 1 with the current count.
    fn opener_count_changed(
        &self,
        world: &Arc<World>,
        pos: BlockPos,
        state: BlockStateId,
        _previous: i32,
        current: i32,
    ) {
        world.block_event(pos, state.get_block(), 1, current);
    }

    /// Vanilla `EnderChestBlockEntity.openersCounter.isOwnContainer`.
    ///
    /// Vanilla compares against `PlayerEnderChestContainer.isActiveChest`; Steel
    /// instead asks whether the player's open menu exposes their ender chest
    /// inventory, which is the container this block entity is opened through.
    fn is_own_container(&self, player: &Player) -> bool {
        let ender_chest = ContainerRef::from(Arc::clone(&player.ender_chest_inventory));
        player.has_open_container(&ender_chest)
    }
}

impl ContainerViewers for EnderChestBlockEntity {
    /// Vanilla `EnderChestBlockEntity.startOpen`.
    fn start_open(&self, player: &Player) {
        if self.is_removed() || player.game_mode() == GameType::Spectator {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let transition = self
            .openers
            .lock()
            .increment(player.block_interaction_range());
        ContainerOpenersCounter::apply(self, transition, Some(player), &world, pos, state);
    }

    /// Vanilla `EnderChestBlockEntity.stopOpen`.
    fn stop_open(&self, player: &Player) {
        if self.is_removed() || player.game_mode() == GameType::Spectator {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let transition = self.openers.lock().decrement();
        ContainerOpenersCounter::apply(self, transition, Some(player), &world, pos, state);
    }

    fn open_count(&self) -> i32 {
        self.openers.lock().open_count()
    }

    /// Vanilla `EnderChestBlockEntity.recheckOpen`, driven from the block's
    /// scheduled tick.
    fn recheck_open(&self) {
        if self.is_removed() {
            return;
        }
        let Some(world) = self.get_level() else {
            return;
        };
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let max_interaction_range = self.openers.lock().max_interaction_range();
        let (open_count, max_interaction_range) =
            count_viewers(&world, pos, self, max_interaction_range);
        let transition = self
            .openers
            .lock()
            .recheck(open_count, max_interaction_range);
        ContainerOpenersCounter::apply(self, transition, None, &world, pos, state);
    }
}
