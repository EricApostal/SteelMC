//! `TrialSpawnerBlockEntity` (`net.minecraft.world.level.block.entity.TrialSpawnerBlockEntity`).
//!
//! Vanilla wraps a `TrialSpawner` state machine plus a `TrialSpawnerConfig`. The entity persists
//! the config, the ominous flag, the cooldown and the rewarded-player set; the per-tick state
//! machine lives in [`TrialSpawnerBlockEntity::tick`].

use std::sync::Weak;

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, TrialSpawnerState};
use steel_registry::vanilla_block_entity_types;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, locks::SyncMutex};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;

/// Vanilla `TrialSpawnerConfig.DEFAULT` values.
#[derive(Clone, Debug)]
pub struct TrialSpawnerConfig {
    /// Mobs spawned per wave.
    pub spawn_range: i32,
    /// Total mobs required to clear the trial.
    pub total_mobs: f32,
    /// Mobs alive at once.
    pub simultaneous_mobs: f32,
    /// Ticks between spawn attempts.
    pub ticks_between_spawns: i32,
    /// Additional mobs per player.
    pub total_mobs_added_per_player: f32,
    /// Additional simultaneous mobs per player.
    pub simultaneous_mobs_added_per_player: f32,
    /// Ticks between the detection particle pulses.
    pub ticks_between_player_particles: i32,
}

impl Default for TrialSpawnerConfig {
    fn default() -> Self {
        Self {
            spawn_range: 4,
            total_mobs: 6.0,
            simultaneous_mobs: 2.0,
            ticks_between_spawns: 40,
            total_mobs_added_per_player: 2.0,
            simultaneous_mobs_added_per_player: 1.0,
            ticks_between_player_particles: 20,
        }
    }
}

/// Mutable trial-spawner state.
struct TrialSpawnerData {
    config: TrialSpawnerConfig,
    /// Whether this spawner is the ominous variant.
    ominous: bool,
    /// Remaining cooldown ticks before it can activate again.
    cooldown_ticks: i32,
    /// Ticks spent in the current state.
    state_ticks: i32,
    /// Total mobs spawned in the current trial.
    spawned: i32,
    /// Mobs killed in the current trial.
    killed: i32,
    /// UUIDs of players already rewarded by this spawner.
    rewarded_players: Vec<uuid::Uuid>,
}

/// `TrialSpawnerBlockEntity`.
pub struct TrialSpawnerBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<TrialSpawnerData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TrialSpawnerBlockEntity`.
unsafe impl DowncastType for TrialSpawnerBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/trial_spawner");
}

impl TrialSpawnerBlockEntity {
    /// Creates a trial-spawner block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::TRIAL_SPAWNER,
                level,
                pos,
                state,
            ),
            state: SyncMutex::new(TrialSpawnerData {
                config: TrialSpawnerConfig::default(),
                ominous: false,
                cooldown_ticks: 0,
                state_ticks: 0,
                spawned: 0,
                killed: 0,
                rewarded_players: Vec::new(),
            }),
        }
    }

    /// Returns whether this spawner is the ominous variant.
    #[must_use]
    pub fn is_ominous(&self) -> bool {
        self.state.lock().ominous
    }

    /// Returns a snapshot of the configuration.
    #[must_use]
    pub fn config(&self) -> TrialSpawnerConfig {
        self.state.lock().config.clone()
    }

    /// Vanilla `TrialSpawner` state transition: picks the next state from the current one and
    /// whether a player is in range.
    fn next_state(
        current: TrialSpawnerState,
        has_player: bool,
        cooldown_ticks: i32,
    ) -> TrialSpawnerState {
        match current {
            TrialSpawnerState::Inactive => TrialSpawnerState::WaitingForPlayers,
            TrialSpawnerState::WaitingForPlayers => {
                if has_player {
                    TrialSpawnerState::Active
                } else {
                    TrialSpawnerState::WaitingForPlayers
                }
            }
            TrialSpawnerState::Active => TrialSpawnerState::WaitingForRewardEjection,
            TrialSpawnerState::WaitingForRewardEjection => TrialSpawnerState::EjectingReward,
            TrialSpawnerState::EjectingReward => TrialSpawnerState::Cooldown,
            TrialSpawnerState::Cooldown => {
                if cooldown_ticks <= 0 {
                    TrialSpawnerState::WaitingForPlayers
                } else {
                    TrialSpawnerState::Cooldown
                }
            }
        }
    }
}

impl BlockEntity for TrialSpawnerBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.ominous = view.byte("ominous").unwrap_or(0) != 0;
        state.cooldown_ticks = view.int("cooldown_ticks").unwrap_or(0);

        if let Some(config) = view.compound("normal_config") {
            state.config = TrialSpawnerConfig {
                spawn_range: config.int("spawn_range").unwrap_or(4),
                total_mobs: config.float("total_mobs").unwrap_or(6.0),
                simultaneous_mobs: config.float("simultaneous_mobs").unwrap_or(2.0),
                ticks_between_spawns: config.int("ticks_between_spawns").unwrap_or(40),
                total_mobs_added_per_player: config
                    .float("total_mobs_added_per_player")
                    .unwrap_or(2.0),
                simultaneous_mobs_added_per_player: config
                    .float("simultaneous_mobs_added_per_player")
                    .unwrap_or(1.0),
                ticks_between_player_particles: config
                    .int("ticks_between_player_particles")
                    .unwrap_or(20),
            };
        }

        state.rewarded_players.clear();
        if let Some(players) = view.list("rewarded_players")
            && let Some(texts) = players.strings()
        {
            for text in texts {
                if let Ok(uuid) = text.to_str().parse::<uuid::Uuid>() {
                    state.rewarded_players.push(uuid);
                }
            }
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        nbt.insert("ominous", i8::from(state.ominous));
        nbt.insert("cooldown_ticks", state.cooldown_ticks);

        let mut config = NbtCompound::new();
        config.insert("spawn_range", state.config.spawn_range);
        config.insert("total_mobs", state.config.total_mobs);
        config.insert("simultaneous_mobs", state.config.simultaneous_mobs);
        config.insert("ticks_between_spawns", state.config.ticks_between_spawns);
        config.insert(
            "total_mobs_added_per_player",
            state.config.total_mobs_added_per_player,
        );
        config.insert(
            "simultaneous_mobs_added_per_player",
            state.config.simultaneous_mobs_added_per_player,
        );
        config.insert(
            "ticks_between_player_particles",
            state.config.ticks_between_player_particles,
        );
        nbt.insert("normal_config", config);

        let rewarded = NbtList::String(
            state
                .rewarded_players
                .iter()
                .map(|uuid| uuid.to_string().into())
                .collect(),
        );
        nbt.insert("rewarded_players", rewarded);
    }

    fn tick(&self, world: &std::sync::Arc<World>) {
        let pos = self.get_block_pos();
        let block_state = self.get_block_state();
        let current = block_state.get_value(&BlockStateProperties::TRIAL_SPAWNER_STATE);

        let has_player = {
            let config = self.state.lock().config.clone();
            let range = f64::from(config.spawn_range.max(1) + 4);
            world
                .nearest_player_distance_sqr(glam::DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()) + 0.5,
                    f64::from(pos.z()) + 0.5,
                ))
                .is_some_and(|distance| distance <= range * range)
        };

        let (cooldown_ticks, next) = {
            let mut state = self.state.lock();
            state.state_ticks += 1;
            if current == TrialSpawnerState::Cooldown && state.cooldown_ticks > 0 {
                state.cooldown_ticks -= 1;
            }
            let next = Self::next_state(current, has_player, state.cooldown_ticks);
            (state.cooldown_ticks, next)
        };
        let _ = cooldown_ticks;

        if next == current {
            return;
        }

        world.set_block(
            pos,
            block_state.set_value(&BlockStateProperties::TRIAL_SPAWNER_STATE, next),
            steel_utils::types::UpdateFlags::UPDATE_ALL,
        );
        {
            let mut state = self.state.lock();
            state.state_ticks = 0;
        }

        // TODO(Entities/Loot): spawn the configured mobs during `Active` (level events
        // `PARTICLES_TRIAL_SPAWNER_SPAWN`, `PARTICLES_TRIAL_SPAWNER_DETECT_PLAYER`) and eject the
        // loot table during `EjectingReward` (`ANIMATION_TRIAL_SPAWNER_EJECT_ITEM`). Both need the
        // entity factory (`EntityTypeRef` → NBT spawn) and a runtime loot-table roll from a block
        // behavior; see the spawner entity for the same dependency.
    }
}
