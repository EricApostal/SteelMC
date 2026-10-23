//! Vanilla `DifficultyInstance` and `ServerLevel.getCurrentDifficultyAt`.

use rustc_hash::FxHashMap;
use steel_registry::vanilla_timelines;
use steel_utils::types::Difficulty;
use steel_utils::{BlockPos, ChunkPos, locks::SyncRwLock};

use crate::world::World;

/// Vanilla `DimensionType.MOON_BRIGHTNESS_PER_PHASE`, indexed by moon phase.
pub const MOON_BRIGHTNESS_PER_PHASE: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];

/// Vanilla `DifficultyInstance.DIFFICULTY_TIME_GLOBAL_OFFSET`.
const DIFFICULTY_TIME_GLOBAL_OFFSET: f32 = -72_000.0;
/// Vanilla `DifficultyInstance.MAX_DIFFICULTY_TIME_GLOBAL`.
const MAX_DIFFICULTY_TIME_GLOBAL: f32 = 1_440_000.0;
/// Vanilla `DifficultyInstance.MAX_DIFFICULTY_TIME_LOCAL`.
const MAX_DIFFICULTY_TIME_LOCAL: f32 = 3_600_000.0;

/// Vanilla `DifficultyInstance`: the difficulty actually used for spawn-time
/// equipment, mob effects, and the other regional difficulty rolls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DifficultyInstance {
    base: Difficulty,
    effective_difficulty: f32,
}

impl DifficultyInstance {
    /// Vanilla `DifficultyInstance(Difficulty, long, long, float)`.
    #[must_use]
    pub fn new(
        base: Difficulty,
        total_game_time: i64,
        local_game_time: i64,
        moon_brightness: f32,
    ) -> Self {
        Self {
            base,
            effective_difficulty: calculate_difficulty(
                base,
                total_game_time,
                local_game_time,
                moon_brightness,
            ),
        }
    }

    /// Vanilla `DifficultyInstance.getDifficulty`.
    #[must_use]
    pub const fn difficulty(self) -> Difficulty {
        self.base
    }

    /// Vanilla `DifficultyInstance.getEffectiveDifficulty`.
    #[must_use]
    pub const fn effective_difficulty(self) -> f32 {
        self.effective_difficulty
    }

    /// Vanilla `DifficultyInstance.isHard`.
    #[must_use]
    pub fn is_hard(self) -> bool {
        self.effective_difficulty >= u8::from(Difficulty::Hard) as f32
    }

    /// Vanilla `DifficultyInstance.isHarderThan`.
    #[must_use]
    pub fn is_harder_than(self, required_difficulty: f32) -> bool {
        self.effective_difficulty > required_difficulty
    }

    /// Vanilla `DifficultyInstance.getSpecialMultiplier`.
    #[must_use]
    pub fn special_multiplier(self) -> f32 {
        if self.effective_difficulty < 2.0 {
            0.0
        } else if self.effective_difficulty > 4.0 {
            1.0
        } else {
            (self.effective_difficulty - 2.0) / 2.0
        }
    }
}

/// Vanilla `DifficultyInstance.calculateDifficulty`.
fn calculate_difficulty(
    base: Difficulty,
    total_game_time: i64,
    local_game_time: i64,
    moon_brightness: f32,
) -> f32 {
    if base == Difficulty::Peaceful {
        return 0.0;
    }

    let is_hard = base == Difficulty::Hard;
    let global_scale = (((total_game_time as f32) + DIFFICULTY_TIME_GLOBAL_OFFSET)
        / MAX_DIFFICULTY_TIME_GLOBAL)
        .clamp(0.0, 1.0)
        * 0.25;
    let mut scale = 0.75 + global_scale;

    let mut local_scale = ((local_game_time as f32) / MAX_DIFFICULTY_TIME_LOCAL).clamp(0.0, 1.0)
        * if is_hard { 1.0 } else { 0.75 };
    local_scale += (moon_brightness * 0.25).clamp(0.0, global_scale);
    if base == Difficulty::Easy {
        local_scale *= 0.5;
    }

    scale += local_scale;
    f32::from(u8::from(base)) * scale
}

/// Vanilla `MoonPhase` index for a clock time, read from the data-driven
/// `minecraft:moon` timeline so a datapack change is honoured.
#[must_use]
pub fn moon_phase_index(total_ticks: i64) -> usize {
    const PHASE_NAMES: [&str; 8] = [
        "full_moon",
        "waning_gibbous",
        "third_quarter",
        "waning_crescent",
        "new_moon",
        "waxing_crescent",
        "first_quarter",
        "waxing_gibbous",
    ];

    let timeline = &vanilla_timelines::MOON;
    let ticks = timeline.current_ticks(total_ticks);

    let Some(track) = timeline
        .tracks
        .iter()
        .find(|track| track.name == "visual/moon_phase")
    else {
        return 0;
    };

    let mut index = 0;
    for keyframe in track.keyframes {
        if keyframe.ticks > ticks {
            break;
        }
        if let steel_registry::timeline::KeyframeValue::String(name) = keyframe.value
            && let Some(found) = PHASE_NAMES.iter().position(|phase| *phase == name)
        {
            index = found;
        }
    }
    index
}

/// Vanilla `ServerLevel.getMoonBrightness`.
#[must_use]
pub fn moon_brightness(total_ticks: i64) -> f32 {
    MOON_BRIGHTNESS_PER_PHASE[moon_phase_index(total_ticks)]
}

/// Per-chunk inhabited time, incremented once per spawning chunk per tick.
///
/// Vanilla stores this on the chunk and persists it with the chunk payload;
/// Steel's chunk format has no inhabited-time field yet, so this table keeps the
/// value for the lifetime of the world and a restart resets it to zero.
pub struct ChunkInhabitedTimes {
    times: SyncRwLock<FxHashMap<ChunkPos, i64>>,
}

impl Default for ChunkInhabitedTimes {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkInhabitedTimes {
    /// Creates an empty table.
    #[must_use]
    pub fn new() -> Self {
        Self {
            times: SyncRwLock::new(FxHashMap::default()),
        }
    }

    /// Vanilla `ChunkAccess.incrementInhabitedTime`.
    pub fn increment(&self, pos: ChunkPos) {
        *self.times.write().entry(pos).or_insert(0) += 1;
    }

    /// Vanilla `ChunkAccess.getInhabitedTime`.
    #[must_use]
    pub fn get(&self, pos: ChunkPos) -> i64 {
        self.times.read().get(&pos).copied().unwrap_or(0)
    }
}

impl World {
    /// Vanilla `ServerLevel.getCurrentDifficultyAt`.
    #[must_use]
    pub fn current_difficulty_at(&self, pos: BlockPos) -> DifficultyInstance {
        let chunk_pos = ChunkPos::from_block_pos(pos);
        let local_time = self.chunk_inhabited_time(chunk_pos);
        DifficultyInstance::new(
            self.difficulty(),
            self.overworld_clock_time(),
            local_time,
            moon_brightness(self.overworld_clock_time()),
        )
    }

    /// Vanilla `Level.getOverworldClockTime`.
    #[must_use]
    pub fn overworld_clock_time(&self) -> i64 {
        self.clock_total_ticks(&steel_registry::vanilla_world_clocks::OVERWORLD)
            .unwrap_or_else(|| self.game_time())
    }

    /// Vanilla `ChunkAccess.getInhabitedTime`.
    #[must_use]
    pub fn chunk_inhabited_time(&self, pos: ChunkPos) -> i64 {
        self.inhabited_times.get(pos)
    }

    /// Vanilla `LevelChunk.incrementInhabitedTime`.
    pub fn increment_chunk_inhabited_time(&self, pos: ChunkPos) {
        self.inhabited_times.increment(pos);
    }
}
