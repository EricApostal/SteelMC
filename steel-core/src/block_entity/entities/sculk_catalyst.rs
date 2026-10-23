//! Vanilla `SculkCatalystBlockEntity`.
//!
//! Vanilla reference: `SculkCatalystBlockEntity`. Reacts immediately to `entity_die` within 8
//! blocks (no vibration travel), converts the victim's experience into sculk charge and pulses the
//! block's `bloom` state for 8 ticks. Charge cursors and spreading need a level-bound
//! `SculkSpreader`; Steel only has the private worldgen spreader
//! (`worldgen/feature/features/sculk_patch.rs`, `WorldGenRegion`-based), so this entity accumulates
//! the charge and marks spreading as a dependency.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::game_events::GameEventRef;
use steel_registry::{vanilla_block_entity_types, vanilla_game_events};
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::world::game_event::vibration::CATALYST_LISTENER_RADIUS;
use crate::world::game_event::{
    GameEventContext, GameEventDeliveryMode, GameEventListener, SharedGameEventListener,
};

/// Charge awaiting a spreader (Vanilla's `SculkSpreader` accumulator).
#[derive(Default)]
struct CatalystCharge {
    /// XP-valued charge collected since construction.
    charge: i32,
    /// Source positions retained for the future level-bound spreader cursors.
    cursors: Vec<BlockPos>,
}

/// Vanilla `SculkCatalystBlockEntity.CatalystListener`: immediate `entity_die` handling.
pub struct CatalystListener {
    pos: BlockPos,
    charge: SyncMutex<CatalystCharge>,
}

impl CatalystListener {
    /// Vanilla `CatalystListener.handleGameEvent` for `ENTITY_DIE`.
    fn on_entity_death(&self, world: &Arc<World>, context: &GameEventContext<'_>) -> bool {
        let Some(living) = context
            .source_entity()
            .and_then(|entity| entity.as_living_entity())
        else {
            return false;
        };
        // `LivingEntity::experience_reward(&World, Option<&dyn Entity>)`; the killer is not carried
        // through Steel's game-event context, so enchantment/weapon bonuses are not applied.
        let reward = living.experience_reward(world, None);
        if reward > 0 {
            let mut charge = self.charge.lock();
            charge.charge += reward;
            charge.cursors.push(self.pos);
        }
        // TODO(Entity): Vanilla checks `mob.wasExperienceConsumed()` / `shouldDropExperience()` and
        // calls `mob.skipDropExperience()`, so the death drops no XP orbs. Steel has no
        // consumed-experience flag, so the orbs still drop in addition to this charge.
        let state = world.get_block_state(self.pos);
        crate::behavior::blocks::SculkCatalystBlock::bloom(
            state.get_block(),
            world,
            self.pos,
            state,
        );
        // TODO(Worldgen): Vanilla also does `sculkSpreader.addCursors(sourceAbove, xp)` and per-tick
        // `updateCursors(level, pos, random, true)`. Wire the stored charge to a level-bound
        // spreader when the Worldgen lane exposes one.
        if let Some(entity) = world.get_block_entity(self.pos) {
            entity.set_changed();
        }
        true
    }
}

impl GameEventListener for CatalystListener {
    fn listener_pos(&self) -> Option<glam::DVec3> {
        Some(glam::DVec3::new(
            f64::from(self.pos.x()) + 0.5,
            f64::from(self.pos.y()) + 0.5,
            f64::from(self.pos.z()) + 0.5,
        ))
    }

    fn listener_radius(&self) -> i32 {
        CATALYST_LISTENER_RADIUS
    }

    fn delivery_mode(&self) -> GameEventDeliveryMode {
        GameEventDeliveryMode::ByDistance
    }

    fn handle_game_event(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        _source_pos: glam::DVec3,
    ) -> bool {
        if event.key != vanilla_game_events::ENTITY_DIE.key {
            return false;
        }
        self.on_entity_death(world, context)
    }
}

/// Vanilla `SculkCatalystBlockEntity`.
pub struct SculkCatalystBlockEntity {
    base: BlockEntityBase,
    listener: Arc<CatalystListener>,
}

// SAFETY: Steel-owned key uniquely identifies this concrete entity type.
unsafe impl DowncastType for SculkCatalystBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_catalyst");
}

impl SculkCatalystBlockEntity {
    /// Creates a catalyst block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_CATALYST,
                level,
                pos,
                state,
            ),
            listener: Arc::new(CatalystListener {
                pos,
                charge: SyncMutex::new(CatalystCharge::default()),
            }),
        }
    }

    /// Total unspread charge (Vanilla `SculkSpreader` cursor budget).
    #[must_use]
    pub fn pending_charge(&self) -> i32 {
        self.listener.charge.lock().charge
    }

    /// Cursor source positions recorded for the future level-bound spreader.
    #[must_use]
    pub fn charge_cursors(&self) -> Vec<BlockPos> {
        self.listener.charge.lock().cursors.clone()
    }

    /// The registered game-event listener.
    #[must_use]
    pub fn catalyst_listener(&self) -> &Arc<CatalystListener> {
        &self.listener
    }
}

impl BlockEntity for SculkCatalystBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        // Vanilla `SculkSpreader.load` reads its cursor list from the entity tag; Steel keeps the
        // accumulated charge plus the cursor positions in a simplified shape until cursors are
        // modelled with real charge/decay state.
        let mut charge = self.listener.charge.lock();
        if let Some(total) = nbt.int("charge") {
            charge.charge = total;
        }
        if let Some(cursors) = nbt.list("cursors").and_then(|list| list.compounds()) {
            charge.cursors.clear();
            for cursor in cursors {
                let x = cursor.int("x").unwrap_or(0);
                let y = cursor.int("y").unwrap_or(0);
                let z = cursor.int("z").unwrap_or(0);
                charge.cursors.push(BlockPos::new(x, y, z));
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let charge = self.listener.charge.lock();
        nbt.insert("charge", charge.charge);
        let cursors: Vec<NbtCompound> = charge
            .cursors
            .iter()
            .map(|pos| {
                let mut cursor = NbtCompound::new();
                cursor.insert("x", pos.x());
                cursor.insert("y", pos.y());
                cursor.insert("z", pos.z());
                cursor
            })
            .collect();
        nbt.insert("cursors", cursors);
    }

    fn tick(&self, _world: &Arc<World>) {
        // Vanilla `serverTick` runs `sculkSpreader.updateCursors(level, pos, random, true)`;
        // spreading is blocked on the Worldgen spreader API (see the TODO above). The 8-tick bloom
        // pulse is reset by `SculkCatalystBlock::tick`.
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }
}
