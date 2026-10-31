//! Level-bound Vanilla `SculkSpreader`.
//!
//! Vanilla has one `SculkSpreader` type shared by world generation (its
//! `createWorldGenSpreader`) and by live catalysts (`createLevelSpreader`). Steel's worldgen
//! feature keeps its own private copy inside `worldgen::feature::features::sculk_patch`, because
//! it writes through a `WorldGenRegion`; this module is the live-level foundation the catalyst
//! block entity drives: cursor state, per-tick `ChargeCursor.update`, charge consumption and the
//! cursor merge rules, all against `&Arc<World>`.
//!
//! The per-block rules that vanilla keeps on `SculkBehaviour` live on the block behaviors:
//! [`SculkBlock::attempt_use_charge`](super::SculkBlock::attempt_use_charge) and
//! [`SculkVeinBlock::attempt_use_charge`](crate::behavior::blocks::SculkVeinBlock::attempt_use_charge).
//! [`SculkVeinBlock`] also owns the multiface vein spreader (`spread_veins`) and
//! `attempt_place_sculk`, mirroring vanilla's `SculkVeinBlock` inner spreader configuration.

use std::mem;
use std::sync::Arc;

use rand::Rng;
use rustc_hash::FxHashMap;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::{level_events, sound_events, vanilla_fluids};
use steel_utils::axis::Axis;
use steel_utils::{BlockPos, BlockStateId, Identifier};

use crate::behavior::blocks::{MultifaceBlock, SculkVeinBlock, next_int_bounded};
use crate::world::{LevelAccessor, LevelReader, World};

use super::SculkBlock;

/// Vanilla `SculkSpreader.MAX_CHARGE`.
const MAX_CHARGE: i32 = 1000;
/// Vanilla `SculkSpreader.MAX_CURSORS`.
const MAX_CURSORS: usize = 32;
/// Vanilla `SculkSpreader.ChargeCursor.MAX_CURSOR_DECAY_DELAY`.
const MAX_CURSOR_DECAY_DELAY: i32 = 1;
/// Vanilla `SculkSpreader.ChargeCursor.MAX_CURSOR_DISTANCE`.
const MAX_CURSOR_DISTANCE: i32 = 1024;
/// Vanilla `SculkSpreader.ChargeCursor.MAX_WORLDGEN_SPREAD` (`12`).
const MAX_WORLDGEN_SPREAD: i64 = 12;

/// Vanilla `SculkBehaviour` implementation selected by a block state.
///
/// Vanilla dispatches these methods through a `SculkBehaviour` interface object; Steel keeps the
/// concrete behavior on the owning block types and selects it here.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SculkBehaviourKind {
    /// `SculkBehaviour.DEFAULT` (every block that is not a sculk behaviour).
    Default,
    /// [`SculkBlock`].
    Sculk,
    /// [`SculkVeinBlock`].
    SculkVein,
}

impl SculkBehaviourKind {
    /// Vanilla `ChargeCursor.getBlockBehaviour(state)`.
    pub(crate) fn of(state: BlockStateId) -> Self {
        if state.get_block() == &vanilla_blocks::SCULK {
            Self::Sculk
        } else if state.get_block() == &vanilla_blocks::SCULK_VEIN {
            Self::SculkVein
        } else {
            Self::Default
        }
    }

    /// Vanilla `SculkBehaviour.canChangeBlockStateOnSpread`.
    ///
    /// Only `SculkBlock` returns `false`; the interface default (and `SculkVeinBlock`) is `true`.
    pub(crate) const fn can_change_block_state_on_spread(self) -> bool {
        !matches!(self, Self::Sculk)
    }

    /// Vanilla `SculkBehaviour.updateDecayDelay`.
    const fn update_decay_delay(self, decay_delay: i32) -> i32 {
        match self {
            Self::Default => {
                if decay_delay > 1 {
                    decay_delay - 1
                } else {
                    0
                }
            }
            Self::Sculk | Self::SculkVein => MAX_CURSOR_DECAY_DELAY,
        }
    }
}

/// Vanilla `SculkSpreader.createLevelSpreader()` configuration.
///
/// The level spreader uses `#minecraft:sculk_replaceable`, `growthSpawnCost = 10`,
/// `noGrowthRadius = 4`, `chargeDecayRate = 10` and `additionalDecayRate = 5`.
pub struct SculkSpreader {
    is_world_generation: bool,
    replaceable_blocks: Identifier,
    growth_spawn_cost: i32,
    no_growth_radius: i32,
    charge_decay_rate: i32,
    additional_decay_rate: i32,
    cursors: Vec<ChargeCursor>,
}

impl SculkSpreader {
    /// Vanilla `SculkSpreader.createLevelSpreader()`.
    #[must_use]
    pub const fn level() -> Self {
        Self {
            is_world_generation: false,
            replaceable_blocks: BlockTag::SCULK_REPLACEABLE,
            growth_spawn_cost: 10,
            no_growth_radius: 4,
            charge_decay_rate: 10,
            additional_decay_rate: 5,
            cursors: Vec::new(),
        }
    }

    /// Vanilla `isWorldGeneration`.
    #[must_use]
    pub const fn is_world_generation(&self) -> bool {
        self.is_world_generation
    }

    /// Vanilla `replaceableBlocks`.
    #[must_use]
    pub const fn replaceable_blocks(&self) -> &Identifier {
        &self.replaceable_blocks
    }

    /// Vanilla `growthSpawnCost`.
    #[must_use]
    pub const fn growth_spawn_cost(&self) -> i32 {
        self.growth_spawn_cost
    }

    /// Vanilla `noGrowthRadius`.
    #[must_use]
    pub const fn no_growth_radius(&self) -> i32 {
        self.no_growth_radius
    }

    /// Vanilla `chargeDecayRate`.
    #[must_use]
    pub const fn charge_decay_rate(&self) -> i32 {
        self.charge_decay_rate
    }

    /// Vanilla `additionalDecayRate`.
    #[must_use]
    pub const fn additional_decay_rate(&self) -> i32 {
        self.additional_decay_rate
    }

    /// The live cursor list.
    #[must_use]
    pub fn cursors(&self) -> &[ChargeCursor] {
        &self.cursors
    }

    /// Replaces the cursor list, used by [`Self::load`].
    fn replace_cursors(&mut self, cursors: Vec<ChargeCursor>) {
        self.cursors = cursors;
    }

    /// Vanilla `addCursors`: split `charge` into `MAX_CHARGE`-sized cursors.
    pub fn add_cursors(&mut self, start_pos: BlockPos, mut charge: i32) {
        while charge > 0 {
            let current_charge = charge.min(MAX_CHARGE);
            self.add_cursor(ChargeCursor::new(start_pos, current_charge));
            charge -= current_charge;
        }
    }

    /// Vanilla `addCursor`: the list is capped at `MAX_CURSORS`.
    fn add_cursor(&mut self, cursor: ChargeCursor) {
        if self.cursors.len() < MAX_CURSORS {
            self.cursors.push(cursor);
        }
    }

    /// Vanilla `SculkSpreader.load`.
    pub fn load(&mut self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let mut cursors = Vec::new();
        if let Some(entries) = nbt.list("cursors").and_then(|list| list.compounds()) {
            for cursor in entries.into_iter().take(MAX_CURSORS) {
                let Some(pos) = cursor.int_array("pos") else {
                    continue;
                };
                if pos.len() < 3 {
                    continue;
                }
                let charge = cursor.int("charge").unwrap_or(0);
                if !(0..=MAX_CHARGE).contains(&charge) {
                    continue;
                }
                let decay_delay = cursor.int("decay_delay").unwrap_or(MAX_CURSOR_DECAY_DELAY);
                let decay_delay = decay_delay.clamp(0, MAX_CURSOR_DECAY_DELAY);
                let update_delay = cursor.int("update_delay").unwrap_or(0).max(0);
                let facings = cursor
                    .list("facings")
                    .and_then(|list| list.strings())
                    .map(|names| {
                        names
                            .iter()
                            .filter_map(|name| direction_from_name(name.to_str().as_ref()))
                            .collect::<Vec<_>>()
                    });
                cursors.push(ChargeCursor {
                    pos: BlockPos::new(pos[0], pos[1], pos[2]),
                    charge,
                    update_delay,
                    decay_delay,
                    facings,
                });
            }
        }
        self.replace_cursors(cursors);
    }

    /// Vanilla `SculkSpreader.save`: only the cursor list is persisted.
    pub fn save(&self, nbt: &mut NbtCompound) {
        let cursors: Vec<NbtCompound> = self
            .cursors
            .iter()
            .map(|cursor| {
                let mut cursor_nbt = NbtCompound::new();
                cursor_nbt.insert(
                    "pos",
                    NbtTag::IntArray(vec![cursor.pos.x(), cursor.pos.y(), cursor.pos.z()]),
                );
                // Vanilla's codec omits the optional fields at their defaults.
                if cursor.charge != 0 {
                    cursor_nbt.insert("charge", cursor.charge);
                }
                if cursor.decay_delay != MAX_CURSOR_DECAY_DELAY {
                    cursor_nbt.insert("decay_delay", cursor.decay_delay);
                }
                if cursor.update_delay != 0 {
                    cursor_nbt.insert("update_delay", cursor.update_delay);
                }
                if let Some(facings) = &cursor.facings {
                    cursor_nbt.insert(
                        "facings",
                        NbtList::String(
                            facings
                                .iter()
                                .map(|direction| direction.as_str().into())
                                .collect(),
                        ),
                    );
                }
                cursor_nbt
            })
            .collect();
        nbt.insert("cursors", cursors);
    }

    /// Vanilla `updateCursors`.
    ///
    /// Each surviving cursor is updated, dropped when discharged, then merged with every other
    /// cursor that ended on the same block position (same charge cap as vanilla) before the
    /// sculk-charge particle event is emitted.
    pub fn update_cursors(
        &mut self,
        world: &Arc<World>,
        origin: BlockPos,
        rng: &mut dyn Rng,
        spread_veins: bool,
    ) {
        if self.cursors.is_empty() {
            return;
        }

        let mut cursors = mem::take(&mut self.cursors);
        let mut alive: Vec<usize> = Vec::new();
        let mut pos_to_cursor: FxHashMap<BlockPos, usize> = FxHashMap::default();
        let mut charge_counts: FxHashMap<BlockPos, i32> = FxHashMap::default();

        for index in 0..cursors.len() {
            if cursor_is_pos_unreasonable(cursors[index].pos, origin) {
                continue;
            }

            cursors[index].update(world, origin, rng, &*self, spread_veins);
            if cursors[index].charge <= 0 {
                // Vanilla `levelEvent(3006, pos, 0)`: a spent cursor's charge particles stop.
                world.level_event(
                    level_events::PARTICLES_SCULK_CHARGE,
                    cursors[index].pos,
                    0,
                    None,
                );
                continue;
            }

            let pos = cursors[index].pos;
            *charge_counts.entry(pos).or_insert(0) += cursors[index].charge;
            match pos_to_cursor.get(&pos).copied() {
                None => {
                    pos_to_cursor.insert(pos, index);
                    alive.push(index);
                }
                Some(existing) => {
                    if !self.is_world_generation
                        && cursors[index].charge + cursors[existing].charge <= MAX_CHARGE
                    {
                        let other_charge = cursors[index].charge;
                        let other_update_delay = cursors[index].update_delay;
                        cursors[existing].merge_with(other_charge, other_update_delay);
                        cursors[index].charge = 0;
                    } else {
                        alive.push(index);
                        if cursors[index].charge < cursors[existing].charge {
                            pos_to_cursor.insert(pos, index);
                        }
                    }
                }
            }
        }

        for (pos, total) in &charge_counts {
            let Some(&index) = pos_to_cursor.get(pos) else {
                continue;
            };
            let Some(facings) = cursors[index].facings.as_ref() else {
                continue;
            };
            if *total > 0 {
                // Vanilla: `(int)(Math.log1p(total) / 2.3) + 1`, shifted above the face mask.
                let amount = (f64::from(*total).ln_1p() / 2.3) as i32 + 1;
                let data = (amount << 6) + multiface_pack(facings);
                world.level_event(level_events::PARTICLES_SCULK_CHARGE, *pos, data, None);
            }
        }

        self.cursors = alive
            .into_iter()
            .map(|index| cursors[index].clone())
            .collect();
    }
}

/// Vanilla `SculkSpreader.ChargeCursor`.
#[derive(Clone)]
pub struct ChargeCursor {
    pos: BlockPos,
    charge: i32,
    update_delay: i32,
    decay_delay: i32,
    /// Vanilla `facings`: `None` before the cursor first reaches a sculk block, then the
    /// `MultifaceBlock.availableFaces` of the block it stands on (possibly empty).
    facings: Option<Vec<Direction>>,
}

impl ChargeCursor {
    /// Vanilla `ChargeCursor(BlockPos, int)`: `decayDelay = 1`, `updateDelay = 0`, no facings.
    #[must_use]
    pub const fn new(pos: BlockPos, charge: i32) -> Self {
        Self {
            pos,
            charge,
            update_delay: 0,
            decay_delay: MAX_CURSOR_DECAY_DELAY,
            facings: None,
        }
    }

    /// The cursor block position.
    #[must_use]
    pub const fn pos(&self) -> BlockPos {
        self.pos
    }

    /// The remaining charge (at most `MAX_CHARGE`, 1000).
    #[must_use]
    pub const fn charge(&self) -> i32 {
        self.charge
    }

    /// Vanilla `getDecayDelay`.
    #[must_use]
    pub const fn decay_delay(&self) -> i32 {
        self.decay_delay
    }

    /// Vanilla `mergeWith`: the other cursor's charge is moved into this one.
    fn merge_with(&mut self, other_charge: i32, other_update_delay: i32) {
        self.charge += other_charge;
        self.update_delay = self.update_delay.min(other_update_delay);
    }

    /// Vanilla `ChargeCursor.shouldUpdate`.
    fn should_update(&self, world: &World) -> bool {
        if self.charge <= 0 {
            return false;
        }
        // Level spreaders follow vanilla `ServerLevel.shouldTickBlocksAt`; the cursor only
        // advances inside block-ticking chunks.
        world.is_block_ticking_chunk_loaded(self.pos)
    }

    /// Vanilla `ChargeCursor.update`.
    fn update(
        &mut self,
        world: &Arc<World>,
        origin: BlockPos,
        rng: &mut dyn Rng,
        spreader: &SculkSpreader,
        spread_veins: bool,
    ) {
        let should_update = if spreader.is_world_generation {
            self.charge > 0
        } else {
            self.should_update(world)
        };
        if !should_update {
            return;
        }
        if self.update_delay > 0 {
            self.update_delay -= 1;
            return;
        }

        let mut state = world.get_block_state(self.pos);
        let mut behavior = SculkBehaviourKind::of(state);
        if spread_veins
            && attempt_spread_vein(world, self.pos, state, self.facings.as_deref(), behavior)
        {
            if behavior.can_change_block_state_on_spread() {
                state = world.get_block_state(self.pos);
                behavior = SculkBehaviourKind::of(state);
            }
            // Vanilla `LevelAccessor.playSound(null, pos, SoundEvents.SCULK_BLOCK_SPREAD, ...)`.
            world.play_sound(
                &sound_events::BLOCK_SCULK_SPREAD,
                SoundSource::Blocks,
                self.pos,
                1.0,
                1.0,
                None,
            );
        }

        self.charge =
            attempt_use_charge(world, rng, origin, spreader, self, spread_veins, behavior);
        if self.charge <= 0 {
            on_discharged(world, behavior, state, self.pos, rng);
            return;
        }

        if let Some(transfer_pos) = get_valid_movement_pos(world, self.pos, origin, rng, spreader) {
            on_discharged(world, behavior, state, self.pos, rng);
            self.pos = transfer_pos;
            state = world.get_block_state(transfer_pos);
        } else if spreader.is_world_generation {
            on_discharged(world, behavior, state, self.pos, rng);
            self.charge = 0;
            return;
        }

        if SculkBehaviourKind::of(state) != SculkBehaviourKind::Default {
            self.facings = Some(available_faces(state));
        }
        self.decay_delay = behavior.update_decay_delay(self.decay_delay);
        // Vanilla `SculkBehaviour.getSculkSpreadDelay` default.
        self.update_delay = 1;
    }
}

/// Vanilla `SculkBehaviour.DEFAULT.attemptSpreadVein` / the interface default, selected by the
/// block state's behavior.
fn attempt_spread_vein(
    world: &Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    facings: Option<&[Direction]>,
    behavior: SculkBehaviourKind,
) -> bool {
    let level: &dyn LevelAccessor = world;
    match behavior {
        // Vanilla `SculkBehaviour.DEFAULT` anonymous class.
        SculkBehaviourKind::Default => match facings {
            None => SculkVeinBlock::spread_veins(level, state, pos, true) > 0,
            Some(faces) if !faces.is_empty() => {
                // `state.isAir() || state.getFluidState().is(Fluids.WATER)`.
                let fluid = state.get_fluid_state();
                if !state.is_air() && !fluid.is_source_of_type(&vanilla_fluids::WATER) {
                    return false;
                }
                SculkVeinBlock::regrow(level, pos, state, faces)
            }
            Some(_) => SculkVeinBlock::spread_veins(level, state, pos, false) > 0,
        },
        // `SculkBlock` / `SculkVeinBlock` inherit the interface default: the vein spreader.
        SculkBehaviourKind::Sculk | SculkBehaviourKind::SculkVein => {
            SculkVeinBlock::spread_veins(level, state, pos, false) > 0
        }
    }
}

/// Vanilla `SculkBehaviour.attemptUseCharge`, dispatched to the concrete behavior.
fn attempt_use_charge(
    world: &Arc<World>,
    rng: &mut dyn Rng,
    origin: BlockPos,
    spreader: &SculkSpreader,
    cursor: &ChargeCursor,
    spread_veins: bool,
    behavior: SculkBehaviourKind,
) -> i32 {
    match behavior {
        SculkBehaviourKind::Default => {
            if cursor.decay_delay > 0 {
                cursor.charge
            } else {
                0
            }
        }
        SculkBehaviourKind::Sculk => {
            SculkBlock::attempt_use_charge(world, rng, origin, spreader, cursor)
        }
        SculkBehaviourKind::SculkVein => {
            SculkVeinBlock::attempt_use_charge(world, rng, spreader, spread_veins, cursor)
        }
    }
}

/// Vanilla `SculkBehaviour.onDischarged` (only `SculkVeinBlock` overrides it).
fn on_discharged(
    world: &Arc<World>,
    behavior: SculkBehaviourKind,
    state: BlockStateId,
    pos: BlockPos,
    rng: &mut dyn Rng,
) {
    if behavior == SculkBehaviourKind::SculkVein {
        SculkVeinBlock::on_discharged(world, state, pos, rng);
    }
}

/// Vanilla `ChargeCursor.getValidMovementPos`.
fn get_valid_movement_pos(
    world: &Arc<World>,
    pos: BlockPos,
    origin: BlockPos,
    rng: &mut dyn Rng,
    spreader: &SculkSpreader,
) -> Option<BlockPos> {
    let mut sculk_position = pos;
    for offset in randomized_non_corner_neighbour_offsets(rng) {
        let neighbor = pos.offset(offset.x(), offset.y(), offset.z());
        if !can_move_to_pos(origin, neighbor, spreader) {
            continue;
        }
        let transferee = world.get_block_state(neighbor);
        if SculkBehaviourKind::of(transferee) == SculkBehaviourKind::Default
            || !is_movement_unobstructed(world, pos, neighbor)
        {
            continue;
        }

        sculk_position = neighbor;
        // Vanilla `SculkVeinBlock.hasSubstrateAccess`: a vein with a `#sculk_replaceable`
        // neighbour is the preferred stopping point.
        if SculkVeinBlock::has_substrate_access(world, transferee, neighbor) {
            break;
        }
    }

    (sculk_position != pos).then_some(sculk_position)
}

/// Vanilla `ChargeCursor.canMoveToPos`: worldgen cursors stay within 12 blocks horizontally.
fn can_move_to_pos(origin: BlockPos, pos: BlockPos, spreader: &SculkSpreader) -> bool {
    if !spreader.is_world_generation {
        return true;
    }
    let dx = i64::from(origin.x()) - i64::from(pos.x());
    let dz = i64::from(origin.z()) - i64::from(pos.z());
    dx * dx + dz * dz <= MAX_WORLDGEN_SPREAD * MAX_WORLDGEN_SPREAD
}

fn is_movement_unobstructed(world: &Arc<World>, from: BlockPos, to: BlockPos) -> bool {
    if manhattan_distance(from, to) == 1 {
        return true;
    }

    let delta_x = to.x() - from.x();
    let delta_y = to.y() - from.y();
    let delta_z = to.z() - from.z();
    let direction_x = direction_from_axis_delta(Axis::X, delta_x);
    let direction_y = direction_from_axis_delta(Axis::Y, delta_y);
    let direction_z = direction_from_axis_delta(Axis::Z, delta_z);
    if delta_x == 0 {
        is_unobstructed(world, from, direction_y) || is_unobstructed(world, from, direction_z)
    } else if delta_y == 0 {
        is_unobstructed(world, from, direction_x) || is_unobstructed(world, from, direction_z)
    } else {
        is_unobstructed(world, from, direction_x) || is_unobstructed(world, from, direction_y)
    }
}

fn is_unobstructed(world: &Arc<World>, from: BlockPos, direction: Direction) -> bool {
    let test_pos = from.relative(direction);
    !world.is_face_sturdy(
        world.get_block_state(test_pos),
        test_pos,
        direction.opposite(),
    )
}

const fn direction_from_axis_delta(axis: Axis, delta: i32) -> Direction {
    match (axis, delta < 0) {
        (Axis::X, true) => Direction::West,
        (Axis::X, false) => Direction::East,
        (Axis::Y, true) => Direction::Down,
        (Axis::Y, false) => Direction::Up,
        (Axis::Z, true) => Direction::North,
        (Axis::Z, false) => Direction::South,
    }
}

/// Vanilla `ChargeCursor.getRandomizedNonCornerNeighbourOffsets`.
fn randomized_non_corner_neighbour_offsets(rng: &mut dyn Rng) -> Vec<BlockPos> {
    let mut offsets = Vec::with_capacity(18);
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                if (x == 0 || y == 0 || z == 0) && (x != 0 || y != 0 || z != 0) {
                    offsets.push(BlockPos::new(x, y, z));
                }
            }
        }
    }
    for i in (1..offsets.len()).rev() {
        let j = next_int_bounded(rng, i as i32 + 1) as usize;
        offsets.swap(i, j);
    }
    offsets
}

/// Vanilla `ChargeCursor.isPosUnreasonable`: cursors further than 1024 blocks (chessboard) from
/// the catalyst are discarded.
fn cursor_is_pos_unreasonable(pos: BlockPos, origin: BlockPos) -> bool {
    (pos.x() - origin.x())
        .abs()
        .max((pos.y() - origin.y()).abs())
        .max((pos.z() - origin.z()).abs())
        > MAX_CURSOR_DISTANCE
}

/// Vanilla `MultifaceBlock.availableFaces`.
pub(crate) fn available_faces(state: BlockStateId) -> Vec<Direction> {
    if state.get_block() != &vanilla_blocks::SCULK_VEIN {
        return Vec::new();
    }
    Direction::ALL
        .into_iter()
        .filter(|direction| MultifaceBlock::has_face(state, *direction))
        .collect()
}

/// Vanilla `MultifaceBlock.pack`: one bit per direction ordinal.
fn multiface_pack(facings: &[Direction]) -> i32 {
    facings.iter().fold(0i32, |mask, direction| {
        mask | (1 << direction.get_3d_data_value())
    })
}

const fn manhattan_distance(left: BlockPos, right: BlockPos) -> i32 {
    (left.x() - right.x()).abs() + (left.y() - right.y()).abs() + (left.z() - right.z()).abs()
}

/// Parses vanilla's lowercase direction names used by the `ChargeCursor` codec.
fn direction_from_name(name: &str) -> Option<Direction> {
    Some(match name {
        "down" => Direction::Down,
        "up" => Direction::Up,
        "north" => Direction::North,
        "south" => Direction::South,
        "west" => Direction::West,
        "east" => Direction::East,
        _ => return None,
    })
}
