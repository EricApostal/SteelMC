//! `ConduitBlockEntity` (`net.minecraft.world.level.block.entity.ConduitBlockEntity`).
//!
//! Vanilla re-scans the conduit frame every 40 ticks (`updateShape`), applies `Conduit Power` to
//! nearby players while active, and damages hostile mobs within 8 blocks once the frame reaches 42
//! blocks (`updateHunting`). The block behavior selects this entity's [`BlockEntity::tick`] as the
//! server ticker, mirroring `ConduitBlockEntity.serverTick`.
//!
//! Deviation: vanilla `ConduitBlockEntity.clientTick` (`tickCount`, `activeRotation`,
//! `animationTick`'s nautilus particles) is client-only; Steel is a server, so those fields and the
//! client-side re-scan are not tracked.
//! Deviation: vanilla draws the hunting target and the ambient-sound jitter from the level's seeded
//! `RandomSource`; Steel has no per-level `RandomSource` (owner: `world`), so those picks use the
//! process RNG.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::sound_events;
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_damage_types;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_mob_effects;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, WorldAabb, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityReference, LivingEntity as _, MobEffectInstance};
use crate::fluid::{FluidStateExt as _, get_fluid_state};
use crate::world::World;

/// Vanilla `ConduitBlockEntity.MIN_ACTIVE_SIZE`.
pub const CONDUIT_MIN_ACTIVE_SIZE: usize = 16;
/// Vanilla `ConduitBlockEntity.MIN_KILL_SIZE`.
pub const CONDUIT_MIN_KILL_SIZE: usize = 42;
/// Vanilla `ConduitBlockEntity.KILL_RANGE`.
pub const CONDUIT_KILL_RANGE: i32 = 8;
/// Vanilla `ConduitBlockEntity.EFFECT_DURATION` (seconds).
pub const CONDUIT_EFFECT_DURATION: i32 = 13;
/// Vanilla `ConduitBlockEntity.BLOCK_REFRESH_RATE`.
pub const CONDUIT_REFRESH_RATE: i64 = 40;

/// Mutable conduit state.
#[derive(Default)]
struct ConduitState {
    /// Whether the frame reaches `MIN_ACTIVE_SIZE`.
    is_active: bool,
    /// Whether the frame reaches `MIN_KILL_SIZE`.
    is_hunting: bool,
    /// Vanilla `destroyTarget`.
    destroy_target: Option<EntityReference>,
    /// Vanilla `nextAmbientSoundActivation`.
    next_ambient_sound_activation: i64,
    /// Vanilla `effectBlocks`: the shell blocks captured by the last shape update.
    effect_blocks: Vec<BlockPos>,
}

/// `ConduitBlockEntity`.
pub struct ConduitBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<ConduitState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ConduitBlockEntity`.
unsafe impl DowncastType for ConduitBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/conduit");
}

impl ConduitBlockEntity {
    /// Creates a conduit block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::CONDUIT, level, pos, state),
            state: SyncMutex::new(ConduitState::default()),
        }
    }

    /// Returns whether the conduit is active (frame >= 16 blocks).
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.state.lock().is_active
    }

    /// Returns whether the conduit is hunting (frame >= 42 blocks).
    #[must_use]
    pub fn is_hunting(&self) -> bool {
        self.state.lock().is_hunting
    }

    /// Vanilla `Vec3i.closerThan`: squared distance below `distance` squared.
    fn closer_than(a: BlockPos, b: BlockPos, distance: f64) -> bool {
        let dx = f64::from(a.x()) - f64::from(b.x());
        let dy = f64::from(a.y()) - f64::from(b.y());
        let dz = f64::from(a.z()) - f64::from(b.z());
        dx * dx + dy * dy + dz * dz < distance * distance
    }

    /// Vanilla `Entity.isInWaterOrRain`: wet, or rained on at the feet or the top of the hitbox.
    fn is_in_water_or_rain(entity: &dyn Entity, world: &World) -> bool {
        if entity.is_in_water() {
            return true;
        }

        let pos = entity.block_position();
        world.is_raining_at(pos)
            || world.is_raining_at(BlockPos::new(
                pos.x(),
                entity.bounding_box().max_y().floor() as i32,
                pos.z(),
            ))
    }

    /// Vanilla `ConduitBlockEntity.updateShape`: requires the full 3×3×3 around the conduit to be
    /// water, then collects the `CONDUIT_EFFECT_BLOCK` blocks on the 5×5×5 shell. An empty result
    /// means the frame is invalid (vanilla returns `false`).
    fn update_shape(world: &Arc<World>, pos: BlockPos) -> Vec<BlockPos> {
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if !get_fluid_state(world, pos.offset(dx, dy, dz)).is_water() {
                        return Vec::new();
                    }
                }
            }
        }

        let mut effect_blocks = Vec::new();
        for dx in -2_i32..=2 {
            for dy in -2_i32..=2 {
                for dz in -2_i32..=2 {
                    let ax = dx.abs();
                    let ay = dy.abs();
                    let az = dz.abs();
                    let on_shell = (ax > 1 || ay > 1 || az > 1)
                        && ((dx == 0 && (ay == 2 || az == 2))
                            || (dy == 0 && (ax == 2 || az == 2))
                            || (dz == 0 && (ax == 2 || ay == 2)));
                    if !on_shell {
                        continue;
                    }

                    let test_pos = pos.offset(dx, dy, dz);
                    if world
                        .get_block_state(test_pos)
                        .get_block()
                        .has_tag(&BlockTag::CONDUIT_EFFECT_BLOCK)
                    {
                        effect_blocks.push(test_pos);
                    }
                }
            }
        }
        effect_blocks
    }

    /// Vanilla `ConduitBlockEntity.applyEffects`: grants `Conduit Power` for 13 seconds to players
    /// within `frame_size / 7 * 16` blocks that are in water or rain.
    fn apply_effects(world: &Arc<World>, pos: BlockPos, frame_size: usize) {
        let effect_range = i32::try_from(frame_size / 7 * 16).unwrap_or(i32::MAX);
        let range = f64::from(effect_range);
        let min = DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
        let aabb = WorldAabb::from_min_max(min, min + DVec3::ONE)
            .inflate(range)
            .expand_towards(DVec3::new(0.0, f64::from(world.get_height()), 0.0));

        for entity in world.get_entities_in_aabb_matching(&aabb, |entity| {
            entity.entity_type() == &vanilla_entities::PLAYER && !entity.is_spectator()
        }) {
            if !Self::closer_than(pos, entity.block_position(), range) {
                continue;
            }
            if !Self::is_in_water_or_rain(entity.as_ref(), world) {
                continue;
            }
            let Some(player) = entity.as_player() else {
                continue;
            };
            player.add_mob_effect(
                MobEffectInstance::with_duration(
                    vanilla_mob_effects::CONDUIT_POWER,
                    CONDUIT_EFFECT_DURATION * 20,
                    0,
                )
                .with_ambient(true)
                .with_visible(true),
            );
        }
    }

    /// Vanilla `ConduitBlockEntity.updateAndAttackTarget`: resolves the hunting target, hurts it
    /// for 4 magic damage and resyncs the block entity when the target changed.
    fn update_and_attack_target(&self, world: &Arc<World>, pos: BlockPos, is_active: bool) {
        let previous = self.state.lock().destroy_target.clone();
        let new_target = Self::update_destroy_target(previous.as_ref(), world, pos, is_active);

        if let Some(target) = new_target
            .as_ref()
            .and_then(|reference| reference.get_entity(world))
        {
            world.play_sound_at(
                &sound_events::BLOCK_CONDUIT_ATTACK_TARGET,
                SoundSource::Blocks,
                target.position(),
                1.0,
                1.0,
                None,
            );
            target.hurt(
                world,
                &DamageSource::environment(&vanilla_damage_types::MAGIC),
                4.0,
            );
        }

        let changed = match (&previous, &new_target) {
            (None, None) => false,
            (Some(old), Some(new)) => old.uuid() != new.uuid(),
            _ => true,
        };
        if changed {
            self.state.lock().destroy_target = new_target;
            world.send_block_updated(pos);
        }
    }

    /// Vanilla `ConduitBlockEntity.updateDestroyTarget`.
    fn update_destroy_target(
        target: Option<&EntityReference>,
        world: &Arc<World>,
        pos: BlockPos,
        is_active: bool,
    ) -> Option<EntityReference> {
        if !is_active {
            return None;
        }

        let Some(target) = target else {
            return Self::select_new_target(world, pos);
        };

        let entity = target.get_entity(world)?;

        (entity.is_alive()
            && Self::closer_than(pos, entity.block_position(), f64::from(CONDUIT_KILL_RANGE)))
        .then(|| target.clone())
    }

    /// Vanilla `ConduitBlockEntity.selectNewTarget`: a random `Enemy` in water or rain within 8
    /// blocks of the conduit.
    fn select_new_target(world: &Arc<World>, pos: BlockPos) -> Option<EntityReference> {
        let min = DVec3::new(f64::from(pos.x()), f64::from(pos.y()), f64::from(pos.z()));
        // Vanilla `new AABB(worldPosition).inflate(8.0)`: the block cube grown by 8 on every side.
        let aabb = WorldAabb::from_min_max(
            min - DVec3::splat(f64::from(CONDUIT_KILL_RANGE)),
            min + DVec3::splat(f64::from(CONDUIT_KILL_RANGE)) + DVec3::ONE,
        );

        let candidates = world.get_entities_in_aabb_matching(&aabb, |entity| {
            entity.as_enemy().is_some() && Self::is_in_water_or_rain(entity, world)
        });
        if candidates.is_empty() {
            return None;
        }

        let index = rand::random_range(0..candidates.len());
        Some(EntityReference::from_entity(&candidates[index]))
    }
}

impl BlockEntity for ConduitBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        // Vanilla `ConduitBlockEntity.loadAdditional` restores only `Target`.
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let target = EntityReference::read(&view, "Target");
        self.state.lock().destroy_target = target;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Vanilla `ConduitBlockEntity.saveAdditional` stores only `Target`.
        if let Some(target) = self.state.lock().destroy_target.as_ref() {
            target.store(nbt, "Target");
        }
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    /// Vanilla `ConduitBlockEntity.serverTick`.
    fn tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let game_time = world.game_time();
        let refreshed = game_time % CONDUIT_REFRESH_RATE == 0;

        let scanned = refreshed.then(|| Self::update_shape(world, pos));
        let (was_active, is_active, frame_size) = {
            let mut state = self.state.lock();
            let was_active = state.is_active;
            if let Some(effect_blocks) = scanned {
                state.is_active = effect_blocks.len() >= CONDUIT_MIN_ACTIVE_SIZE;
                state.is_hunting = effect_blocks.len() >= CONDUIT_MIN_KILL_SIZE;
                state.effect_blocks = effect_blocks;
            }
            (was_active, state.is_active, state.effect_blocks.len())
        };

        if refreshed {
            if is_active != was_active {
                let sound = if is_active {
                    &sound_events::BLOCK_CONDUIT_ACTIVATE
                } else {
                    &sound_events::BLOCK_CONDUIT_DEACTIVATE
                };
                world.play_block_sound(sound, pos, 1.0, 1.0, None);
            }

            if is_active {
                Self::apply_effects(world, pos, frame_size);
                self.update_and_attack_target(world, pos, frame_size >= CONDUIT_MIN_KILL_SIZE);
            }
        }

        if is_active {
            if game_time % 80 == 0 {
                world.play_block_sound(&sound_events::BLOCK_CONDUIT_AMBIENT, pos, 1.0, 1.0, None);
            }

            let play_short_ambient = {
                let mut state = self.state.lock();
                if game_time > state.next_ambient_sound_activation {
                    state.next_ambient_sound_activation =
                        game_time + 60 + rand::random_range(0..40_i64);
                    true
                } else {
                    false
                }
            };
            if play_short_ambient {
                world.play_block_sound(
                    &sound_events::BLOCK_CONDUIT_AMBIENT_SHORT,
                    pos,
                    1.0,
                    1.0,
                    None,
                );
            }
        }
    }
}
