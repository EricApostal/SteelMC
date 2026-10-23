//! `CreakingHeartBlockEntity` (`net.minecraft.world.level.block.entity.CreakingHeartBlockEntity`).
//!
//! Owns the linked `Creaking` (resolved through the world, or a UUID until the entity loads), the
//! 20-24 tick re-evaluation cadence, the heart's `awake`/`dormant` block-state sync, the analog
//! output signal, and the hurt emitter that drives `CREAKING_HEART_HURT` sounds and resin spread
//! towards the creaking.
//!
//! Vanilla reads `EnvironmentAttributes.CREAKING_ACTIVE` here (to recompute `STATE` and to decide
//! whether the protector must despawn). Steel's timeline sampler lives in
//! `world::environment` and is private, so this entity reads the block state instead — the block's
//! own `updateState` must supply the attribute; see the reported dependency.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, CreakingHeartState, EnumProperty};
use steel_registry::particle_type::{ParticleData, TrailParticleOption};
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::{
    sound_events, vanilla_block_entity_types, vanilla_blocks, vanilla_entities,
    vanilla_game_events, vanilla_particle_types,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::{Difficulty, TraversalNodeStatus, UpdateFlags};
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, RgbColor, UuidExt as _,
    axis::Axis,
};

use crate::behavior::blocks::{CreakingHeartBlock, multiface_face_property};
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::entities::mobs::hostile::creaking::CreakingEntity;
use crate::entity::{Entity as _, EntitySpawnReason, SharedEntity};
use crate::world::game_event::GameEventContext;
use crate::world::spawn::SpawnStrategy;
use crate::world::spawn::natural::runtime_random;
use crate::world::World;

/// Vanilla `CreakingHeartBlockEntity.PLAYER_DETECTION_RANGE`.
const PLAYER_DETECTION_RANGE: f64 = 32.0;
/// Vanilla `CreakingHeartBlockEntity.CREAKING_ROAMING_RADIUS`.
pub const CREAKING_ROAMING_RADIUS: f64 = 32.0;
/// Vanilla `CreakingHeartBlockEntity.DISTANCE_CREAKING_TOO_FAR`.
const DISTANCE_CREAKING_TOO_FAR: f64 = 34.0;
/// Vanilla `CreakingHeartBlockEntity.SPAWN_RANGE_XZ`.
const SPAWN_RANGE_XZ: i32 = 16;
/// Vanilla `CreakingHeartBlockEntity.SPAWN_RANGE_Y`.
const SPAWN_RANGE_Y: i32 = 8;
/// Vanilla `CreakingHeartBlockEntity.ATTEMPTS_PER_SPAWN`.
const ATTEMPTS_PER_SPAWN: i32 = 5;
/// Vanilla `CreakingHeartBlockEntity.UPDATE_TICKS`.
const UPDATE_TICKS: i32 = 20;
/// Vanilla `CreakingHeartBlockEntity.UPDATE_TICKS_VARIANCE`.
const UPDATE_TICKS_VARIANCE: i32 = 5;
/// Vanilla `CreakingHeartBlockEntity.HURT_CALL_TOTAL_TICKS`.
const HURT_CALL_TOTAL_TICKS: i32 = 100;
/// Vanilla `CreakingHeartBlockEntity.HURT_CALL_INTERVAL`.
const HURT_CALL_INTERVAL: i32 = 10;
/// Vanilla `CreakingHeartBlockEntity.HURT_CALL_PARTICLE_TICKS`.
const HURT_CALL_PARTICLE_TICKS: i32 = 50;
/// Vanilla `CreakingHeartBlockEntity.MAX_DEPTH`.
const MAX_DEPTH: i32 = 2;
/// Vanilla `CreakingHeartBlockEntity.MAX_COUNT`.
const MAX_COUNT: i32 = 64;
/// Vanilla `CreakingHeartBlockEntity.TICKS_GRACE_PERIOD`.
const TICKS_GRACE_PERIOD: i64 = 30;

/// Vanilla `TrailParticleOption` color sent towards the creaking.
const PARTICLE_COLOR_TOWARDS_CREAKING: RgbColor = RgbColor::new(16_545_810);
/// Vanilla `TrailParticleOption` color sent towards the heart.
const PARTICLE_COLOR_TOWARDS_HEART: RgbColor = RgbColor::new(6_250_335);

const STATE: &EnumProperty<CreakingHeartState> = &BlockStateProperties::CREAKING_HEART_STATE;
const WATERLOGGED: &steel_registry::blocks::properties::BoolProperty =
    &BlockStateProperties::WATERLOGGED;

/// `CreakingHeartBlockEntity`.
pub struct CreakingHeartBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<CreakingHeartData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `CreakingHeartBlockEntity`.
unsafe impl DowncastType for CreakingHeartBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/creaking_heart");
}

/// Vanilla `CreakingHeartBlockEntity` persistent and transient state.
#[derive(Default)]
struct CreakingHeartData {
    /// Linked creaking UUID (`Either<Creaking, UUID>` persisted side).
    creaking: Option<uuid::Uuid>,
    /// Ticks since the link was established.
    ticks_existed: i64,
    /// Randomized 20-24 tick burst counter.
    ticker: i32,
    /// Remaining hurt-emitter ticks.
    emitter: i32,
    /// Position the hurt sounds travel towards.
    emitter_target: Option<DVec3>,
    /// Cached analog output signal.
    output_signal: i32,
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
            state: SyncMutex::new(CreakingHeartData::default()),
        }
    }

    /// Vanilla `CreakingHeartBlockEntity.setCreakingInfo(UUID)`: records the link and resets the
    /// timer.
    pub fn set_creaking_info(&self, uuid: uuid::Uuid) {
        {
            let mut state = self.state.lock();
            state.creaking = Some(uuid);
            state.ticks_existed = 0;
        }
        self.set_changed();
    }

    /// Vanilla `CreakingHeartBlockEntity.clearCreakingInfo`.
    pub fn clear_creaking_info(&self) {
        self.state.lock().creaking = None;
        self.set_changed();
    }

    /// Returns the linked creaking UUID.
    #[must_use]
    pub fn creaking(&self) -> Option<uuid::Uuid> {
        self.state.lock().creaking
    }

    /// Vanilla `CreakingHeartBlockEntity.getAnalogOutputSignal`.
    #[must_use]
    pub fn analog_output_signal(&self) -> i32 {
        self.state.lock().output_signal
    }

    /// Vanilla `CreakingHeartBlockEntity.isProtector`.
    #[must_use]
    pub fn is_protector(&self, creaking: &CreakingEntity) -> bool {
        self.creaking() == Some(creaking.uuid())
    }

    /// Vanilla `CreakingHeartBlockEntity.getCreakingProtector`: resolves the link and clears it once
    /// the grace period expires.
    fn get_creaking_protector(&self, world: &Arc<World>) -> Option<SharedEntity> {
        let uuid = self.creaking()?;
        let resolved = world
            .get_entity_by_uuid(&uuid)
            .filter(|entity| entity.as_ref().downcast_ref::<CreakingEntity>().is_some());
        let Some(entity) = resolved else {
            // Vanilla's `else` branch clears the link once the grace period expires, both when the
            // entity is gone and when the UUID resolves to another entity type.
            self.clear_creaking_info_if_expired();
            return None;
        };
        if entity.is_removed() {
            // Vanilla downgrades a removed `Creaking` to its UUID form (a one-shot
            // `setCreakingInfo(UUID)` that resets `ticksExisted`) and then applies the grace check.
            // Steel stores the UUID form already, so only the grace check is left; resetting the
            // timer here every tick would keep the dead link alive forever.
            self.clear_creaking_info_if_expired();
            return None;
        }
        Some(entity)
    }

    /// Vanilla `getCreakingProtector`'s `ticksExisted >= TICKS_GRACE_PERIOD` fallback.
    fn clear_creaking_info_if_expired(&self) {
        let expired = self.state.lock().ticks_existed >= TICKS_GRACE_PERIOD;
        if expired {
            self.clear_creaking_info();
        }
    }

    /// Vanilla `CreakingHeartBlockEntity.distanceToCreaking`.
    fn distance_to_creaking(&self, world: &Arc<World>) -> f64 {
        let pos = self.get_block_pos();
        let bottom_center = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        self.get_creaking_protector(world)
            .map_or(0.0, |creaking| {
                creaking.position().distance(bottom_center)
            })
    }

    /// Vanilla `CreakingHeartBlockEntity.computeAnalogOutputSignal`.
    fn compute_analog_output_signal(&self, world: &Arc<World>) -> i32 {
        if self.creaking().is_none() || self.get_creaking_protector(world).is_none() {
            return 0;
        }
        let distance = self.distance_to_creaking(world).clamp(0.0, CREAKING_ROAMING_RADIUS);
        let scaled_distance = distance / CREAKING_ROAMING_RADIUS;
        15 - (scaled_distance * 15.0).floor() as i32
    }

    /// Vanilla `CreakingHeartBlockEntity.updateCreakingState`.
    fn update_creaking_state(
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        has_creaking: bool,
    ) -> BlockStateId {
        if !CreakingHeartBlock::has_required_logs(state, world.as_ref(), pos) && !has_creaking {
            return state.set_value(STATE, CreakingHeartState::Uprooted);
        }
        let heart_state = if Self::creaking_active(state) {
            CreakingHeartState::Awake
        } else {
            CreakingHeartState::Dormant
        };
        state.set_value(STATE, heart_state)
    }

    /// Vanilla `EnvironmentAttributes.CREAKING_ACTIVE` as reflected by the block state.
    ///
    /// The attribute is timeline-driven (`minecraft:gameplay/creaking_active`); Steel's sampler is
    /// private to `world::environment`, so the block's `updateState` is the only producer of the
    /// `awake` state here. Reported dependency: expose the attribute from `World`.
    fn creaking_active(state: BlockStateId) -> bool {
        state.get_value(STATE) == CreakingHeartState::Awake
    }

    /// Vanilla `CreakingHeartBlockEntity.spawnProtector`.
    fn spawn_protector(world: &Arc<World>, pos: BlockPos) -> Option<SharedEntity> {
        let mut random = runtime_random();
        let spawned = crate::world::spawn::try_spawn_mob(
            &vanilla_entities::CREAKING,
            EntitySpawnReason::Spawner,
            world,
            pos,
            ATTEMPTS_PER_SPAWN,
            SPAWN_RANGE_XZ,
            SPAWN_RANGE_Y,
            SpawnStrategy::OnTopOfColliderNoLeaves,
            true,
            &mut random,
        )?;
        world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            BlockPos::from(spawned.position()),
            &GameEventContext::new(Some(spawned.as_ref()), None),
        );
        // Vanilla `broadcastEntityEvent(creaking, (byte) 60)` (`EntityStatus.POOF`).
        spawned.broadcast_entity_event(EntityStatus::Poof);
        if let Some(creaking) = spawned.downcast_ref::<CreakingEntity>() {
            creaking.set_transient(pos);
        }
        Some(spawned)
    }

    /// Vanilla `CreakingHeartBlockEntity.creakingHurt`: start the hurt emitter and spread resin.
    pub fn creaking_hurt(&self, world: &Arc<World>) {
        let Some(creaking) = self.get_creaking_protector(world) else {
            return;
        };
        if self.state.lock().emitter > 0 {
            return;
        }
        self.emit_particles(world, 20, false);
        let pos = self.get_block_pos();
        if world.get_block_state(pos).get_value(STATE) == CreakingHeartState::Awake {
            let clumps = rand::random_range(2..=3);
            for _ in 0..clumps {
                if let Some(resin_pos) = self.spread_resin(world) {
                    world.play_block_sound(
                        &sound_events::BLOCK_RESIN_PLACE,
                        resin_pos,
                        1.0,
                        1.0,
                        None,
                    );
                    world.game_event(
                        &vanilla_game_events::BLOCK_PLACE,
                        resin_pos,
                        // Vanilla `GameEvent.Context.of(this.getBlockState())` — the heart's own
                        // state, not the freshly placed resin.
                        &GameEventContext::new(None, Some(self.get_block_state())),
                    );
                }
            }
        }
        let mut state = self.state.lock();
        state.emitter = HURT_CALL_TOTAL_TICKS;
        state.emitter_target = Some(creaking.bounding_box().center());
    }

    /// Vanilla `CreakingHeartBlockEntity.spreadResin`.
    fn spread_resin(&self, world: &Arc<World>) -> Option<BlockPos> {
        let mut placed: Option<BlockPos> = None;
        let _ = BlockPos::breadth_first_traversal(
            self.get_block_pos(),
            MAX_DEPTH,
            MAX_COUNT,
            |pos, acceptor| {
                for direction in shuffled_directions() {
                    let neighbour = pos.relative(direction);
                    if is_pale_oak_log(world, neighbour) {
                        acceptor(neighbour);
                    }
                }
            },
            |pos| {
                if !is_pale_oak_log(world, pos) {
                    return TraversalNodeStatus::Accept;
                }
                for direction in shuffled_directions() {
                    let neighbour_pos = pos.relative(direction);
                    let mut neighbour_state = world.get_block_state(neighbour_pos);
                    let opposite = direction.opposite();
                    if neighbour_state.is_air() {
                        neighbour_state = vanilla_blocks::RESIN_CLUMP.default_state();
                    } else if neighbour_state.get_block() == &vanilla_blocks::WATER
                        && neighbour_state.get_fluid_state().is_source()
                    {
                        neighbour_state =
                            vanilla_blocks::RESIN_CLUMP.default_state().set_value(WATERLOGGED, true);
                    }
                    let face = multiface_face_property(opposite);
                    if neighbour_state.get_block() == &vanilla_blocks::RESIN_CLUMP
                        && !neighbour_state.get_value(face)
                    {
                        world.set_block(
                            neighbour_pos,
                            neighbour_state.set_value(face, true),
                            UpdateFlags::UPDATE_ALL,
                        );
                        placed = Some(neighbour_pos);
                        return TraversalNodeStatus::Stop;
                    }
                }
                TraversalNodeStatus::Accept
            },
        );
        placed
    }

    /// Vanilla `CreakingHeartBlockEntity.emitParticles`.
    fn emit_particles(&self, world: &Arc<World>, count: i32, towards_creaking: bool) {
        let Some(creaking) = self.get_creaking_protector(world) else {
            return;
        };
        let color = if towards_creaking {
            PARTICLE_COLOR_TOWARDS_CREAKING
        } else {
            PARTICLE_COLOR_TOWARDS_HEART
        };
        let pos = self.get_block_pos();
        let box_ = creaking.bounding_box();
        let min = DVec3::new(box_.min(Axis::X), box_.min(Axis::Y), box_.min(Axis::Z));
        for _ in 0..count {
            let mut source = min
                + DVec3::new(
                    rand::random::<f64>() * box_.width(),
                    rand::random::<f64>() * box_.height(),
                    rand::random::<f64>() * box_.depth(),
                );
            let mut destination = DVec3::new(
                f64::from(pos.x()) + rand::random::<f64>(),
                f64::from(pos.y()) + rand::random::<f64>(),
                f64::from(pos.z()) + rand::random::<f64>(),
            );
            if towards_creaking {
                std::mem::swap(&mut source, &mut destination);
            }
            world.send_particles_with_options(
                ParticleData::new(
                    &vanilla_particle_types::TRAIL,
                    TrailParticleOption::new(destination, color, rand::random_range(10..50)),
                ),
                true,
                true,
                source,
                1,
                DVec3::ZERO,
                0.0,
            );
        }
    }

    /// Vanilla `CreakingHeartBlockEntity.removeProtector`.
    ///
    /// The damage-source half additionally calls `Creaking.creakingDeathEffects` and
    /// `Creaking.tearDown`; both are entity-lane hooks Steel does not expose yet.
    pub fn remove_protector(&self, world: &Arc<World>) {
        if self.get_creaking_protector(world).is_some() {
            self.clear_creaking_info();
        }
    }

    /// Vanilla `CreakingHeartBlockEntity.serverTick` emitter half.
    fn tick_emitter(&self, world: &Arc<World>, pos: BlockPos) {
        let (emitter, target) = {
            let state = self.state.lock();
            (state.emitter, state.emitter_target)
        };
        if emitter <= 0 {
            return;
        }
        if emitter > HURT_CALL_PARTICLE_TICKS {
            self.emit_particles(world, 1, true);
            self.emit_particles(world, 1, false);
        }

        if emitter % HURT_CALL_INTERVAL == 0
            && let Some(mut target) = target
        {
            if let Some(creaking) = self.get_creaking_protector(world) {
                target = creaking.bounding_box().center();
                self.state.lock().emitter_target = Some(target);
            }
            let heart_position = DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            );
            let progress = 0.2 + 0.8 * f64::from(HURT_CALL_TOTAL_TICKS - emitter) / 100.0;
            let sound_location = heart_position.subtract(target).scale(progress) + target;
            let volume = emitter as f32 / 2.0 / 100.0 + 0.5;
            world.play_block_sound(
                &sound_events::BLOCK_CREAKING_HEART_HURT,
                BlockPos::from(sound_location),
                volume,
                1.0,
                None,
            );
        }
        self.state.lock().emitter -= 1;
    }
}

/// Vanilla `Util.shuffledCopy(Direction.values(), random)` over the six directions.
fn shuffled_directions() -> [steel_utils::Direction; 6] {
    let mut directions = steel_utils::Direction::ALL;
    for index in (1..directions.len()).rev() {
        let swap_with = rand::random_range(0..=index);
        directions.swap(index, swap_with);
    }
    directions
}

/// Vanilla `BlockState.is(BlockTags.PALE_OAK_LOGS)`.
fn is_pale_oak_log(world: &World, pos: BlockPos) -> bool {
    world
        .get_block_state(pos)
        .get_block()
        .has_tag(&BlockTag::PALE_OAK_LOGS)
}

impl BlockEntity for CreakingHeartBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let view: NbtCompoundView<'_, '_> = nbt.into();
        // Vanilla `input.read("creaking", UUIDUtil.CODEC)`: a four-int array.
        match view
            .int_array("creaking")
            .and_then(|uuid| uuid::Uuid::from_int_array(&uuid))
        {
            Some(uuid) => self.set_creaking_info(uuid),
            None => self.clear_creaking_info(),
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        if let Some(creaking) = self.creaking() {
            nbt.insert("creaking", NbtTag::IntArray(creaking.to_int_array().to_vec()));
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        // Vanilla `CreakingHeartBlockEntity.getUpdateTag` returns `saveCustomOnly`.
        Some(self.save_custom_only())
    }

    fn pre_remove_side_effects(&self, _pos: BlockPos, _state: BlockStateId) {
        // Vanilla `CreakingHeartBlockEntity.preRemoveSideEffects` → `removeProtector(null)`.
        if let Some(world) = self.get_level() {
            self.remove_protector(&world);
        }
    }

    fn tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        self.state.lock().ticks_existed += 1;

        let computed_output_signal = self.compute_analog_output_signal(world);
        let output_changed = {
            let mut state = self.state.lock();
            let changed = state.output_signal != computed_output_signal;
            state.output_signal = computed_output_signal;
            changed
        };
        if output_changed {
            world.update_neighbor_for_output_signal(pos, &vanilla_blocks::CREAKING_HEART);
        }

        self.tick_emitter(world, pos);

        {
            let mut state = self.state.lock();
            state.ticker -= 1;
            if state.ticker >= 0 {
                return;
            }
            // Vanilla `entity.level == null ? 20 : random.nextInt(5) + 20`.
            state.ticker = rand::random_range(UPDATE_TICKS..UPDATE_TICKS + UPDATE_TICKS_VARIANCE);
        }

        let state_before = world.get_block_state(pos);
        let updated_state =
            Self::update_creaking_state(state_before, world, pos, self.creaking().is_some());
        if updated_state != state_before {
            world.set_block(pos, updated_state, UpdateFlags::UPDATE_ALL);
            if updated_state.get_value(STATE) == CreakingHeartState::Uprooted {
                return;
            }
        }

        if self.creaking().is_none() {
            if updated_state.get_value(STATE) != CreakingHeartState::Awake
                || !world.is_spawning_monsters()
                || world.difficulty() == Difficulty::Peaceful
            {
                return;
            }
            let has_player = world
                .nearest_player(
                    DVec3::new(
                        f64::from(pos.x()) + 0.5,
                        f64::from(pos.y()) + 0.5,
                        f64::from(pos.z()) + 0.5,
                    ),
                    PLAYER_DETECTION_RANGE,
                    // Vanilla `getNearestPlayer(..., false)` ⇒ `EntitySelector.NO_SPECTATORS`.
                    |player| !player.is_spectator(),
                )
                .is_some();
            if !has_player {
                return;
            }
            let Some(creaking) = Self::spawn_protector(world, pos) else {
                return;
            };
            self.set_creaking_info(creaking.uuid());
            // Vanilla `creaking.makeSound(SoundEvents.CREAKING_SPAWN)`:
            // `playSound(sound, getSoundVolume(), getRandom().nextFloat() * 0.2F + 0.9F)`.
            creaking.play_sound(
                &sound_events::ENTITY_CREAKING_SPAWN,
                1.0,
                rand::random::<f32>() * 0.2 + 0.9,
            );
            world.play_block_sound(
                &sound_events::BLOCK_CREAKING_HEART_SPAWN,
                pos,
                1.0,
                1.0,
                None,
            );
        } else if let Some(creaking) = self.get_creaking_protector(world) {
            let persistence_required = creaking
                .as_mob()
                .is_some_and(crate::entity::Mob::is_persistence_required);
            // TODO(Entities): Vanilla's `creaking.playerIsStuckInYou()` clause is the third despawn
            // trigger; `Creaking` has no equivalent hook yet.
            if (!Self::creaking_active(updated_state) && !persistence_required)
                || self.distance_to_creaking(world) > DISTANCE_CREAKING_TOO_FAR
            {
                self.remove_protector(world);
            }
        }
    }
}
