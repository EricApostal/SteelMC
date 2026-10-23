//! Vanilla `SculkCatalystBlockEntity`.
//!
//! Vanilla reference: `SculkCatalystBlockEntity`. Reacts immediately to `entity_die` within 8
//! blocks (no vibration travel), converts the victim's experience into sculk charge cursors
//! (`SculkSpreader.createLevelSpreader()`) and pulses the block's `bloom` state for 8 ticks.
//! Charge cursors and their per-tick spreading need a level-bound `SculkSpreader`; Steel only has
//! the private worldgen spreader (`worldgen/feature/features/sculk_patch.rs`,
//! `WorldGenRegion`-based), so this entity accumulates the vanilla cursor list and marks spreading
//! as a dependency.

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtTag};
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

/// Vanilla `SculkSpreader.MAX_CHARGE`.
const MAX_CHARGE: i32 = 1000;
/// Vanilla `SculkSpreader.MAX_CURSORS`.
const MAX_CURSORS: usize = 32;

/// Vanilla `SculkSpreader.ChargeCursor`. The vein `facings` are only consumed by the level-bound
/// spreader Steel does not have yet, so they are not modeled; `decay_delay`/`update_delay` are
/// persisted because vanilla's codec round-trips them.
#[derive(Clone, Copy)]
struct ChargeCursor {
    pos: BlockPos,
    charge: i32,
    /// Vanilla `decay_delay` (`0..=1`, default 1).
    decay_delay: i32,
    /// Vanilla `update_delay` (default 0).
    update_delay: i32,
}

/// Vanilla `SculkSpreader` cursor list for a level spreader.
#[derive(Default)]
struct CatalystCharge {
    cursors: Vec<ChargeCursor>,
}

impl CatalystCharge {
    /// Vanilla `SculkSpreader.addCursors`.
    fn add_cursors(&mut self, start_pos: BlockPos, mut charge: i32) {
        while charge > 0 {
            let current_charge = charge.min(MAX_CHARGE);
            self.add_cursor(ChargeCursor {
                pos: start_pos,
                charge: current_charge,
                // `ChargeCursor(BlockPos, int)`: `decayDelay = 1`, `updateDelay = 0`.
                decay_delay: 1,
                update_delay: 0,
            });
            charge -= current_charge;
        }
    }

    /// Vanilla `SculkSpreader.addCursor`.
    fn add_cursor(&mut self, cursor: ChargeCursor) {
        if self.cursors.len() < MAX_CURSORS {
            self.cursors.push(cursor);
        }
    }
}

/// Vanilla `SculkCatalystBlockEntity.CatalystListener`: immediate `entity_die` handling.
pub struct CatalystListener {
    pos: BlockPos,
    charge: SyncMutex<CatalystCharge>,
}

impl CatalystListener {
    /// Vanilla `CatalystListener.handleGameEvent` for `ENTITY_DIE`.
    fn on_entity_death(
        &self,
        world: &Arc<World>,
        context: &GameEventContext<'_>,
        source_pos: glam::DVec3,
    ) -> bool {
        let Some(living) = context
            .source_entity()
            .and_then(|entity| entity.as_living_entity())
        else {
            return false;
        };
        // `LivingEntity::experience_reward(&World, Option<&dyn Entity>)`; the killer is not carried
        // through Steel's game-event context, so enchantment/weapon bonuses are not applied.
        let reward = living.experience_reward(world, None);
        if !living.was_experience_consumed() {
            if living.should_drop_experience() && reward > 0 {
                // Vanilla `BlockPos.containing(sourcePosition.relative(Direction.UP, 0.5))`.
                let cursor_pos =
                    BlockPos::containing(source_pos.x, source_pos.y + 0.5, source_pos.z);
                self.charge.lock().add_cursors(cursor_pos, reward);
            }
            // Vanilla `mob.skipDropExperience()`: the death must not also drop XP orbs.
            living.skip_drop_experience();
            let state = world.get_block_state(self.pos);
            crate::behavior::blocks::SculkCatalystBlock::bloom(
                state.get_block(),
                world,
                self.pos,
                state,
            );
        }
        // TODO(Worldgen): Vanilla also runs `updateCursors(level, pos, random, true)` every tick,
        // which is what consumes the cursors above. Wire the cursor list to a level-bound
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
        source_pos: glam::DVec3,
    ) -> bool {
        if event.key != vanilla_game_events::ENTITY_DIE.key {
            return false;
        }
        self.on_entity_death(world, context, source_pos)
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

    /// Total unspread charge (sum over Vanilla `SculkSpreader` cursors).
    #[must_use]
    pub fn pending_charge(&self) -> i32 {
        self.listener
            .charge
            .lock()
            .cursors
            .iter()
            .map(|cursor| cursor.charge)
            .sum()
    }

    /// Cursor source positions recorded for the future level-bound spreader.
    #[must_use]
    pub fn charge_cursors(&self) -> Vec<BlockPos> {
        self.listener
            .charge
            .lock()
            .cursors
            .iter()
            .map(|cursor| cursor.pos)
            .collect()
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
        // Vanilla `SculkSpreader.load`: a size-limited list of `ChargeCursor` compounds.
        let mut charge = self.listener.charge.lock();
        charge.cursors.clear();
        if let Some(cursors) = nbt.list("cursors").and_then(|list| list.compounds()) {
            for cursor in cursors.take(MAX_CURSORS) {
                let Some(pos) = cursor.int_array("pos") else {
                    continue;
                };
                if pos.len() < 3 {
                    continue;
                }
                let cursor_charge = cursor.int("charge").unwrap_or(0).clamp(0, MAX_CHARGE);
                if cursor_charge <= 0 {
                    continue;
                }
                charge.cursors.push(ChargeCursor {
                    pos: BlockPos::new(pos[0], pos[1], pos[2]),
                    charge: cursor_charge,
                    // `optionalFieldOf("decay_delay", 1)` / `optionalFieldOf("update_delay", 0)`.
                    decay_delay: cursor.int("decay_delay").unwrap_or(1).clamp(0, 1),
                    update_delay: cursor.int("update_delay").unwrap_or(0).max(0),
                });
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Vanilla `SculkSpreader.save` writes only the cursor list; each cursor stores `pos` as a
        // block-position int array plus its `charge`.
        let charge = self.listener.charge.lock();
        let cursors: Vec<NbtCompound> = charge
            .cursors
            .iter()
            .map(|cursor| {
                let mut cursor_nbt = NbtCompound::new();
                cursor_nbt.insert(
                    "pos",
                    NbtTag::IntArray(vec![cursor.pos.x(), cursor.pos.y(), cursor.pos.z()]),
                );
                cursor_nbt.insert("charge", cursor.charge);
                // Vanilla's `optionalFieldOf` omits the delay fields at their defaults.
                if cursor.decay_delay != 1 {
                    cursor_nbt.insert("decay_delay", cursor.decay_delay);
                }
                if cursor.update_delay != 0 {
                    cursor_nbt.insert("update_delay", cursor.update_delay);
                }
                cursor_nbt
            })
            .collect();
        nbt.insert("cursors", cursors);
    }

    fn tick(&self, _world: &Arc<World>) {
        // Vanilla `serverTick` runs `sculkSpreader.updateCursors(level, pos, random, true)`;
        // spreading is blocked on the level-bound spreader API (see the TODO above). The 8-tick
        // bloom pulse is reset by `SculkCatalystBlock::tick`.
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }
}
