//! Beehive block entity implementation.
//!
//! Mirrors vanilla `BeehiveBlockEntity`: occupant storage and ticking, occupant
//! release with the vanilla spawn offsets, honey-level growth when a nectar
//! carrier leaves, emergency release near fire, and campfire smoke sedation.
//!
//! Deviation: vanilla `BeehiveBlockEntity.releaseOccupant` gates on
//! `EnvironmentAttributes.BEES_STAY_IN_HIVE`; Steel has no environment-attribute
//! system (owner: `steel-core/src/world`), so the attribute always reads false.
//! Deviation: vanilla `BeehiveBlockEntity.Occupant.createEntity` loads the stored
//! compound with `EntityType.loadEntityRecursive`; Steel spawns the entity through
//! `spawn_entity` and then applies the stored compound with
//! `Entity::load_with_owned_components` (owner: entity lane).
//! Deviation: vanilla `Bee.setStayOutOfHiveCountdown(400)` on sedation,
//! `Bee.dropOffNectar`'s crop-counter reset, and `addOccupant`'s `dropLeash` have
//! no Steel bee API (owner: `entity/entities/mobs/neutral/bee/bee.rs` plus the
//! missing `Leashable` re-export); sedation only clears the new target instead.
//! Deviation: vanilla `CampfireBlock.isSmokeyPos` tests the neighbour collision
//! shape including its `getOffset`; Steel's `VoxelShape` has no offset-aware
//! intersection helper (owner: `steel-registry/src/blocks/shapes.rs`), so the
//! static collision shape is tested.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BlockStateProperties, BoolProperty, Direction, EnumProperty, IntProperty,
};
use steel_registry::blocks::shapes::{BooleanOp, VoxelShape, join_is_not_empty};
use steel_registry::data_components::components::{BeehiveOccupant, Bees, CustomData, EntityData};
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_block_entity_types,
    vanilla_blocks, vanilla_entities, vanilla_game_events,
};
use steel_utils::nbt::NbtNumeric as _;
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockLocalAabb, BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey, Identifier,
    locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::entities::BeeEntity;
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, RemovalReason,
    SharedEntity, spawn_entity,
};
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

/// Maximum number of occupants in a vanilla beehive (`BeehiveBlockEntity.MAX_OCCUPANTS`).
pub const BEEHIVE_MAX_OCCUPANTS: usize = 3;
/// Minimum occupation time for bees without nectar (`MIN_OCCUPATION_TICKS_NECTARLESS`).
pub const BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS: i32 = 600;
/// Minimum occupation time for bees that still carry nectar (`MIN_OCCUPATION_TICKS_NECTAR`).
pub const BEEHIVE_MIN_OCCUPATION_TICKS_NECTAR: i32 = 2400;
/// Ticks a released bee must stay out of any hive (`MIN_TICKS_BEFORE_REENTERING_HIVE`).
pub const BEEHIVE_MIN_TICKS_BEFORE_REENTERING_HIVE: i32 = 400;

const LEVEL_HONEY: &IntProperty = &BlockStateProperties::LEVEL_HONEY;
const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;
const LIT: &BoolProperty = &BlockStateProperties::LIT;
const MAX_HONEY_LEVELS: u8 = 5;

/// Vanilla `BeehiveBlockEntity.BeeReleaseStatus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeeReleaseStatus {
    /// The bee delivered nectar, so the hive's honey level grows.
    HoneyDelivered,
    /// The bee simply finished its stay.
    BeeReleased,
    /// The hive is being destroyed or burned; blocking rules are ignored.
    Emergency,
}

/// Vanilla `BeehiveBlockEntity.IGNORED_BEE_TAGS`.
const IGNORED_BEE_TAGS: &[&str] = &[
    "Air",
    "drop_chances",
    "equipment",
    "Brain",
    "CanPickUpLoot",
    "DeathTime",
    "fall_distance",
    "FallFlying",
    "Fire",
    "HurtTime",
    "LeftHanded",
    "Motion",
    "NoGravity",
    "OnGround",
    "PortalCooldown",
    "Pos",
    "Rotation",
    "sleeping_pos",
    "CannotEnterHiveTicks",
    "TicksSincePollination",
    "CropsGrownSincePollination",
    "hive_pos",
    "Passengers",
    "leash",
    "UUID",
];

/// One stored occupant (`BeehiveBlockEntity.Occupant`).
#[derive(Debug, Clone)]
pub struct BeehiveBeeOccupant {
    entity_data: NbtCompound,
    ticks_in_hive: i32,
    min_ticks_in_hive: i32,
}

impl BeehiveBeeOccupant {
    /// Vanilla `Occupant.create(int ticksInHive)` for worldgen bee nests.
    #[must_use]
    pub fn worldgen(ticks_in_hive: i32) -> Self {
        Self {
            entity_data: default_bee_entity_data(),
            ticks_in_hive,
            min_ticks_in_hive: BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS,
        }
    }

    /// Vanilla `Occupant.of(Entity)`: the saved entity data plus release timers.
    #[must_use]
    pub fn of(entity: &dyn Entity) -> Self {
        let mut entity_data = entity.nbt_for_data_compare();
        for tag in IGNORED_BEE_TAGS {
            while entity_data.remove(tag).is_some() {}
        }
        entity_data.insert("id", entity.entity_type().key.to_string());
        let has_nectar = has_nectar_tag(&entity_data);
        Self {
            entity_data,
            ticks_in_hive: 0,
            min_ticks_in_hive: if has_nectar {
                BEEHIVE_MIN_OCCUPATION_TICKS_NECTAR
            } else {
                BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS
            },
        }
    }

    /// Vanilla `Occupant.entityData`.
    #[must_use]
    pub const fn entity_data(&self) -> &NbtCompound {
        &self.entity_data
    }

    /// Vanilla `Occupant.ticksInHive`.
    #[must_use]
    pub const fn ticks_in_hive(&self) -> i32 {
        self.ticks_in_hive
    }

    /// Vanilla `Occupant.minTicksInHive`.
    #[must_use]
    pub const fn min_ticks_in_hive(&self) -> i32 {
        self.min_ticks_in_hive
    }

    /// Vanilla `BeeData.hasNectar`: the stored `HasNectar` flag.
    #[must_use]
    pub fn has_nectar(&self) -> bool {
        has_nectar_tag(&self.entity_data)
    }

    /// Returns the stored occupant as the `minecraft:bees` item component entry.
    #[must_use]
    pub fn to_component(&self) -> BeehiveOccupant {
        let entity_type = entity_type_of(&self.entity_data).unwrap_or(&vanilla_entities::BEE);
        let data = CustomData::try_from_compound(self.entity_data.clone()).unwrap_or_default();
        BeehiveOccupant::new(
            EntityData::new(entity_type, data),
            self.ticks_in_hive,
            self.min_ticks_in_hive,
        )
    }

    /// Vanilla `Occupant` codec entry (`entity_data`, `ticks_in_hive`, `min_ticks_in_hive`).
    fn load(nbt: NbtCompoundView<'_, '_>) -> Self {
        let entity_data = nbt
            .compound("entity_data")
            .map_or_else(default_bee_entity_data, |entity_data| {
                entity_data.to_owned()
            });
        let ticks_in_hive = nbt.int("ticks_in_hive").unwrap_or(0);
        let min_ticks_in_hive = nbt
            .int("min_ticks_in_hive")
            .unwrap_or(BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS);

        Self {
            entity_data,
            ticks_in_hive,
            min_ticks_in_hive,
        }
    }

    /// Vanilla `Occupant` codec entry from an owned compound.
    fn load_owned(nbt: &NbtCompound) -> Self {
        let entity_data = nbt
            .compound("entity_data")
            .cloned()
            .unwrap_or_else(default_bee_entity_data);
        let ticks_in_hive = nbt.int("ticks_in_hive").unwrap_or(0);
        let min_ticks_in_hive = nbt
            .int("min_ticks_in_hive")
            .unwrap_or(BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS);

        Self {
            entity_data,
            ticks_in_hive,
            min_ticks_in_hive,
        }
    }

    fn save(&self) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.insert("entity_data", self.entity_data.clone());
        nbt.insert("ticks_in_hive", self.ticks_in_hive);
        nbt.insert("min_ticks_in_hive", self.min_ticks_in_hive);
        nbt
    }
}

fn default_bee_entity_data() -> NbtCompound {
    let mut entity_data = NbtCompound::new();
    entity_data.insert("id", vanilla_entities::BEE.key.to_string());
    entity_data
}

/// Reads the vanilla `HasNectar` byte flag out of an occupant compound.
fn has_nectar_tag(entity_data: &NbtCompound) -> bool {
    entity_data
        .get("HasNectar")
        .and_then(|tag| tag.codec_i32())
        .is_some_and(|value| value != 0)
}

/// Resolves the stored `id` tag to a registered entity type.
fn entity_type_of(
    entity_data: &NbtCompound,
) -> Option<&'static steel_registry::entity_type::EntityType> {
    let id = entity_data
        .get("id")
        .and_then(|tag| tag.string())
        .map(|value| value.to_str())?;
    let id: Identifier = id.parse().ok()?;
    REGISTRY.entity_types.by_key(&id)
}

/// Vanilla `CampfireBlock.SHAPE_VIRTUAL_POST`: `Block.column(4.0, 0.0, 16.0)`.
const SHAPE_VIRTUAL_POST: VoxelShape =
    VoxelShape::from_boxes(&[BlockLocalAabb::new(0.375, 0.0, 0.375, 0.625, 1.0, 0.625)]);

struct BeehiveState {
    stored: Vec<BeehiveBeeOccupant>,
    saved_flower_pos: Option<BlockPos>,
}

impl BeehiveState {
    fn push_occupant(&mut self, occupant: BeehiveBeeOccupant) -> bool {
        if self.stored.len() >= BEEHIVE_MAX_OCCUPANTS {
            return false;
        }

        self.stored.push(occupant);
        true
    }
}

/// Beehive and bee nest block entity.
pub struct BeehiveBlockEntity {
    base: BlockEntityBase,
    state: SyncMutex<BeehiveState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BeehiveBlockEntity`.
unsafe impl DowncastType for BeehiveBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/beehive");
}

impl BeehiveBlockEntity {
    /// Vanilla `BeeReleaseStatus.HONEY_DELIVERED`.
    pub const HONEY_DELIVERED: BeeReleaseStatus = BeeReleaseStatus::HoneyDelivered;
    /// Vanilla `BeeReleaseStatus.BEE_RELEASED`.
    pub const BEE_RELEASED: BeeReleaseStatus = BeeReleaseStatus::BeeReleased;
    /// Vanilla `BeeReleaseStatus.EMERGENCY`.
    pub const EMERGENCY: BeeReleaseStatus = BeeReleaseStatus::Emergency;

    /// Creates a new beehive block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(&vanilla_block_entity_types::BEEHIVE, level, pos, state),
            state: SyncMutex::new(BeehiveState {
                stored: Vec::new(),
                saved_flower_pos: None,
            }),
        }
    }

    /// Stores a vanilla worldgen bee occupant.
    ///
    /// Mirrors `BeehiveBlockEntity.Occupant.create(ticksInHive)`.
    pub fn store_worldgen_bee(&self, ticks_in_hive: i32) {
        let stored = {
            self.state
                .lock()
                .push_occupant(BeehiveBeeOccupant::worldgen(ticks_in_hive))
        };
        if stored {
            BlockEntity::set_changed(self);
        }
    }

    /// Vanilla `BeehiveBlockEntity.storeBee(Occupant)`.
    ///
    /// Accepts the raw occupant compound (`entity_data`, `ticks_in_hive`,
    /// `min_ticks_in_hive`) so callers can restore saved or item-borne bees.
    pub fn store_bee(&self, occupant: NbtCompound) {
        let occupant = BeehiveBeeOccupant::load_owned(&occupant);
        self.state.lock().push_occupant(occupant);
    }

    /// Vanilla `BeehiveBlockEntity.getOccupantCount`.
    #[must_use]
    pub fn occupant_count(&self) -> usize {
        self.state.lock().stored.len()
    }

    /// Vanilla `BeehiveBlockEntity.isEmpty`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.state.lock().stored.is_empty()
    }

    /// Vanilla `BeehiveBlockEntity.isFull`.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.state.lock().stored.len() >= BEEHIVE_MAX_OCCUPANTS
    }

    /// Returns the stored occupants as `minecraft:bees` component entries.
    #[must_use]
    pub fn collect_bees_component(&self) -> Bees {
        Bees::new(
            self.state
                .lock()
                .stored
                .iter()
                .map(BeehiveBeeOccupant::to_component)
                .collect(),
        )
    }

    /// Vanilla `BeehiveBlockEntity.isFireNearby`.
    #[must_use]
    pub fn is_fire_nearby(&self) -> bool {
        let Some(level) = self.get_level() else {
            return false;
        };
        let pos = self.get_block_pos();
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let block = level.get_block_state(pos.offset(dx, dy, dz)).get_block();
                    if std::ptr::eq(block, &vanilla_blocks::FIRE)
                        || std::ptr::eq(block, &vanilla_blocks::SOUL_FIRE)
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Vanilla `BeehiveBlockEntity.isSedated`: `CampfireBlock.isSmokeyPos`.
    #[must_use]
    pub fn is_sedated(&self) -> bool {
        self.get_level()
            .is_some_and(|level| Self::is_smokey_pos(&level, self.get_block_pos()))
    }

    /// Vanilla `CampfireBlock.isSmokeyPos`.
    #[must_use]
    pub fn is_smokey_pos(world: &World, pos: BlockPos) -> bool {
        for i in 1..=5 {
            let checked = pos.below_n(i);
            let state = world.get_block_state(checked);
            if Self::is_lit_campfire(state) {
                return true;
            }

            let smoke_blocked = join_is_not_empty(
                SHAPE_VIRTUAL_POST,
                state.get_static_collision_shape(),
                BooleanOp::And,
            );
            if smoke_blocked {
                return Self::is_lit_campfire(world.get_block_state(checked.below()));
            }
        }

        false
    }

    /// Vanilla `CampfireBlock.isLitCampfire`.
    fn is_lit_campfire(state: BlockStateId) -> bool {
        state.get_block().has_tag(&BlockTag::CAMPFIRES) && state.try_get_value(LIT).unwrap_or(false)
    }

    /// Vanilla `BeehiveBlockEntity.addOccupant(Bee)`.
    pub fn add_occupant(&self, bee: &SharedEntity) {
        if self.is_full() {
            return;
        }
        bee.stop_riding();

        let occupant = BeehiveBeeOccupant::of(bee.as_ref());
        let flower_pos = bee
            .as_ref()
            .downcast_ref::<BeeEntity>()
            .and_then(BeeEntity::flower_pos);

        {
            let mut state = self.state.lock();
            if !state.push_occupant(occupant) {
                return;
            }
            if let Some(flower_pos) = flower_pos
                && (state.saved_flower_pos.is_none() || rand::random_bool(0.5))
            {
                state.saved_flower_pos = Some(flower_pos);
            }
        }

        let Some(world) = self.get_level() else {
            bee.set_removed(RemovalReason::Discarded);
            return;
        };
        let pos = self.get_block_pos();
        world.play_block_sound(&sound_events::BLOCK_BEEHIVE_ENTER, pos, 1.0, 1.0, None);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(Some(bee.as_ref()), Some(self.get_block_state())),
        );
        bee.set_removed(RemovalReason::Discarded);
        BlockEntity::set_changed(self);
    }

    /// Vanilla `BeehiveBlockEntity.releaseAllOccupants`.
    pub fn release_all_occupants(
        &self,
        state: BlockStateId,
        status: BeeReleaseStatus,
    ) -> Vec<SharedEntity> {
        let Some(world) = self.get_level() else {
            return Vec::new();
        };
        let pos = self.get_block_pos();
        let (occupants, saved_flower_pos) = {
            let mut stored = self.state.lock();
            let saved_flower_pos = stored.saved_flower_pos;
            (std::mem::take(&mut stored.stored), saved_flower_pos)
        };

        let mut spawned = Vec::new();
        let mut remaining = Vec::new();
        for occupant in occupants {
            if !Self::release_occupant(
                &world,
                pos,
                state,
                &occupant,
                Some(&mut spawned),
                status,
                saved_flower_pos,
            ) {
                remaining.push(occupant);
            }
        }

        if !remaining.is_empty() {
            self.state.lock().stored = remaining;
        }
        if !spawned.is_empty() {
            self.base.set_changed();
        }
        spawned
    }

    /// Vanilla `BeehiveBlockEntity.emptyAllLivingFromHive`.
    pub fn empty_all_living_from_hive(
        &self,
        player: Option<&Player>,
        state: BlockStateId,
        status: BeeReleaseStatus,
    ) {
        let released = self.release_all_occupants(state, status);
        let Some(player) = player else {
            return;
        };
        let Some(world) = self.get_level() else {
            return;
        };
        let Some(player_entity) = world.get_entity_by_id(player.id()) else {
            return;
        };
        let sedated = self.is_sedated();
        for entity in released {
            if entity.as_ref().downcast_ref::<BeeEntity>().is_none() {
                continue;
            }
            if player.position().distance_squared(entity.position()) > 16.0 {
                continue;
            }
            let Some(mob) = entity.as_mob() else {
                continue;
            };
            if sedated {
                // Vanilla sets a 400-tick stay-out countdown; see the module docs.
                let _ = mob.set_target(None);
            } else {
                let _ = mob.set_target(Some(&player_entity));
            }
        }
    }

    /// Vanilla `BeehiveBlockEntity.serverTick`.
    pub fn server_tick(&self, world: &Arc<World>) {
        let pos = self.get_block_pos();
        let state = self.get_block_state();
        let saved_flower_pos = self.state.lock().saved_flower_pos;

        let mut changed = false;
        let mut still_stored = Vec::new();
        let occupants = {
            let mut stored = self.state.lock();
            std::mem::take(&mut stored.stored)
        };
        for mut occupant in occupants {
            let release = occupant.ticks_in_hive > occupant.min_ticks_in_hive;
            occupant.ticks_in_hive += 1;
            if release {
                let status = if occupant.has_nectar() {
                    BeeReleaseStatus::HoneyDelivered
                } else {
                    BeeReleaseStatus::BeeReleased
                };
                if Self::release_occupant(
                    world,
                    pos,
                    state,
                    &occupant,
                    None,
                    status,
                    saved_flower_pos,
                ) {
                    changed = true;
                    continue;
                }
            }
            still_stored.push(occupant);
        }
        self.state.lock().stored = still_stored;

        if changed {
            self.base.set_changed();
        }

        if !self.is_empty() && rand::random::<f64>() < 0.005 {
            world.play_block_sound(&sound_events::BLOCK_BEEHIVE_WORK, pos, 1.0, 1.0, None);
        }
    }

    /// Vanilla `BeehiveBlockEntity.releaseOccupant`.
    fn release_occupant(
        world: &Arc<World>,
        block_pos: BlockPos,
        state: BlockStateId,
        occupant: &BeehiveBeeOccupant,
        mut spawned: Option<&mut Vec<SharedEntity>>,
        status: BeeReleaseStatus,
        saved_flower_pos: Option<BlockPos>,
    ) -> bool {
        let facing = state.get_value(HORIZONTAL_FACING);
        let facing_pos = block_pos.relative(facing);
        let front_blocked = !world
            .get_block_state(facing_pos)
            .get_static_collision_shape()
            .is_empty();
        if front_blocked && status != BeeReleaseStatus::Emergency {
            return false;
        }

        let Some(entity_type) = entity_type_of(&occupant.entity_data) else {
            return false;
        };
        if !REGISTRY
            .entity_types
            .is_in_tag(entity_type, &EntityTypeTag::BEEHIVE_INHABITORS)
        {
            return false;
        }

        let request = EntitySpawnRequest {
            entity_type,
            placement: EntitySpawnPlacement::Block {
                pos: block_pos,
                try_move_down: false,
                moved_up: false,
            },
            reason: EntitySpawnReason::Load,
            finalize_spawn: false,
            play_ambient_sound: false,
            item_stack: None,
            user_is_operator: false,
        };
        let Ok(entity) = spawn_entity(world, request) else {
            return false;
        };

        let mut data = occupant.entity_data.clone();
        while data.remove("id").is_some() {}
        for tag in IGNORED_BEE_TAGS {
            while data.remove(tag).is_some() {}
        }
        let _ = entity.load_with_owned_components(&data);
        entity.set_no_gravity(true);

        if let Some(bee) = entity.as_ref().downcast_ref::<BeeEntity>() {
            bee.set_hive_pos(Some(block_pos));
            if let Some(saved_flower_pos) = saved_flower_pos
                && bee.flower_pos().is_none()
                && rand::random::<f32>() < 0.9
            {
                bee.set_flower_pos(Some(saved_flower_pos));
            }

            if status == BeeReleaseStatus::HoneyDelivered {
                bee.set_has_nectar(false);
                Self::grow_honey_level(world, block_pos, state);
            }
        }

        Self::update_bee_age(occupant.ticks_in_hive, entity.as_ref());
        if let Some(animal) = entity.as_animal() {
            animal.set_in_love_time((animal.in_love_time() - occupant.ticks_in_hive).max(0));
        }

        if let Some(spawned) = spawned.as_deref_mut() {
            spawned.push(SharedEntity::clone(&entity));
        }

        let width = entity.bounding_box().width();
        let height = entity.bounding_box().height();
        let (step_x, _, step_z) = facing.offset();
        let delta = if front_blocked {
            0.0
        } else {
            0.55 + width / 2.0
        };
        let spawn_x = f64::from(block_pos.x()) + 0.5 + delta * f64::from(step_x);
        let spawn_y = f64::from(block_pos.y()) + 0.5 - height / 2.0;
        let spawn_z = f64::from(block_pos.z()) + 0.5 + delta * f64::from(step_z);
        let (yaw, pitch) = entity.rotation();
        entity.snap_to(DVec3::new(spawn_x, spawn_y, spawn_z), yaw, pitch);

        world.play_block_sound(&sound_events::BLOCK_BEEHIVE_EXIT, block_pos, 1.0, 1.0, None);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            block_pos,
            &GameEventContext::new(
                Some(entity.as_ref()),
                Some(world.get_block_state(block_pos)),
            ),
        );
        true
    }

    /// Vanilla `releaseOccupant`'s `HONEY_DELIVERED` honey-level growth.
    fn grow_honey_level(world: &Arc<World>, block_pos: BlockPos, state: BlockStateId) {
        if !state.get_block().has_tag(&BlockTag::BEEHIVES) {
            return;
        }
        let Some(honey_level) = state.try_get_value(LEVEL_HONEY) else {
            return;
        };
        if honey_level >= MAX_HONEY_LEVELS {
            return;
        }

        let mut increase = if rand::random_range(0..100) == 0 {
            2
        } else {
            1
        };
        if honey_level + increase > MAX_HONEY_LEVELS {
            increase -= 1;
        }
        world.set_block(
            block_pos,
            state.set_value(LEVEL_HONEY, honey_level + increase),
            UpdateFlags::UPDATE_ALL,
        );
    }

    /// Vanilla `Occupant.updateBeeAge`.
    fn update_bee_age(ticks_in_hive: i32, entity: &dyn Entity) {
        let Some(ageable) = entity.as_ageable_mob() else {
            return;
        };
        if ageable.is_age_locked() {
            return;
        }
        let age = ageable.get_age();
        if age < 0 {
            ageable.set_age(age.saturating_add(ticks_in_hive).min(0));
        } else if age > 0 {
            ageable.set_age(age.saturating_sub(ticks_in_hive).max(0));
        }
    }
}

impl BlockEntity for BeehiveBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let mut stored = Vec::new();

        if let Some(bees) = nbt.list("bees")
            && let Some(compounds) = bees.compounds()
        {
            for compound in compounds {
                if stored.len() >= BEEHIVE_MAX_OCCUPANTS {
                    break;
                }
                stored.push(BeehiveBeeOccupant::load(compound));
            }
        }

        let saved_flower_pos = nbt.int_array("flower_pos").and_then(|values| {
            let [x, y, z] = <[i32; 3]>::try_from(values).ok()?;
            Some(BlockPos::new(x, y, z))
        });

        let mut state = self.state.lock();
        state.stored = stored;
        state.saved_flower_pos = saved_flower_pos;
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        let state = self.state.lock();
        let bees = state
            .stored
            .iter()
            .map(BeehiveBeeOccupant::save)
            .collect::<Vec<_>>();
        nbt.insert("bees", NbtList::Compound(bees));
        if let Some(pos) = state.saved_flower_pos {
            nbt.insert("flower_pos", NbtList::Int(vec![pos.x(), pos.y(), pos.z()]));
        }
    }

    fn tick(&self, world: &Arc<World>) {
        self.server_tick(world);
    }

    fn set_changed(&self) {
        if self.is_fire_nearby() {
            let state = self.get_block_state();
            self.empty_all_living_from_hive(None, state, BeeReleaseStatus::Emergency);
        }

        self.base.set_changed();
    }

    fn apply_components_from_item(&self, item: &ItemStack) {
        let bees = item.get_or_default(
            steel_registry::data_components::vanilla_components::BEES,
            Bees::empty(),
        );
        if bees.bees().is_empty() {
            return;
        }

        let mut state = self.state.lock();
        state.stored.clear();
        for bee in bees.bees() {
            let mut entity_data = bee.entity_data().data().copy_tag();
            entity_data.insert("id", bee.entity_data().entity_type().key.to_string());
            state.stored.push(BeehiveBeeOccupant {
                entity_data,
                ticks_in_hive: bee.ticks_in_hive(),
                min_ticks_in_hive: bee.min_ticks_in_hive(),
            });
        }
    }
}
