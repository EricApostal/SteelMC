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
//! Deviation: vanilla `addOccupant` also calls `dropLeash`; Steel's `Leashable` has no
//! matching re-export in this module's reach, so only `stop_riding`/`eject_passengers`
//! equivalents run.
//! Deviation: vanilla `CampfireBlock.isSmokeyPos` tests the neighbour collision
//! shape including its `getOffset`; Steel's `VoxelShape` has no offset-aware
//! intersection helper (owner: `steel-registry/src/blocks/shapes.rs`), so the
//! static collision shape is tested.

use std::mem;
use std::ptr;
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
use steel_registry::data_components::vanilla_components::BEES;
use steel_registry::entity_type::EntityType;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_block_entity_types,
    vanilla_blocks, vanilla_entities, vanilla_game_events,
};
use steel_utils::nbt::NbtNumeric;
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockLocalAabb, BlockPos, BlockStateId, Downcast, DowncastType, DowncastTypeKey, Identifier,
    locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::entity::entities::BeeEntity;
use crate::entity::{
    Entity, EntitySpawnPlacement, EntitySpawnReason, EntitySpawnRequest, Mob, RemovalReason,
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
/// Vanilla `BeehiveBlockEntity.MIN_TICKS_BEFORE_REENTERING_HIVE`, applied to sedated bees.
pub const MIN_TICKS_BEFORE_REENTERING_HIVE: i32 = 400;
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
        .and_then(NbtNumeric::codec_i32)
        .is_some_and(|value| value != 0)
}

/// Resolves the stored `id` tag to a registered entity type.
fn entity_type_of(entity_data: &NbtCompound) -> Option<&'static EntityType> {
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
                    if ptr::eq(block, &raw const vanilla_blocks::FIRE)
                        || ptr::eq(block, &raw const vanilla_blocks::SOUL_FIRE)
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
            (mem::take(&mut stored.stored), saved_flower_pos)
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
            let Some(bee) = entity.as_ref().downcast_ref::<BeeEntity>() else {
                continue;
            };
            if player.position().distance_squared(entity.position()) > 16.0 {
                continue;
            }
            if sedated {
                // Vanilla `BeehiveBlockEntity.emptyAllLivingFromHive`:
                // `bee.setStayOutOfHiveCountdown(MIN_TICKS_BEFORE_REENTERING_HIVE)`.
                bee.set_stay_out_of_hive_countdown(MIN_TICKS_BEFORE_REENTERING_HIVE);
            } else {
                let _ = Mob::set_target(bee, Some(&player_entity));
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
            mem::take(&mut stored.stored)
        };
        for mut occupant in occupants {
            // Vanilla `BeeData.tick`: `++ticksInHive > minTicksInHive`.
            occupant.ticks_in_hive += 1;
            let release = occupant.ticks_in_hive > occupant.min_ticks_in_hive;
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
        spawned: Option<&mut Vec<SharedEntity>>,
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
                // Vanilla `BeehiveBlockEntity.releaseOccupant` → `Bee.dropOffNectar`, which clears
                // the nectar flag and resets the crop counter.
                bee.drop_off_nectar();
                Self::grow_honey_level(world, block_pos, state);
            }
        }

        Self::update_bee_age(occupant.ticks_in_hive, entity.as_ref());
        if let Some(animal) = entity.as_animal() {
            animal.set_in_love_time((animal.in_love_time() - occupant.ticks_in_hive).max(0));
        }

        if let Some(spawned) = spawned {
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
        let bees = item.get_or_default(BEES, Bees::empty());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::behavior::init_behaviors;
    use crate::block_entity::{SharedBlockEntity, init_block_entities};
    use crate::entity::entities::BeeEntity;
    use crate::entity::init_entities;
    use crate::test_support::{TestWorld, fresh_test_world, insert_ready_full_chunk};
    use glam::DVec3;
    use steel_registry::blocks::properties::Direction;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
    use steel_utils::ChunkPos;

    const HIVE_POS: BlockPos = BlockPos::new(4, 64, 4);

    fn beehive_state() -> BlockStateId {
        vanilla_blocks::BEEHIVE
            .default_state()
            .set_value(HORIZONTAL_FACING, Direction::North)
    }

    fn honey_level(world: &World) -> u8 {
        world.get_block_state(HIVE_POS).get_value(LEVEL_HONEY)
    }

    /// Builds a loaded test world with a placed beehive, its block entity, and (optionally) a
    /// nectar-carrying bee that has just entered the hive.
    fn hive_with_bee(
        name: &'static str,
        nectar: bool,
    ) -> (TestWorld, Arc<World>, SharedBlockEntity, SharedEntity) {
        init_vanilla_registry();
        init_behaviors();
        init_entities();
        init_block_entities();
        let fixture = fresh_test_world(name);
        let world = Arc::clone(&fixture.world);
        insert_ready_full_chunk(&world, ChunkPos::new(0, 0));
        world.set_block(HIVE_POS, beehive_state(), UpdateFlags::UPDATE_ALL);

        let hive: SharedBlockEntity = Arc::new(BeehiveBlockEntity::new(
            Arc::downgrade(&world),
            HIVE_POS,
            beehive_state(),
        ));

        let bee = Arc::new(BeeEntity::new(
            &vanilla_entities::BEE,
            1,
            DVec3::new(4.5, 64.0, 4.5),
            Arc::downgrade(&world),
        ));
        bee.set_has_nectar(nectar);
        bee.set_flower_pos(Some(BlockPos::new(6, 64, 6)));
        let shared: SharedEntity = Arc::clone(&bee) as SharedEntity;

        hive.downcast_ref::<BeehiveBlockEntity>()
            .expect("test hive should be a beehive")
            .add_occupant(&shared);

        (fixture, world, hive, shared)
    }

    #[test]
    fn occupant_strips_exactly_the_ignored_bee_tags() {
        init_vanilla_registry();
        let bee = BeeEntity::new(&vanilla_entities::BEE, 1, DVec3::ZERO, Weak::new());
        bee.set_hive_pos(Some(HIVE_POS));
        bee.set_flower_pos(Some(BlockPos::new(6, 64, 6)));
        bee.set_has_nectar(true);
        bee.set_stay_out_of_hive_countdown(7);
        bee.increment_crops_grown_since_pollination();

        let occupant = BeehiveBeeOccupant::of(&bee);
        let data = &occupant.entity_data;

        // `hive_pos` and the three counters are stripped, so a bee cannot leak its hive back
        // into the world through the stored occupant.
        assert!(data.get("hive_pos").is_none(), "hive_pos must be stripped");
        assert!(data.get("TicksSincePollination").is_none());
        assert!(data.get("CropsGrownSincePollination").is_none());
        assert!(data.get("CannotEnterHiveTicks").is_none());
        assert!(data.get("Air").is_none());
        assert!(data.get("Pos").is_none());
        assert!(data.get("UUID").is_none());

        // `flower_pos` and `HasNectar` survive, which is what the release path reads.
        assert!(data.get("flower_pos").is_some());
        assert!(data.get("HasNectar").is_some());
        assert!(occupant.has_nectar());
        assert_eq!(
            occupant.min_ticks_in_hive,
            BEEHIVE_MIN_OCCUPATION_TICKS_NECTAR
        );
    }

    #[test]
    fn hive_records_an_occupant_and_grows_honey_at_the_vanilla_dwell() {
        let (_fixture, world, hive, bee) = hive_with_bee("beehive_dwell", true);
        let hive = hive
            .downcast_ref::<BeehiveBlockEntity>()
            .expect("test hive should be a beehive");

        assert_eq!(hive.occupant_count(), 1, "the bee should be an occupant");
        // Vanilla discards the occupant, so no AI tick can run for it.
        assert_eq!(bee.removal_reason(), Some(RemovalReason::Discarded));

        // `BeeData.tick` releases on the first tick past `minTicksInHive`.
        for _ in 0..BEEHIVE_MIN_OCCUPATION_TICKS_NECTAR {
            hive.server_tick(&world);
        }
        assert_eq!(hive.occupant_count(), 1, "dwell must not release early");
        assert_eq!(honey_level(&world), 0);

        hive.server_tick(&world);
        assert_eq!(hive.occupant_count(), 0, "dwell must release the bee");
        // Vanilla grows the honey level by one, or by two on the 1/100 roll.
        assert!(
            (1..=2).contains(&honey_level(&world)),
            "honey level should grow once, got {}",
            honey_level(&world)
        );

        let released = world
            .get_entities_in_aabb_matching(
                &steel_utils::WorldAabb::of_size(DVec3::new(4.5, 64.5, 3.5), 3.0, 3.0, 3.0),
                |entity| entity.entity_type() == &vanilla_entities::BEE,
            )
            .into_iter()
            .filter_map(|entity| {
                entity
                    .as_ref()
                    .downcast_ref::<BeeEntity>()
                    .map(BeeEntity::has_nectar)
            })
            .collect::<Vec<_>>();
        assert_eq!(released.len(), 1, "the occupant should have been released");
        assert!(!released[0], "delivery must clear the nectar");
    }

    #[test]
    fn nectarless_occupants_dwell_for_the_short_occupation_time() {
        let (_fixture, world, hive, _bee) = hive_with_bee("beehive_nectarless_dwell", false);
        let hive = hive
            .downcast_ref::<BeehiveBlockEntity>()
            .expect("test hive should be a beehive");

        for _ in 0..BEEHIVE_MIN_OCCUPATION_TICKS_NECTARLESS {
            hive.server_tick(&world);
        }
        assert_eq!(hive.occupant_count(), 1);
        assert_eq!(honey_level(&world), 0);

        hive.server_tick(&world);
        assert_eq!(hive.occupant_count(), 0);
        assert_eq!(
            honey_level(&world),
            0,
            "a nectar-less release must not grow the honey level"
        );
    }
}
