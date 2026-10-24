//! Vanilla fire spread, aging and burnout.
//!
//! Ports `FireBlock.tick` (`/tmp/vanilla/src/world/level/block/FireBlock.java`) together with its
//! flammability table (`FireBlock.bootStrap`), the `FireBlock.isNearRain` neighborhood check and
//! the `ServerLevel.canSpreadFireAround` gate. The fire block behavior in
//! [`crate::behavior::blocks::portal::fire`] schedules the tick and delegates the body here.
//!
//! Deliberate divergence: vanilla's flammability odds are Java code in `FireBlock.bootStrap` and are
//! absent from the datapack, so Steel carries the same table in [`ignite_and_burn_odds`] keyed by
//! registry block id. It is transcribed from that method and should move to `steel-registry`
//! codegen so the table stops being hand-maintained.

use std::sync::Arc;

use steel_registry::RegistryEntry;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_game_rules::FIRE_SPREAD_RADIUS_AROUND_PLAYER;
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Identifier};

use crate::behavior::blocks::FireBlock;
use crate::entity::Entity as _;
use crate::world::{LevelReader, World};

/// Vanilla `FireBlock.MAX_AGE`.
const MAX_AGE: i32 = 15;

/// Vanilla `FireBlock.getFireTickDelay`: `30 + random(10)`.
#[must_use]
pub fn fire_tick_delay(random_offset: i32) -> i32 {
    30 + random_offset.rem_euclid(10)
}

/// Vanilla flammability odds `(ignite_odds, burn_odds)` for a registry block id.
///
/// Mirrors `FireBlock.bootStrap` (`/tmp/vanilla/src/world/level/block/FireBlock.java:302-499`),
/// including the `Blocks.WOOL`, `Blocks.WOOL_STAIRS`, `Blocks.WOOL_SLAB` and `Blocks.CARPET`
/// `forEach` families expanded by registry block id. `(0, 0)` means the block never ignites.
#[must_use]
pub const fn ignite_and_burn_odds(id: usize) -> (u8, u8) {
    match id {
        20 | 51..=60 | 63..=93 | 609 => (5, 5),
        13..=19
        | 21..=25
        | 61
        | 244
        | 333
        | 419
        | 455..=457
        | 569..=576
        | 653..=664
        | 682..=701
        | 999
        | 1002 => (5, 20),
        1278 | 1280 => (5, 100),
        1000 => (15, 20),
        1225..=1226 => (15, 60),
        220 | 416..=417 => (15, 100),
        221 | 223..=226 | 228..=234 | 935 | 1001 => (30, 20),
        94..=107 | 135 | 151..=198 | 834 | 1228..=1229 | 1238 => (30, 60),
        591..=607 | 1230 | 1279 => (60, 20),
        882 | 927 => (60, 60),
        140..=146
        | 200..=214
        | 329
        | 611..=616
        | 722
        | 951
        | 1227
        | 1231..=1233
        | 1235..=1237
        | 1281..=1282
        | 1285 => (60, 100),
        _ => (0, 0),
    }
}

/// Vanilla `FireBlock.getIgniteOdds(BlockState)`.
#[must_use]
fn ignite_odds_of(state: BlockStateId) -> u8 {
    if state.try_get_value(&BlockStateProperties::WATERLOGGED) == Some(true) {
        return 0;
    }
    ignite_and_burn_odds(state.get_block().id()).0
}

/// Vanilla `FireBlock.getBurnOdds(BlockState)`.
#[must_use]
fn burn_odds_of(state: BlockStateId) -> u8 {
    if state.try_get_value(&BlockStateProperties::WATERLOGGED) == Some(true) {
        return 0;
    }
    ignite_and_burn_odds(state.get_block().id()).1
}

impl World {
    /// Vanilla `ServerLevel.canSpreadFireAround`: fire simulation stops when no player is within
    /// `fire_spread_radius_around_player` blocks of the position (`-1` disables the limit).
    #[must_use]
    pub fn can_spread_fire_around(&self, pos: BlockPos) -> bool {
        let radius = self.get_game_rule(&FIRE_SPREAD_RADIUS_AROUND_PLAYER);
        if radius < 0 {
            return true;
        }

        let target = glam::DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
        let radius = f64::from(radius);
        let mut found = false;
        self.players.iter_players(|_, player| {
            if !player.is_spectator() && player.position().distance(target) < radius {
                found = true;
            }
            !found
        });
        found
    }

    /// Vanilla `FireBlock.isNearRain`: the position or one of its four horizontal neighbors is
    /// exposed to rain.
    #[must_use]
    pub fn fire_is_near_rain(&self, pos: BlockPos) -> bool {
        self.is_raining_at(pos)
            || self.is_raining_at(pos.west())
            || self.is_raining_at(pos.east())
            || self.is_raining_at(pos.north())
            || self.is_raining_at(pos.south())
    }

    /// Vanilla `FireBlock.tick`: ages, extinguishes, burns out and spreads fire.
    ///
    /// Scheduled from the fire block behavior with [`fire_tick_delay`].
    pub fn tick_fire(self: &Arc<Self>, state: BlockStateId, pos: BlockPos) {
        self.schedule_fire_tick(pos);
        if !self.can_spread_fire_around(pos) {
            return;
        }

        if !Self::fire_can_survive_at(self.as_ref(), pos) {
            self.remove_block(pos, false);
        }

        let below_state = self.get_block_state(pos.below());
        let infiniburn = self.is_infiniburn(below_state);
        let age = i32::from(state.get_value(&BlockStateProperties::AGE_15));
        if !infiniburn
            && self.is_raining()
            && self.fire_is_near_rain(pos)
            && rand::random::<f32>() < 0.2 + age as f32 * 0.03
        {
            self.remove_block(pos, false);
            return;
        }

        let new_age = (age + rand::random_range(0..3) / 2).min(MAX_AGE);
        let mut state = state;
        if age != new_age {
            state = state.set_value(&BlockStateProperties::AGE_15, new_age as u8);
            self.set_block(pos, state, UpdateFlags::UPDATE_ALL);
        }

        if !infiniburn {
            if !Self::is_valid_fire_location(self.as_ref(), pos) {
                let below = pos.below();
                if !self.is_face_sturdy(self.get_block_state(below), below, Direction::Up)
                    || age > 3
                {
                    self.remove_block(pos, false);
                }
                return;
            }

            if age == MAX_AGE
                && rand::random_range(0..4) == 0
                && ignite_odds_of(self.get_block_state(pos.below())) == 0
            {
                self.remove_block(pos, false);
                return;
            }
        }

        // Vanilla applies `EnvironmentAttributes.INCREASED_FIRE_BURNOUT` (-50) in humid biomes.
        let extra = if self.increased_fire_burnout(pos) {
            -50
        } else {
            0
        };
        self.check_burn_out(pos.east(), 300 + extra, age);
        self.check_burn_out(pos.west(), 300 + extra, age);
        self.check_burn_out(pos.below(), 250 + extra, age);
        self.check_burn_out(pos.above(), 250 + extra, age);
        self.check_burn_out(pos.north(), 300 + extra, age);
        self.check_burn_out(pos.south(), 300 + extra, age);

        let difficulty = i32::from(self.difficulty() as u8);
        for xx in -1..=1 {
            for zz in -1..=1 {
                for yy in -1..=4 {
                    if xx == 0 && yy == 0 && zz == 0 {
                        continue;
                    }

                    let mut rate = 100;
                    if yy > 1 {
                        rate += (yy - 1) * 100;
                    }

                    let test_pos = pos.offset(xx, yy, zz);
                    let ignite_odds = Self::ignite_odds_at(self.as_ref(), test_pos);
                    if ignite_odds == 0 {
                        continue;
                    }

                    let mut odds = (i32::from(ignite_odds) + 40 + difficulty * 7) / (age + 30);
                    if extra != 0 {
                        odds /= 2;
                    }
                    if odds > 0
                        && rand::random_range(0..rate) <= odds
                        && (!self.is_raining() || !self.is_raining_at(test_pos))
                    {
                        let spread_age = (age + rand::random_range(0..5) / 4).min(MAX_AGE);
                        self.set_block(
                            test_pos,
                            Self::fire_state_with_age(self.as_ref(), test_pos, spread_age),
                            UpdateFlags::UPDATE_ALL,
                        );
                    }
                }
            }
        }
    }

    /// Vanilla `FireBlock.onPlace`/`FireBlock.tick` scheduling: `scheduleTick(pos, this, 30 + random(10))`.
    pub fn schedule_fire_tick(self: &Arc<Self>, pos: BlockPos) {
        self.schedule_block_tick_default(
            pos,
            &vanilla_blocks::FIRE,
            fire_tick_delay(rand::random()),
        );
    }

    /// Vanilla `LavaFluid.randomTick`: spreads fire to flammable neighbors.
    ///
    /// Exposed for the lava fluid behavior, which owns `FluidBehavior::random_tick`.
    pub fn lava_random_tick_fire_spread(self: &Arc<Self>, pos: BlockPos) {
        if !self.can_spread_fire_around(pos) {
            return;
        }

        let passes = rand::random_range(0..3);
        if passes > 0 {
            let mut test_pos = pos;
            for _ in 0..passes {
                test_pos = test_pos.offset(rand::random_range(-1..2), 1, rand::random_range(-1..2));
                if !self.is_chunk_loaded(test_pos) {
                    return;
                }

                let state = self.get_block_state(test_pos);
                if state.is_air() {
                    if self.has_flammable_neighbors(test_pos) {
                        self.set_block(
                            test_pos,
                            FireBlock::get_state(self.as_ref(), test_pos),
                            UpdateFlags::UPDATE_ALL,
                        );
                        return;
                    }
                } else if state
                    .get_block()
                    .has_tag(&BlockTag::BLOCKS_LAVA_FIRE_SPREAD)
                {
                    return;
                }
            }
        } else {
            for _ in 0..3 {
                let test_pos = pos.offset(rand::random_range(-1..2), 0, rand::random_range(-1..2));
                if !self.is_chunk_loaded(test_pos) {
                    return;
                }

                if self.get_block_state(test_pos.above()).is_air() && self.is_flammable(test_pos) {
                    self.set_block(
                        test_pos.above(),
                        FireBlock::get_state(self.as_ref(), test_pos),
                        UpdateFlags::UPDATE_ALL,
                    );
                }
            }
        }
    }

    /// Vanilla `FireBlock.getStateForPlacement(BlockGetter, BlockPos)`.
    ///
    /// Regular fire connects to burnable neighbors only when it cannot rest on a sturdy face.
    #[must_use]
    pub(crate) fn fire_placement_state<L: LevelReader + ?Sized>(
        world: &L,
        pos: BlockPos,
    ) -> BlockStateId {
        let below = pos.below();
        let below_state = world.get_block_state(below);
        if ignite_odds_of(below_state) > 0
            || world.is_face_sturdy(below_state, below, Direction::Up)
        {
            return vanilla_blocks::FIRE.default_state();
        }

        let mut state = vanilla_blocks::FIRE.default_state();
        for direction in Direction::ALL {
            let property = match direction {
                Direction::North => &BlockStateProperties::NORTH,
                Direction::East => &BlockStateProperties::EAST,
                Direction::South => &BlockStateProperties::SOUTH,
                Direction::West => &BlockStateProperties::WEST,
                Direction::Up => &BlockStateProperties::UP,
                Direction::Down => continue,
            };
            let can_burn = ignite_odds_of(world.get_block_state(pos.relative(direction))) > 0;
            state = state.set_value(property, can_burn);
        }
        state
    }

    /// Vanilla `FireBlock.canSurvive`: the block below has a sturdy top face, or an adjacent block
    /// can burn.
    #[must_use]
    pub(crate) fn fire_can_survive_at<L: LevelReader + ?Sized>(world: &L, pos: BlockPos) -> bool {
        let below = pos.below();
        world.is_face_sturdy(world.get_block_state(below), below, Direction::Up)
            || Self::is_valid_fire_location(world, pos)
    }

    /// Vanilla `FireBlock.isValidFireLocation`: at least one neighbour can burn.
    #[must_use]
    fn is_valid_fire_location<L: LevelReader + ?Sized>(world: &L, pos: BlockPos) -> bool {
        Direction::ALL
            .iter()
            .any(|direction| ignite_odds_of(world.get_block_state(pos.relative(*direction))) > 0)
    }

    /// Vanilla `FireBlock.getIgniteOdds(LevelReader, BlockPos)`: the highest ignite odds among the
    /// neighbors of an empty position.
    #[must_use]
    fn ignite_odds_at<L: LevelReader + ?Sized>(world: &L, pos: BlockPos) -> u8 {
        if !world.get_block_state(pos).is_air() {
            return 0;
        }

        Direction::ALL
            .iter()
            .map(|direction| ignite_odds_of(world.get_block_state(pos.relative(*direction))))
            .max()
            .unwrap_or(0)
    }

    /// Vanilla `LavaFluid.isFlammable`: the block burns when ignited by lava.
    fn is_flammable(&self, pos: BlockPos) -> bool {
        self.get_block_state(pos).get_block().config.ignited_by_lava
    }

    /// Vanilla `LavaFluid.hasFlammableNeighbours`.
    fn has_flammable_neighbors(&self, pos: BlockPos) -> bool {
        Direction::ALL
            .iter()
            .any(|direction| self.is_flammable(pos.relative(*direction)))
    }

    /// Vanilla `FireBlock.checkBurnOut`.
    fn check_burn_out(self: &Arc<Self>, pos: BlockPos, chance: i32, age: i32) {
        let odds = burn_odds_of(self.get_block_state(pos));
        if odds == 0 || rand::random_range(0..chance) >= i32::from(odds) {
            return;
        }

        let old_state = self.get_block_state(pos);
        if rand::random_range(0..age + 10) < 5 && !self.is_raining_at(pos) {
            let new_age = (age + rand::random_range(0..5) / 4).min(MAX_AGE);
            self.set_block(
                pos,
                Self::fire_state_with_age(self.as_ref(), pos, new_age),
                UpdateFlags::UPDATE_ALL,
            );
        } else {
            self.remove_block(pos, false);
        }

        if old_state.get_block() == &vanilla_blocks::TNT {
            // Vanilla primes TNT caught in a fire burnout; Steel's TntBlock.prime returns false
            // until the PrimedTnt entity lane registers a factory.
            crate::behavior::blocks::TntBlock::prime(self, pos, None, &ItemStack::empty());
        }
    }

    /// Vanilla `FireBlock.getStateWithAge`.
    pub(crate) fn fire_state_with_age<L: LevelReader + ?Sized>(
        world: &L,
        pos: BlockPos,
        age: i32,
    ) -> BlockStateId {
        let state = FireBlock::get_state(world, pos);
        if state.get_block() == &vanilla_blocks::FIRE {
            state.set_value(&BlockStateProperties::AGE_15, age as u8)
        } else {
            state
        }
    }

    /// Vanilla `DimensionType.infiniburn()` membership for a block state.
    fn is_infiniburn(&self, state: BlockStateId) -> bool {
        let Some(path) = self
            .dimension_type
            .infiniburn
            .strip_prefix("#minecraft:")
            .or_else(|| self.dimension_type.infiniburn.strip_prefix('#'))
        else {
            return false;
        };
        state
            .get_block()
            .has_tag(&Identifier::new("minecraft", path.to_owned()))
    }

    /// Vanilla `Level.hasChunkAt` for the chunk containing `pos`.
    fn is_chunk_loaded(&self, pos: BlockPos) -> bool {
        self.chunk_map
            .with_full_chunk(ChunkPos::new(pos.x() >> 4, pos.z() >> 4), |_| ())
            .is_some()
    }

    /// Vanilla `EnvironmentAttributes.INCREASED_FIRE_BURNOUT`.
    ///
    /// Deliberate divergence: `steel_registry::Biome` does not expose the biome attribute map yet
    /// (the datapack carries `minecraft:gameplay/increased_fire_burnout`), so Steel keeps the
    /// attribute's default until the registry surfaces it.
    fn increased_fire_burnout(&self, _pos: BlockPos) -> bool {
        false
    }
}
