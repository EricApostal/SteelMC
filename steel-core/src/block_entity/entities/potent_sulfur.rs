//! `PotentSulfurBlockEntity` for geyser eruption

use std::sync::{Arc, Weak};

use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, PotentSulfurState};
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_game_events;
use steel_registry::vanilla_mob_effects;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, vanilla_blocks, vanilla_entity_type_tags::EntityTypeTag,
};
use steel_utils::random::xoroshiro::Xoroshiro;
use steel_utils::random::{PositionalRandom, Random, RandomSource, RandomSplitter};
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, WorldAabb, locks::SyncMutex,
};

use crate::behavior::{BLOCK_BEHAVIORS, BlockCollisionContext};
use crate::block_entity::{BlockEntity, BlockEntityBase, BlockEntityLifecycleExt as _};
use crate::entity::{MobEffectInstance, SharedEntity};
use crate::fluid::FluidStateExt as _;
use crate::world::{ClipBlockShape, ClipFluid, World};

const GEYSER_SALT: i64 = -904_011_478;
const COUNTDOWN_FREQUENCY_TICKS: i64 = 20;
const LAUNCH_FORCE: f64 = 0.2;
const BASE_VELOCITY_THRESHOLD: f64 = 0.3;
const VELOCITY_THRESHOLD_SCALE: f64 = 0.1;
const MAX_WATER_BLOCKS_ABOVE: i32 = 4;
const FORCE_HEIGHT_MULTIPLIER: i32 = 6;
/// Vanilla `PotentSulfurBlockEntity.EFFECT_APPLICATION_FREQUENCY_TICKS`.
const EFFECT_APPLICATION_FREQUENCY_TICKS: i64 = 10;
/// Vanilla `PotentSulfurBlockEntity.EFFECT_DURATION_IN_TICKS`.
const EFFECT_DURATION_IN_TICKS: i32 = 80;
/// Vanilla `PotentSulfurBlockEntity.EFFECT_RANGE` (`AABB.inflate(2.5, 0.0, 2.5)`).
const EFFECT_RANGE: f64 = 2.5;

/// Block entity for `potent_sulfur` blocks
pub struct PotentSulfurBlockEntity {
    base: BlockEntityBase,
    sulfur: SyncMutex<PotentSulfurData>,
}

struct PotentSulfurData {
    /// Countdown for 20 tick steps before the state toggles. -1 is uninitialized
    waiting_countdown: i32,
    /// Game tick at which the current eruption started
    eruption_tick: i64,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PotentSulfurBlockEntity`.
unsafe impl DowncastType for PotentSulfurBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/potent_sulfur");
}

impl PotentSulfurBlockEntity {
    /// Creates a new block entity
    #[must_use]
    pub fn new(world: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let eruption_tick = world.upgrade().map_or(-1, |w| w.game_time());
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::POTENT_SULFUR,
                world,
                pos,
                state,
            ),
            sulfur: SyncMutex::new(PotentSulfurData {
                waiting_countdown: -1,
                eruption_tick,
            }),
        }
    }

    /// Resets the countdown so it reinitializes on the next tick
    pub fn reset_countdown(&self) {
        self.sulfur.lock().waiting_countdown = -1;
    }

    /// Records the tick at which an eruption started.
    pub fn set_eruption_tick(&self, eruption_tick: i64) {
        self.sulfur.lock().eruption_tick = eruption_tick;
    }

    fn geyser_positional_rng(seed: i64, pos: BlockPos) -> Xoroshiro {
        let mut base = Xoroshiro::from_seed((seed ^ GEYSER_SALT) as u64);
        let RandomSplitter::Xoroshiro(splitter) = base.next_positional() else {
            unreachable!("Xoroshiro always produces Xoroshiro splitter")
        };
        match splitter.at(pos.x(), pos.y(), pos.z()) {
            RandomSource::Xoroshiro(r) => r,
            RandomSource::Legacy(_) => {
                unreachable!("XoroshiroSplitter::at always returns Xoroshiro")
            }
        }
    }

    fn is_geyser_passable(world: &World, pos: BlockPos, context: BlockCollisionContext) -> bool {
        let state = world.get_block_state(pos);
        if state.is_air() || state.get_block() == &vanilla_blocks::WATER {
            return true;
        }

        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        behavior
            .get_collision_shape(state, world, pos, context)
            .is_empty()
    }

    fn find_source_block(world: &World, origin: BlockPos) -> Option<BlockPos> {
        let max_y = origin.y() + MAX_WATER_BLOCKS_ABOVE + 1;
        let geyser_position_context =
            BlockCollisionContext::position_context(f64::from(origin.y()));
        let mut pos = BlockPos::new(origin.x(), origin.y() + 1, origin.z());

        while pos.y() <= max_y {
            let state = world.get_block_state(pos);
            let fluid = state.get_fluid_state();
            let is_water_source = fluid.is_source() && fluid.is_water();

            if is_water_source
                && (state.get_block() == &vanilla_blocks::WATER
                    || Self::is_geyser_passable(world, pos, geyser_position_context))
            {
                pos = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
                continue;
            }

            if state.is_air() || Self::is_geyser_passable(world, pos, geyser_position_context) {
                return Some(pos);
            }

            break; // Solid obstruction
        }

        None
    }

    fn unobstructed_block_count(world: &World, start: BlockPos, water_blocks: i32) -> i32 {
        let max_height = FORCE_HEIGHT_MULTIPLIER * water_blocks;
        let geyser_position_context =
            BlockCollisionContext::position_context(f64::from(start.y() - 1));
        for i in 0..max_height {
            let check = BlockPos::new(start.x(), start.y() + i, start.z());
            if !Self::is_geyser_passable(world, check, geyser_position_context) {
                return i;
            }
        }
        max_height
    }

    fn tick_countdown(
        &self,
        world: &World,
        pos: BlockPos,
        state: BlockStateId,
    ) -> Option<(BlockStateId, bool)> {
        let source = Self::find_source_block(world, pos)?;
        let water_blocks = source.y() - pos.y() - 1;
        let mut rng = Self::geyser_positional_rng(world.seed(), pos);
        let game_time = world.game_time();
        let mut sulfur = self.sulfur.lock();

        if sulfur.waiting_countdown <= 0 {
            let current_state = state.get_value(&BlockStateProperties::POTENT_SULFUR_STATE);

            sulfur.waiting_countdown = if current_state == PotentSulfurState::Dormant {
                10 * (water_blocks - 1) + rng.next_i32_between(15, 30)
            } else {
                rng.next_i32();
                (water_blocks - 1) + rng.next_i32_between(1, 2)
            };
        }

        if sulfur.waiting_countdown > 0 {
            sulfur.waiting_countdown -= 1;
        }

        if sulfur.waiting_countdown == 0 {
            let current_state = state.get_value(&BlockStateProperties::POTENT_SULFUR_STATE);
            let next_state = if current_state == PotentSulfurState::Dormant {
                PotentSulfurState::Erupting
            } else {
                PotentSulfurState::Dormant
            };
            let deactivates = next_state == PotentSulfurState::Dormant;
            let activates = next_state == PotentSulfurState::Erupting;
            let new_state = state.set_value(&BlockStateProperties::POTENT_SULFUR_STATE, next_state);
            if activates {
                sulfur.eruption_tick = game_time;
            }
            return Some((new_state, deactivates));
        }

        None
    }

    fn tick_launch(world: &Arc<World>, pos: BlockPos) {
        let Some(source) = Self::find_source_block(world, pos) else {
            return;
        };

        let water_blocks = source.y() - pos.y() - 1;
        let above = BlockPos::new(pos.x(), pos.y() + 1, pos.z());
        let force_height = Self::unobstructed_block_count(world, above, water_blocks);

        let aabb = WorldAabb::new(
            f64::from(pos.x()),
            f64::from(pos.y() + 1),
            f64::from(pos.z()),
            f64::from(pos.x() + 1),
            f64::from(pos.y() + 1) + f64::from(force_height),
            f64::from(pos.z() + 1),
        );

        let velocity_threshold =
            BASE_VELOCITY_THRESHOLD + f64::from(water_blocks) * VELOCITY_THRESHOLD_SCALE;

        for entity in world.get_entities_in_aabb(&aabb) {
            if !entity.is_alive() || entity.is_spectator() {
                continue;
            }
            let vel = entity.velocity();
            entity.check_fall_distance_accumulation();

            if !entity.can_simulate_movement() {
                continue;
            }
            if entity.is_flying_player() {
                continue;
            }
            if entity.is_passenger() {
                continue;
            }
            if REGISTRY.entity_types.is_in_tag(
                entity.entity_type(),
                &EntityTypeTag::NOT_AFFECTED_BY_GEYSERS,
            ) {
                continue;
            }
            if vel.y >= velocity_threshold {
                continue;
            }

            entity.set_velocity(glam::DVec3::new(vel.x, vel.y + LAUNCH_FORCE, vel.z));
            entity.mark_velocity_sync();
        }
    }

    /// Vanilla `PotentSulfurBlockEntity.getNearbyLivingEntities`: living, non-spectator entities in
    /// the source block's `new AABB(pos).inflate(2.5, 0.0, 2.5)` box.
    fn nearby_living_entities(world: &World, source: BlockPos) -> Vec<SharedEntity> {
        let aabb = WorldAabb::new(
            f64::from(source.x()) - EFFECT_RANGE,
            f64::from(source.y()),
            f64::from(source.z()) - EFFECT_RANGE,
            f64::from(source.x() + 1) + EFFECT_RANGE,
            f64::from(source.y() + 1),
            f64::from(source.z() + 1) + EFFECT_RANGE,
        );
        world.get_entities_in_aabb_matching(&aabb, |entity| {
            !entity.is_spectator() && entity.is_alive() && entity.as_living_entity().is_some()
        })
    }

    /// Vanilla `PotentSulfurBlockEntity.isWater`.
    fn is_water(world: &World, pos: glam::DVec3) -> bool {
        let block_pos = BlockPos::containing(pos.x, pos.y, pos.z);
        let fluid = world.get_block_state(block_pos).get_fluid_state();
        fluid.is_source() && fluid.is_water()
    }

    /// Vanilla `PotentSulfurBlockEntity.haveLineOfSight`: the `COLLIDER`/`NONE` clip between the two
    /// points must not hit a block.
    fn have_line_of_sight(world: &World, from: glam::DVec3, to: glam::DVec3) -> bool {
        world
            .clip(from, to, ClipBlockShape::Collider, ClipFluid::None)
            .is_miss()
    }

    /// Vanilla `PotentSulfurBlockEntity.canBeReachedByNoxiousGas`.
    fn can_be_reached_by_noxious_gas(
        world: &World,
        source: BlockPos,
        eye_pos: glam::DVec3,
    ) -> bool {
        let block_pos = BlockPos::containing(eye_pos.x, eye_pos.y, eye_pos.z);
        let context = BlockCollisionContext::position_context(f64::from(block_pos.y() - 1));
        if !Self::is_geyser_passable(world, block_pos, context) {
            return false;
        }
        let source_center = glam::DVec3::new(
            f64::from(source.x()) + 0.5,
            f64::from(source.y()) + 0.5,
            f64::from(source.z()) + 0.5,
        );
        if eye_pos.distance_squared(source_center) > 9.0 {
            return false;
        }
        // Vanilla `Vec3.atCenterOf(sourceBlock.below())` and `pos.with(Axis.Y, pos.y - 1.0)`.
        let below_source = source_center - glam::DVec3::new(0.0, 1.0, 0.0);
        let below_pos = glam::DVec3::new(eye_pos.x, eye_pos.y - 1.0, eye_pos.z);
        Self::is_water(world, below_pos) && Self::have_line_of_sight(world, below_source, below_pos)
    }

    /// Vanilla `PotentSulfurBlockEntity.SERVER_NAUSEA_EFFECT_TICKER`: nausea for every reachable
    /// living entity around the noxious gas source.
    fn tick_nausea(world: &World, pos: BlockPos) {
        let Some(source) = Self::find_source_block(world, pos) else {
            return;
        };
        for entity in Self::nearby_living_entities(world, source) {
            let Some(living) = entity.as_living_entity() else {
                continue;
            };
            let position = entity.position();
            let eye_pos = glam::DVec3::new(position.x, entity.get_eye_y(), position.z);
            if !Self::can_be_reached_by_noxious_gas(world, source, eye_pos) {
                continue;
            }
            // Vanilla `applyNauseaEffect`: `new MobEffectInstance(NAUSEA, 80, 0, true, true)`.
            living.add_mob_effect(
                MobEffectInstance::with_duration(
                    vanilla_mob_effects::NAUSEA,
                    EFFECT_DURATION_IN_TICKS,
                    0,
                )
                .with_ambient(true)
                .with_visible(true),
            );
        }
    }
}

impl BlockEntity for PotentSulfurBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        if let Some(countdown) = nbt.int("countdown") {
            self.sulfur.lock().waiting_countdown = countdown;
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        nbt.insert("countdown", self.sulfur.lock().waiting_countdown);
    }

    fn tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let state = world.get_block_state(pos);
        if state.get_block() != &vanilla_blocks::POTENT_SULFUR {
            self.set_removed();
            return;
        }

        let current = state.get_value(&BlockStateProperties::POTENT_SULFUR_STATE);

        if current == PotentSulfurState::Dry {
            return;
        }

        let game_time = world.game_time();

        let action = if matches!(
            &current,
            PotentSulfurState::Dormant | PotentSulfurState::Erupting
        ) && game_time % COUNTDOWN_FREQUENCY_TICKS == 0
        {
            self.tick_countdown(world, pos, state)
        } else {
            None
        };

        if matches!(
            &current,
            PotentSulfurState::Erupting | PotentSulfurState::Continuous
        ) {
            Self::tick_launch(world, pos);
        }

        if let Some((new_state, deactivates)) = action {
            world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
            if deactivates {
                world.game_event(
                    &vanilla_game_events::BLOCK_DEACTIVATE,
                    pos,
                    &crate::world::game_event::GameEventContext::new(None, Some(state)),
                );
            }
        }

        // Vanilla ticker selection: `SERVER_NAUSEA_EFFECT_TICKER` runs for `WET` and `DORMANT`
        // only (`DORMANT` = `SERVER_WAITING_COUNTDOWN_TICKER.andThen(SERVER_NAUSEA_EFFECT_TICKER)`).
        if matches!(&current, PotentSulfurState::Wet | PotentSulfurState::Dormant)
            && game_time % EFFECT_APPLICATION_FREQUENCY_TICKS == 0
        {
            Self::tick_nausea(world, pos);
        }
    }
}
