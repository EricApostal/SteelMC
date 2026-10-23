//! `ConduitBlockEntity` (`net.minecraft.world.level.block.entity.ConduitBlockEntity`).
//!
//! Vanilla re-scans the conduit frame every 40 ticks (`updateShape`), applies `Conduit Power` to
//! nearby players while active, and damages hostile mobs within 8 blocks once the frame reaches 42
//! blocks (`updateHunting`).

use std::sync::Weak;

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_block_entity_types;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_mob_effects;
use steel_utils::{
    BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey, WorldAabb,
    locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::{Entity, LivingEntity as _, MobEffectInstance};
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
    /// Frames blocks found around the conduit (`effectBlocks`).
    effect_blocks: Vec<BlockPos>,
    /// Whether the frame reaches `MIN_ACTIVE_SIZE`.
    is_active: bool,
    /// Whether the frame reaches `MIN_KILL_SIZE`.
    is_hunting: bool,
    /// Vanilla `tickCount`.
    tick_count: i32,
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

    /// Vanilla `ConduitBlockEntity.updateShape`: collects the prismarine-family blocks in the
    /// 3×3×3 ring around the conduit; the frame stays active from `MIN_ACTIVE_SIZE` blocks and
    /// hunting from `MIN_KILL_SIZE`.
    fn update_shape(world: &World, pos: BlockPos) -> Vec<BlockPos> {
        let mut effect_blocks = Vec::new();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let neighbor = BlockPos::new(pos.x() + dx, pos.y() + dy, pos.z() + dz);
                    let state = world.get_block_state(neighbor);
                    if state.get_block().has_tag(&BlockTag::CONDUIT_EFFECT_BLOCK) {
                        effect_blocks.push(neighbor);
                    }
                }
            }
        }
        effect_blocks
    }

    /// Vanilla `ConduitBlockEntity.applyEffects`: grants `Conduit Power` to non-spectator players
    /// within 8 blocks (plus one block per extra frame block beyond the minimum).
    fn apply_effects(world: &std::sync::Arc<World>, pos: BlockPos, effect_blocks: usize) {
        let range =
            CONDUIT_KILL_RANGE + (effect_blocks.saturating_sub(CONDUIT_MIN_ACTIVE_SIZE) / 7) as i32;
        let range = f64::from(range);
        let center = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        );
        let aabb =
            WorldAabb::from_min_max(center - DVec3::splat(range), center + DVec3::splat(range));
        for entity in world.get_entities_in_aabb_matching(&aabb, |entity| {
            entity.entity_type() == &vanilla_entities::PLAYER && !entity.is_spectator()
        }) {
            let Some(player) = entity.downcast_ref::<crate::player::Player>() else {
                continue;
            };
            player.add_mob_effect(
                MobEffectInstance::with_duration(
                    &vanilla_mob_effects::CONDUIT_POWER,
                    CONDUIT_EFFECT_DURATION * 20,
                    0,
                )
                .with_ambient(true)
                .with_visible(true),
            );
        }
    }

    /// Vanilla `ConduitBlockEntity.updateHunting`: picks a hostile mob within 8 blocks and hurts it
    /// with `indirectMagic`.
    fn update_hunting(world: &std::sync::Arc<World>, pos: BlockPos, target: Option<&dyn Entity>) {
        let Some(target) = target else {
            return;
        };
        target.hurt(
            world,
            &crate::entity::damage::DamageSource::environment(
                &steel_registry::vanilla_damage_types::INDIRECT_MAGIC,
            ),
            4.0,
        );
        let _ = pos;
    }
}

impl BlockEntity for ConduitBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        // Vanilla persists the destroy target as an entity reference; the link is re-resolved from
        // the world each hunting scan, so only the raw uuid is retained here.
        let view: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.state.lock();
        state.is_active = false;
        state.is_hunting = false;
        state.tick_count = 0;
        let _ = view;
    }

    fn save_additional(&self, _nbt: &mut NbtCompound) {
        // Vanilla only writes `Target` (a transient hunting target); nothing persistent remains
        // between sessions, matching an unset target.
    }

    fn get_update_tag(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        self.save_additional(&mut nbt);
        Some(nbt)
    }

    fn tick(&self, world: &std::sync::Arc<World>) {
        let pos = self.get_block_pos();
        let refresh = world.game_time() % CONDUIT_REFRESH_RATE == 0;

        let effect_blocks = {
            let mut state = self.state.lock();
            state.tick_count += 1;
            if refresh {
                state.effect_blocks = Self::update_shape(world, pos);
                state.is_active = state.effect_blocks.len() >= CONDUIT_MIN_ACTIVE_SIZE;
                state.is_hunting = state.effect_blocks.len() >= CONDUIT_MIN_KILL_SIZE;
            }
            state.effect_blocks.len()
        };

        let (is_active, is_hunting) = {
            let state = self.state.lock();
            (state.is_active, state.is_hunting)
        };
        if !is_active {
            return;
        }

        Self::apply_effects(world, pos, effect_blocks);
        if is_hunting {
            let center = DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            );
            let range = f64::from(CONDUIT_KILL_RANGE);
            let aabb =
                WorldAabb::from_min_max(center - DVec3::splat(range), center + DVec3::splat(range));
            let target = world
                .get_entities_in_aabb_matching(&aabb, |entity| {
                    entity.entity_type() == &vanilla_entities::DROWNED
                        || entity.entity_type() == &vanilla_entities::GUARDIAN
                        || entity.entity_type() == &vanilla_entities::ELDER_GUARDIAN
                })
                .into_iter()
                .next();
            if let Some(target) = target {
                Self::update_hunting(world, pos, Some(target.as_ref()));
            }
        }
    }
}
