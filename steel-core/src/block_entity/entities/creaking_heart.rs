//! `CreakingHeartBlockEntity` (`net.minecraft.world.level.block.entity.CreakingHeartBlockEntity`).
//!
//! Persists the linked `Creaking` (entity or UUID until the entity loads) and the heart's tick
//! counters. The per-tick spawn/proximity logic is an entity-lane dependency
//! (`Creaking.mustDespawn`/spawn hearing), so `tick` keeps the counters accurate and leaves the
//! mob interaction as a reported block.

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// `CreakingHeartBlockEntity`.
pub struct CreakingHeartBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<CreakingHeartState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CreakingHeartBlockEntity`.
unsafe impl DowncastType for CreakingHeartBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/creaking_heart");
}

/// Vanilla `CreakingHeartBlockEntity` persistent state.
#[derive(Default)]
struct CreakingHeartState {
    /// Linked creaking UUID (`Either<Creaking, UUID>` persisted side).
    creaking: Option<uuid::Uuid>,
    /// Ticks since the link was established.
    ticks_existed: i64,
    /// Randomised 20-24 tick burst counter.
    ticker: i32,
}

impl CreakingHeartBlockEntity {
    /// Creates a creaking-heart block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::CREAKING_HEART,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(CreakingHeartState::default()),
        }
    }

    /// Vanilla `CreakingHeartBlockEntity.setCreakingInfo`: records the link and resets the timer.
    pub fn set_creaking_info(&self, uuid: uuid::Uuid) {
        let mut state = self.state.lock();
        state.creaking = Some(uuid);
        state.ticks_existed = 0;
        drop(state);
        BlockEntity::set_changed(self);
    }

    /// Vanilla `CreakingHeartBlockEntity.clearCreakingInfo`.
    pub fn clear_creaking_info(&self) {
        self.state.lock().creaking = None;
        BlockEntity::set_changed(self);
    }

    /// Returns the linked creaking UUID.
    #[must_use]
    pub fn creaking(&self) -> Option<uuid::Uuid> {
        self.state.lock().creaking
    }
}

impl BlockEntity for CreakingHeartBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.creaking = view
            .string("creaking")
            .and_then(|text| text.to_str().parse().ok());
        state.ticks_existed = 0;
        state.ticker = 0;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        if let Some(creaking) = state.creaking {
            nbt.insert("creaking", creaking.to_string());
        }
    }

    fn tick(&self, _world: &std::sync::Arc<World>) {
        let mut state = self.state.lock();
        state.ticks_existed += 1;
        state.ticker -= 1;
        if state.ticker >= 0 {
            return;
        }
        state.ticker = rand::random_range(20..25);
        // TODO(Entities): implement the creaking spawn/proximity loop
        // (`CreakingHeartBlockEntity.serverTick`: spawn creaking in a 32-block roaming radius,
        // distance-based hurt sounds, comparator output). Blocked on the `CREAKING` entity
        // factory (owner: entity lane / `PassiveAnimals` for passive-type factories).
        let _ = self.get_block_pos();
    }
}
