//! Boat entities: vanilla `AbstractBoat`, `Boat`, `Raft`, `AbstractChestBoat`,
//! `ChestBoat` and `ChestRaft`.
//!
//! One struct per concrete vanilla class covers every entity type that maps to it,
//! because the entity type *is* the wood variant (vanilla registers each boat type
//! with its own dimensions and drop item; no variant field is saved):
//!
//! | Struct | Vanilla class | Entity types |
//! | --- | --- | --- |
//! | [`BoatEntity`] | `Boat` | `oak_boat` … `pale_oak_boat` |
//! | [`ChestBoatEntity`] | `ChestBoat` | `oak_chest_boat` … `pale_oak_chest_boat` |
//! | [`RaftEntity`] | `Raft` | `bamboo_raft` |
//! | [`ChestRaftEntity`] | `ChestRaft` | `bamboo_chest_raft` |
//!
//! Implemented from `AbstractBoat`: the five-state float status machine
//! (`IN_WATER`/`UNDER_WATER`/`UNDER_FLOWING_WATER`/`ON_LAND`/`IN_AIR`), buoyancy and
//! per-status friction, `outOfControlTicks` passenger ejection, bubble-column wobble,
//! fall-distance bookkeeping, paddle animation and sounds, occupant seating, nearby
//! entity boarding/pushing, `VehicleEntity` damage and drops, and chest-boat storage
//! (`Items` list plus the deferred `LootTable`/`LootTableSeed` fields).
//!
//! Divergences from vanilla, all deliberate:
//!
//! - Steel is server-authoritative only, so `AbstractBoat.tick`'s client branch
//!   (`controlBoat()` plus `ServerboundPaddleBoatPacket`) is inverted: the server runs
//!   `controlBoat()` itself from the input flags, which the protocol lane fills through
//!   the public `set_input`/`set_paddle_state` methods.
//! - Vanilla `AbstractBoat.push` gates pushing on the pushed entity's bounding box.
//!   Steel's `Entity::push_entity` default cannot be reached from an override, so that
//!   gate is applied where the boat itself pushes entities, in its tick scan.
//! - Lily pads are not broken here: vanilla moved that into `LilyPadBlock.entityInside`,
//!   which Steel already implements in `behavior/blocks/vegetation/lily_pad_block.rs`.
//! - Vanilla persists no paddle NBT in this version (paddle state is synced data only),
//!   so the paddle state round-trips through `DATA_ID_PADDLE_LEFT`/`RIGHT` rather than
//!   through a saved tag.
//! - Only vanilla's server bubble-column branch is modelled (`tickBubbleColumn`); the
//!   client-only wobble-angle interpolation has no server analogue.
//!
//! Framework gaps this file cannot close (owned elsewhere):
//!
//! - No `getDismountLocationForPassenger` hook on `Entity`; boats fall back to the
//!   generic dismount helper.
//! - No entity-container menu open path (`Player.openMenu(this)`), so a chest boat's
//!   menu cannot be opened from here.
//! - No loot-table fill API for entity containers, so `LootTable`/`LootTableSeed` are
//!   persisted but not unpacked on open.
//! - No entity-to-team lookup, so the team `collisionRule` half of
//!   `EntitySelector.pushableBy` cannot be evaluated; the spectator and pushable filters
//!   are applied.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::REGISTRY;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::fluid::FluidStateExt;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::AbstractBoatEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::vanilla_game_rules::ENTITY_DROPS;
use steel_registry::vanilla_particle_types::SPLASH;
use steel_registry::{
    RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_blocks, vanilla_game_events,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, WorldAabb};

use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::leash::{LeashData, Leashable};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityMovementEmission, EntitySyncedData, RemovalReason,
    SharedEntity, reset_forward_direction_of_relative_portal_position,
};
use crate::fluid::{get_fluid_state, get_height};
use crate::inventory::container::{Container, SimpleContainer};
use crate::physics::MoverType;
use crate::physics::collision::{WorldCollisionProvider, has_block_collision};
use crate::physics::shapes::join_is_not_empty;
use crate::player::Player;
use crate::world::World;

/// Ticks a boat may stay submerged before its passengers are ejected.
const TIME_TO_EJECT: f32 = 60.0;

/// Paddle animation advance per tick, vanilla `PADDLE_SPEED`.
const PADDLE_SPEED: f32 = std::f32::consts::PI / 8.0;

/// Bubble-column wobble duration in ticks, vanilla `BUBBLE_TIME`.
const BUBBLE_TIME: i32 = 60;

/// Default boat gravity, vanilla `AbstractBoat.getDefaultGravity`.
const BOAT_GRAVITY: f64 = 0.04;

/// Vanilla `AbstractBoat.getAirDrag`.
const BOAT_AIR_DRAG: f64 = 1.0;

/// Slot count of a chest boat, vanilla `AbstractChestBoat.CONTAINER_SIZE`.
const CHEST_BOAT_SLOTS: usize = 27;

/// Boat float status, vanilla `AbstractBoat.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoatStatus {
    /// Hull is touching water but the deck is above the surface.
    InWater,
    /// Hull is fully submerged in still water.
    UnderWater,
    /// Hull is fully submerged in flowing water.
    UnderFlowingWater,
    /// Hull is resting on a block.
    OnLand,
    /// Hull is neither in water nor supported by a block.
    InAir,
}

impl BoatStatus {
    /// Whether this status counts as underwater, vanilla `AbstractBoat.isUnderWater`.
    #[must_use]
    pub const fn is_under_water(self) -> bool {
        matches!(self, Self::UnderWater | Self::UnderFlowingWater)
    }
}

/// Mutable `AbstractBoat` state that is neither synced nor persisted.
struct BoatState {
    paddle_positions: [f32; 2],
    out_of_control_ticks: f32,
    delta_rotation: f32,
    input_left: bool,
    input_right: bool,
    input_up: bool,
    input_down: bool,
    water_level: f64,
    land_friction: f32,
    status: BoatStatus,
    old_status: BoatStatus,
    last_yd: f64,
    is_above_bubble_column: bool,
    bubble_column_direction_is_down: bool,
}

impl BoatState {
    fn new() -> Self {
        Self {
            paddle_positions: [0.0; 2],
            out_of_control_ticks: 0.0,
            delta_rotation: 0.0,
            input_left: false,
            input_right: false,
            input_up: false,
            input_down: false,
            water_level: 0.0,
            land_friction: 0.0,
            status: BoatStatus::InAir,
            old_status: BoatStatus::InAir,
            last_yd: 0.0,
            is_above_bubble_column: false,
            bubble_column_direction_is_down: false,
        }
    }
}

/// Chest-boat container contents: vanilla `AbstractChestBoat`'s `itemStacks` plus the
/// deferred loot-table fields declared by `ContainerEntity`.
///
/// Saving is mutually exclusive like vanilla `addChestVehicleSaveData`: an unresolved
/// loot table is written as `LootTable`/`LootTableSeed`, otherwise the occupied slots
/// are written as a `Slot`-indexed `Items` list.
pub struct ChestBoatStorage {
    items: SimpleContainer,
    loot_table: Option<Identifier>,
    loot_table_seed: i64,
}

impl ChestBoatStorage {
    /// Creates an empty 27-slot chest-boat container.
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: SimpleContainer::new(CHEST_BOAT_SLOTS),
            loot_table: None,
            loot_table_seed: 0,
        }
    }

    /// Returns all 27 slots.
    #[must_use]
    pub fn items(&self) -> &[ItemStack] {
        self.items.items()
    }

    /// Returns the stack in `slot`, vanilla `getChestVehicleItem`.
    #[must_use]
    pub fn get_item(&self, slot: usize) -> Option<&ItemStack> {
        self.items.items().get(slot)
    }

    /// Replaces the stack in `slot`, vanilla `setChestVehicleItem`.
    pub fn set_item(&mut self, slot: usize, stack: ItemStack) {
        if let Some(current) = self.items.items_mut().get_mut(slot) {
            *current = stack;
        }
    }

    /// Clears every slot, vanilla `clearChestVehicleContent`.
    pub fn clear(&mut self) {
        self.items.items_mut().fill(ItemStack::empty());
    }

    /// Returns whether every slot is empty, vanilla `isChestVehicleEmpty`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.items().iter().all(ItemStack::is_empty)
    }

    /// Returns the deferred loot table, if this chest has not been generated yet.
    #[must_use]
    pub fn loot_table(&self) -> Option<&Identifier> {
        self.loot_table.as_ref()
    }

    /// Returns the deferred loot-table seed.
    #[must_use]
    pub const fn loot_table_seed(&self) -> i64 {
        self.loot_table_seed
    }

    /// Sets the deferred loot table used when the container is first opened.
    pub fn set_loot_table(&mut self, loot_table: Identifier, seed: i64) {
        self.loot_table = Some(loot_table);
        self.loot_table_seed = seed;
    }

    /// Writes the vanilla `ContainerEntity` save data.
    fn save(&self, nbt: &mut NbtCompound) {
        if let Some(loot_table) = self.loot_table.as_ref() {
            nbt.insert("LootTable", loot_table.to_string());
            if self.loot_table_seed != 0 {
                nbt.insert("LootTableSeed", NbtTag::Long(self.loot_table_seed));
            }
            return;
        }

        let mut saved = Vec::new();
        for (slot, stack) in self.items.items().iter().enumerate() {
            if stack.is_empty() {
                continue;
            }
            if let NbtTag::Compound(mut compound) = stack.to_nbt_tag_ref() {
                compound.insert("Slot", slot as i8);
                saved.push(compound);
            }
        }
        nbt.insert("Items", NbtList::Compound(saved));
    }

    /// Reads the vanilla `ContainerEntity` save data.
    fn load(&mut self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.clear();
        self.loot_table = nbt
            .string("LootTable")
            .and_then(|value| value.to_string().parse::<Identifier>().ok());
        self.loot_table_seed = nbt.long("LootTableSeed").unwrap_or(0);
        if self.loot_table.is_some() {
            return;
        }

        let Some(compounds) = nbt.list("Items").and_then(|list| list.compounds()) else {
            return;
        };
        for compound in compounds {
            let Some(slot) = compound.byte("Slot") else {
                continue;
            };
            let slot = slot as usize;
            if slot >= CHEST_BOAT_SLOTS {
                continue;
            }
            if let Some(stack) = ItemStack::from_borrowed_compound(&compound) {
                self.set_item(slot, stack);
            }
        }
    }
}

impl Default for ChestBoatStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `AbstractBoat` behavior shared by all four concrete boat structs.
///
/// Implementors supply storage accessors, the two per-class geometry hooks
/// (`rideHeight` and `getSinglePassengerXOffset`) and the passenger capacity.
trait AbstractBoat: Entity + Leashable {
    /// Returns the unsynced boat state.
    fn boat_state(&self) -> &SyncMutex<BoatState>;

    /// Returns the synchronized `AbstractBoat` data.
    fn boat_data(&self) -> &SyncMutex<AbstractBoatEntityData>;

    /// Vanilla `rideHeight`: the passenger attachment height for this hull shape.
    fn ride_height(&self, dimensions: EntityDimensions) -> f64;

    /// Vanilla `getSinglePassengerXOffset`.
    fn single_passenger_x_offset(&self) -> f32;

    /// Vanilla `getMaxPassengers`.
    fn max_passengers(&self) -> usize;

    /// Current float status, vanilla `AbstractBoat.getStatus` as stored by the last tick.
    fn status(&self) -> BoatStatus {
        self.boat_state().lock().status
    }

    /// Vanilla `AbstractBoat.tick`.
    fn boat_tick(self: Arc<Self>) {
        {
            let mut state = self.boat_state().lock();
            state.old_status = state.status;
        }
        let status = self.compute_status();
        {
            let mut state = self.boat_state().lock();
            state.status = status;
            if status.is_under_water() {
                state.out_of_control_ticks += 1.0;
            } else {
                state.out_of_control_ticks = 0.0;
            }
        }

        if self.boat_state().lock().out_of_control_ticks >= TIME_TO_EJECT {
            self.eject_passengers();
        }

        {
            let mut data = self.boat_data().lock();
            let hurt_time = *data.vehicle_entity.id_hurt.get();
            if hurt_time > 0 {
                data.vehicle_entity.id_hurt.set(hurt_time - 1);
            }
            let damage = *data.vehicle_entity.id_damage.get();
            if damage > 0.0 {
                data.vehicle_entity.id_damage.set(damage - 1.0);
            }
        }

        self.default_tick();

        if !self
            .first_passenger()
            .is_some_and(|passenger| passenger.as_player().is_some())
        {
            self.set_paddle_state(false, false);
        }

        self.float_boat();
        if self.is_vehicle() {
            self.control_boat();
        }

        let velocity = self.velocity();
        let _ = Arc::clone(&self).move_entity(MoverType::SelfMovement, velocity);

        // Vanilla calls this twice so block effects see the pre- and post-move box.
        self.apply_effects_from_blocks();
        self.apply_effects_from_blocks();
        self.tick_bubble_column();
        self.tick_paddles();
        self.push_and_board_nearby_entities();
    }

    /// Vanilla `AbstractBoat.controlBoat`.
    ///
    /// Vanilla runs this on the client and streams paddle flags to the server. Steel is
    /// server-authoritative, so the server applies the same input here; the protocol lane
    /// fills the input flags from the paddle packet.
    fn control_boat(&self) {
        if !self.is_vehicle() {
            return;
        }

        let (input_left, input_right, input_up, input_down) = {
            let state = self.boat_state().lock();
            (
                state.input_left,
                state.input_right,
                state.input_up,
                state.input_down,
            )
        };
        let mut acceleration = 0.0_f32;
        let (yaw, pitch) = self.rotation();

        {
            let mut state = self.boat_state().lock();
            if input_left {
                state.delta_rotation -= 1.0;
            }
            if input_right {
                state.delta_rotation += 1.0;
            }
            if input_right != input_left && !input_up && !input_down {
                acceleration += 0.005;
            }
        }

        let delta_rotation = self.boat_state().lock().delta_rotation;
        self.set_rotation((yaw + delta_rotation, pitch));

        if input_up {
            acceleration += 0.04;
        }
        if input_down {
            acceleration -= 0.005;
        }

        let yaw = f64::from(self.rotation().0) * std::f64::consts::PI / 180.0;
        let velocity = self.velocity();
        self.set_velocity(
            velocity
                + DVec3::new(
                    -yaw.sin() * f64::from(acceleration),
                    0.0,
                    yaw.cos() * f64::from(acceleration),
                ),
        );

        self.set_paddle_state(
            input_right && !input_left || input_up,
            input_left && !input_right || input_up,
        );
    }

    /// Vanilla `AbstractBoat.getStatus`.
    fn compute_status(&self) -> BoatStatus {
        if let Some(underwater_status) = self.underwater_status() {
            let max_y = self.bounding_box().max_y();
            self.boat_state().lock().water_level = max_y;
            return underwater_status;
        }

        if self.check_in_water() {
            return BoatStatus::InWater;
        }

        let friction = self.ground_friction();
        if friction > 0.0 {
            self.boat_state().lock().land_friction = friction;
            BoatStatus::OnLand
        } else {
            BoatStatus::InAir
        }
    }

    /// Vanilla `AbstractBoat.isUnderwater`: `None` while the deck stays above water.
    fn underwater_status(&self) -> Option<BoatStatus> {
        let world = self.level()?;
        let aabb = self.bounding_box();
        let max_y = aabb.max_y() + 0.001;
        let min_x = aabb.min_x().floor() as i32;
        let max_x = aabb.max_x().ceil() as i32;
        let min_y = aabb.max_y().floor() as i32;
        let max_y_bound = max_y.ceil() as i32;
        let min_z = aabb.min_z().floor() as i32;
        let max_z = aabb.max_z().ceil() as i32;
        let mut under_water = false;

        for x in min_x..max_x {
            for y in min_y..max_y_bound {
                for z in min_z..max_z {
                    let pos = BlockPos::new(x, y, z);
                    let fluid_state = get_fluid_state(&world, pos);
                    if !fluid_state.is_water() {
                        continue;
                    }
                    let surface = f64::from(y) + f64::from(get_height(&world, pos, fluid_state));
                    if max_y < surface {
                        if !fluid_state.is_source() {
                            return Some(BoatStatus::UnderFlowingWater);
                        }
                        under_water = true;
                    }
                }
            }
        }

        under_water.then_some(BoatStatus::UnderWater)
    }

    /// Vanilla `AbstractBoat.checkInWater`.
    fn check_in_water(&self) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        let aabb = self.bounding_box();
        let min_x = aabb.min_x().floor() as i32;
        let max_x = aabb.max_x().ceil() as i32;
        let min_y = aabb.min_y().floor() as i32;
        let max_y = (aabb.min_y() + 0.001).ceil() as i32;
        let min_z = aabb.min_z().floor() as i32;
        let max_z = aabb.max_z().ceil() as i32;
        let mut in_water = false;
        let mut water_level = -f64::MAX;

        for x in min_x..max_x {
            for y in min_y..max_y {
                for z in min_z..max_z {
                    let pos = BlockPos::new(x, y, z);
                    let fluid_state = get_fluid_state(&world, pos);
                    if !fluid_state.is_water() {
                        continue;
                    }
                    let height = f64::from(y) + f64::from(get_height(&world, pos, fluid_state));
                    water_level = water_level.max(height);
                    in_water |= aabb.min_y() < height;
                }
            }
        }

        self.boat_state().lock().water_level = water_level;
        in_water
    }

    /// Vanilla `AbstractBoat.getWaterLevelAbove`.
    fn get_water_level_above(&self) -> f64 {
        let Some(world) = self.level() else {
            return 0.0;
        };
        let aabb = self.bounding_box();
        let last_yd = self.boat_state().lock().last_yd;
        let min_x = aabb.min_x().floor() as i32;
        let max_x = aabb.max_x().ceil() as i32;
        let min_y = aabb.max_y().floor() as i32;
        let max_y = (aabb.max_y() - last_yd).ceil() as i32;
        let min_z = aabb.min_z().floor() as i32;
        let max_z = aabb.max_z().ceil() as i32;

        'scan: for y in min_y..max_y {
            let mut block_height = 0.0_f32;

            for x in min_x..max_x {
                for z in min_z..max_z {
                    let pos = BlockPos::new(x, y, z);
                    let fluid_state = get_fluid_state(&world, pos);
                    if fluid_state.is_water() {
                        block_height = block_height.max(get_height(&world, pos, fluid_state));
                    }
                    if block_height >= 1.0 {
                        continue 'scan;
                    }
                }
            }

            if block_height < 1.0 {
                return f64::from(y) + f64::from(block_height);
            }
        }

        f64::from(max_y) + 1.0
    }

    /// Vanilla `AbstractBoat.getGroundFriction`.
    fn ground_friction(&self) -> f32 {
        let Some(world) = self.level() else {
            return 0.0;
        };
        let bb = self.bounding_box();
        let boat_box = WorldAabb::new(
            bb.min_x(),
            bb.min_y() - 0.001,
            bb.min_z(),
            bb.max_x(),
            bb.min_y(),
            bb.max_z(),
        );
        let x0 = boat_box.min_x().floor() as i32 - 1;
        let x1 = boat_box.max_x().ceil() as i32 + 1;
        let y0 = boat_box.min_y().floor() as i32 - 1;
        let y1 = boat_box.max_y().ceil() as i32 + 1;
        let z0 = boat_box.min_z().floor() as i32 - 1;
        let z1 = boat_box.max_z().ceil() as i32 + 1;
        let mut friction = 0.0_f32;
        let mut count = 0_i32;

        for x in x0..x1 {
            for z in z0..z1 {
                let edges = i32::from(x == x0 || x == x1 - 1) + i32::from(z == z0 || z == z1 - 1);
                if edges == 2 {
                    continue;
                }
                for y in y0..y1 {
                    if edges > 0 && y != y0 && y != y1 - 1 {
                        continue;
                    }

                    let pos = BlockPos::new(x, y, z);
                    let state = world.get_block_state(pos);
                    if state.get_block() == &vanilla_blocks::LILY_PAD {
                        continue;
                    }
                    let collides = state
                        .get_collision_shape_at(pos)
                        .iter()
                        .any(|local| join_is_not_empty(&local.at_block(pos), &boat_box));
                    if collides {
                        friction += state.get_block().config.friction;
                        count += 1;
                    }
                }
            }
        }

        if count == 0 {
            return 0.0;
        }
        friction / count as f32
    }

    /// Vanilla `AbstractBoat.floatBoat`.
    fn float_boat(&self) {
        let mut vertical_speed = -self.get_gravity();
        let mut buoyancy = 0.0_f64;
        let mut inverse_friction = 0.05_f32;
        let (old_status, status) = {
            let state = self.boat_state().lock();
            (state.old_status, state.status)
        };

        if old_status == BoatStatus::InAir
            && status != BoatStatus::InAir
            && status != BoatStatus::OnLand
        {
            let height = self.boat_height();
            self.boat_state().lock().water_level = self.position().y + height;
            let target_y = self.get_water_level_above() - height + 0.101;
            let moved =
                self.bounding_box()
                    .translate(DVec3::new(0.0, target_y - self.position().y, 0.0));
            if let Some(world) = self.level() {
                let collision_world = WorldCollisionProvider::new(&world);
                if !has_block_collision(&collision_world, moved) {
                    let position = self.position();
                    let _ = self.try_set_position(DVec3::new(position.x, target_y, position.z));
                    let velocity = self.velocity();
                    self.set_velocity(DVec3::new(velocity.x, 0.0, velocity.z));
                    self.boat_state().lock().last_yd = 0.0;
                }
            }

            self.boat_state().lock().status = BoatStatus::InWater;
        } else {
            match status {
                BoatStatus::InWater => {
                    let water_level = self.boat_state().lock().water_level;
                    buoyancy = (water_level - self.position().y) / self.boat_height();
                    inverse_friction = 0.9;
                }
                BoatStatus::UnderFlowingWater => {
                    vertical_speed = -7.0e-4;
                    inverse_friction = 0.9;
                }
                BoatStatus::UnderWater => {
                    buoyancy = 0.01;
                    inverse_friction = 0.45;
                }
                BoatStatus::InAir => inverse_friction = 0.9,
                BoatStatus::OnLand => {
                    let mut land_friction = self.boat_state().lock().land_friction;
                    inverse_friction = land_friction;
                    if self
                        .controlling_passenger()
                        .is_some_and(|passenger| passenger.as_player().is_some())
                    {
                        land_friction /= 2.0;
                        self.boat_state().lock().land_friction = land_friction;
                    }
                }
            }

            let velocity = self.velocity();
            self.set_velocity(DVec3::new(
                velocity.x * f64::from(inverse_friction),
                velocity.y + vertical_speed,
                velocity.z * f64::from(inverse_friction),
            ));
            self.boat_state().lock().delta_rotation *= inverse_friction;

            if buoyancy > 0.0 {
                let velocity = self.velocity();
                self.set_velocity(DVec3::new(
                    velocity.x,
                    (velocity.y + buoyancy * (self.get_default_gravity() / 0.65)) * 0.75,
                    velocity.z,
                ));
            }
        }
    }

    /// Vanilla `AbstractBoat.tickBubbleColumn`, server branch.
    fn tick_bubble_column(&self) {
        if !self.boat_state().lock().is_above_bubble_column {
            self.set_bubble_time(0);
        }

        let mut bubble_time = self.get_bubble_time();
        if bubble_time <= 0 {
            return;
        }

        bubble_time -= 1;
        self.set_bubble_time(bubble_time);
        let elapsed = BUBBLE_TIME - bubble_time - 1;
        if elapsed > 0 && bubble_time == 0 {
            self.set_bubble_time(0);
            let drag_down = self.boat_state().lock().bubble_column_direction_is_down;
            if drag_down {
                self.eject_passengers();
                self.broadcast_entity_event(steel_utils::entity_events::EntityStatus::BoatSink);
            } else {
                self.broadcast_entity_event(steel_utils::entity_events::EntityStatus::BoatLaunch);
            }
            self.handle_bubble_column_effect(drag_down);
        }

        self.boat_state().lock().is_above_bubble_column = false;
    }

    /// Vanilla `AbstractBoat.handleBubbleColumnEffect`.
    fn handle_bubble_column_effect(&self, drag_down: bool) {
        if !self.can_simulate_movement() {
            return;
        }

        let velocity = self.velocity();
        if drag_down {
            self.set_velocity(velocity + DVec3::new(0.0, -0.7, 0.0));
        } else {
            self.set_velocity(DVec3::new(velocity.x, 0.6, velocity.z));
        }
    }

    /// Vanilla `AbstractBoat.onAboveBubbleColumn`.
    fn boat_on_above_bubble_column(&self, drag_down: bool, _pos: BlockPos) {
        {
            let mut state = self.boat_state().lock();
            state.is_above_bubble_column = true;
            state.bubble_column_direction_is_down = drag_down;
        }
        if self.get_bubble_time() == 0 {
            self.set_bubble_time(BUBBLE_TIME);
        }

        if !self.is_under_water() && rand::random_range(0..100) == 0 {
            if let Some(world) = self.level() {
                let position = self.position();
                world.play_sound_at(
                    self.swim_sound(),
                    self.sound_source(),
                    position,
                    1.0,
                    0.8 + 0.4 * rand::random::<f32>(),
                    None,
                );
                world.send_particles(
                    ParticleData::simple(&SPLASH),
                    DVec3::new(
                        position.x + rand::random::<f64>(),
                        position.y + 0.7,
                        position.z + rand::random::<f64>(),
                    ),
                    1,
                    DVec3::ZERO,
                    0.0,
                );
            }
            let controlling = self.controlling_passenger();
            self.game_event_with_source_entity(
                &vanilla_game_events::SPLASH,
                controlling.as_deref(),
            );
        }
    }

    /// Advances paddle animation and plays the paddle sounds, vanilla `AbstractBoat.tick`.
    fn tick_paddles(&self) {
        let view = self.look_angle();
        for side in 0..2 {
            if !self.get_paddle_state(side) {
                self.boat_state().lock().paddle_positions[side] = 0.0;
                continue;
            }

            let (previous, next) = {
                let mut state = self.boat_state().lock();
                let previous = state.paddle_positions[side];
                let next = previous + PADDLE_SPEED;
                state.paddle_positions[side] = next;
                (previous, next)
            };

            let full_turn = std::f32::consts::TAU;
            let quarter = std::f32::consts::FRAC_PI_4;
            if previous % full_turn <= quarter
                && next % full_turn >= quarter
                && let Some(sound) = self.paddle_sound()
            {
                let position = self.position();
                let (dx, dz) = if side == 1 {
                    (-view.z, view.x)
                } else {
                    (view.z, -view.x)
                };
                if let Some(world) = self.level() {
                    world.play_sound_at(
                        sound,
                        self.sound_source(),
                        DVec3::new(position.x + dx, position.y, position.z + dz),
                        1.0,
                        0.8 + 0.4 * rand::random::<f32>(),
                        None,
                    );
                }
            }
        }
    }

    /// Vanilla `AbstractBoat.getPaddleSound`.
    fn paddle_sound(&self) -> Option<SoundEventRef> {
        match self.status() {
            BoatStatus::InWater | BoatStatus::UnderWater | BoatStatus::UnderFlowingWater => {
                Some(&sound_events::ENTITY_BOAT_PADDLE_WATER)
            }
            BoatStatus::OnLand => Some(&sound_events::ENTITY_BOAT_PADDLE_LAND),
            BoatStatus::InAir => None,
        }
    }

    /// Vanilla `AbstractBoat.tick`'s trailing boarding and pushing scan.
    fn push_and_board_nearby_entities(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let area = self.bounding_box().inflate_xyz(0.2, -0.01, 0.2);
        let candidates = world.get_entities_in_aabb(&area);
        if candidates.is_empty() {
            return;
        }

        let can_board = !self
            .controlling_passenger()
            .is_some_and(|passenger| passenger.as_player().is_some());
        let own_box = self.bounding_box();
        let own_width = own_box.max_x() - own_box.min_x();

        for entity in candidates {
            // `EntitySelector.pushableBy`: spectators and non-pushable entities are
            // excluded before either the boarding or the pushing branch.
            if entity.is_spectator() || !entity.is_pushable() {
                continue;
            }
            if entity.has_passenger(self.as_entity_event_source()) {
                continue;
            }

            let other_box = entity.bounding_box();
            let boardable = can_board
                && self.passengers().len() < self.max_passengers()
                && !entity.is_passenger()
                && other_box.max_x() - other_box.min_x() < own_width
                && entity.as_living_entity().is_some()
                && !REGISTRY.entity_types.is_in_tag(
                    entity.entity_type(),
                    &EntityTypeTag::CANNOT_BE_PUSHED_ONTO_BOATS,
                );

            if boardable {
                if let Some(vehicle) = world.get_entity_by_id(self.id()) {
                    let _ = entity.start_riding(&vehicle);
                }
                continue;
            }

            // Vanilla `AbstractBoat.push` gate, applied here because the shared
            // `Entity::push_entity` default cannot be reached from an override.
            let should_push = if entity.entity_type().is_abstract_boat {
                other_box.min_y() < own_box.max_y()
            } else {
                other_box.min_y() <= own_box.min_y()
            };
            if should_push {
                self.push_entity(entity.as_ref());
            }
        }
    }

    /// Vanilla `AbstractBoat.getPassengerAttachmentPoint`.
    fn boat_passenger_attachment(&self, passenger: &dyn Entity) -> DVec3 {
        let dimensions = self.base().dimensions();
        let mut offset = f64::from(self.single_passenger_x_offset());
        let passengers = self.passengers();
        if passengers.len() > 1 {
            let index = passengers
                .iter()
                .position(|candidate| candidate.id() == passenger.id())
                .unwrap_or_default();
            offset = if index == 0 { 0.2 } else { -0.6 };
            if passenger.as_animal().is_some() {
                offset += 0.2;
            }
        }

        let angle = -f64::from(self.rotation().0) * std::f64::consts::PI / 180.0;
        DVec3::new(
            offset * angle.sin(),
            self.ride_height(dimensions),
            offset * angle.cos(),
        )
    }

    /// Vanilla `VehicleEntity.hurtServer`.
    fn hurt_vehicle(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_removed() {
            return true;
        }
        if self.is_invulnerable_to_base(source) {
            return false;
        }

        let damage = {
            let mut data = self.boat_data().lock();
            let hurt_dir = *data.vehicle_entity.id_hurtdir.get();
            data.vehicle_entity.id_hurtdir.set(-hurt_dir);
            data.vehicle_entity.id_hurt.set(10);
            let damage = *data.vehicle_entity.id_damage.get() + amount * 10.0;
            data.vehicle_entity.id_damage.set(damage);
            damage
        };

        self.mark_hurt();
        let causing_entity = source.causing_entity().map(|entity| &**entity);
        self.game_event_with_source_entity(&vanilla_game_events::ENTITY_DAMAGE, causing_entity);

        let creative_player = source.is_creative_player();
        if (creative_player || damage <= 40.0) && !self.should_source_destroy(source) {
            if creative_player {
                self.set_removed(RemovalReason::Discarded);
            }
        } else {
            self.destroy_vehicle(world, source);
        }

        true
    }

    /// Vanilla `VehicleEntity.shouldSourceDestroy`; boat sources never destroy directly.
    fn should_source_destroy(&self, _source: &DamageSource) -> bool {
        false
    }

    /// Vanilla `VehicleEntity.destroy(ServerLevel, Item)`.
    fn destroy_vehicle(&self, world: &World, source: &DamageSource) {
        self.destroy_hull(world, source);
    }

    /// Drops the hull item without touching chest contents.
    ///
    /// Chest boats override `destroy_vehicle` to call this and then empty their storage.
    fn destroy_hull(&self, world: &World, _source: &DamageSource) {
        self.kill(world);
        if !world.get_game_rule(&ENTITY_DROPS) {
            return;
        }

        let Some(item) = REGISTRY.items.by_key(&self.entity_type().key) else {
            return;
        };
        let mut stack = ItemStack::new(item);
        if let Some(name) = self.custom_name() {
            stack.set(CUSTOM_NAME, name);
        }
        let _ = self.spawn_at_location(stack, 0.0);
    }

    /// Vanilla `AbstractBoat.checkFallDamage`; boats accumulate but never take fall damage.
    fn boat_check_fall_damage(&self, vertical_movement: f64, on_ground: bool, world: &Arc<World>) {
        let velocity = self.velocity();
        self.boat_state().lock().last_yd = velocity.y;
        if self.is_passenger() {
            return;
        }

        if on_ground {
            self.reset_fall_distance();
        } else if vertical_movement < 0.0
            && !world
                .get_block_state(self.block_position().below())
                .get_fluid_state()
                .is_water()
        {
            self.base().accumulate_fall_distance(vertical_movement);
        }
    }

    /// Vanilla `AbstractBoat.interact`: mount the interacting player.
    fn boat_interact(&self, player: &Player) -> InteractionResult {
        if player.is_secondary_use_active() {
            return InteractionResult::Pass;
        }
        if self.boat_state().lock().out_of_control_ticks >= TIME_TO_EJECT {
            return InteractionResult::Pass;
        }

        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        let Some(vehicle) = world.get_entity_by_id(self.id()) else {
            return InteractionResult::Pass;
        };

        if player.start_riding(&vehicle) {
            InteractionResult::Success
        } else {
            InteractionResult::Pass
        }
    }

    /// Drops every passenger, vanilla `Entity.ejectPassengers`.
    fn eject_passengers(&self) {
        for passenger in self.passengers() {
            passenger.stop_riding();
        }
    }

    /// Returns the synced paddle flag for `side` (0 = left, 1 = right).
    ///
    /// Mirrors vanilla `getPaddleState`, which also requires a controlling passenger.
    fn get_paddle_state(&self, side: usize) -> bool {
        let data = self.boat_data().lock();
        let pressed = if side == 0 {
            *data.id_paddle_left.get()
        } else {
            *data.id_paddle_right.get()
        };
        pressed && self.controlling_passenger().is_some()
    }

    /// Sets the synced paddle flags, vanilla `setPaddleState`.
    fn set_paddle_state(&self, left: bool, right: bool) {
        let mut data = self.boat_data().lock();
        data.id_paddle_left.set(left);
        data.id_paddle_right.set(right);
    }

    /// Returns the synced bubble-column timer.
    fn get_bubble_time(&self) -> i32 {
        *self.boat_data().lock().id_bubble_time.get()
    }

    /// Sets the synced bubble-column timer.
    fn set_bubble_time(&self, value: i32) {
        self.boat_data().lock().id_bubble_time.set(value);
    }

    /// Returns this hull's height in world units, vanilla `getBbHeight`.
    fn boat_height(&self) -> f64 {
        f64::from(self.base().dimensions().height)
    }
}

/// Generates the shared construction, downcast key and `Entity` impl for a boat struct.
macro_rules! impl_boat_entity {
    ($struct_name:ident, $type_key:literal) => {
        // SAFETY: This key is owned by Steel and uniquely identifies the boat struct.
        unsafe impl DowncastType for $struct_name {
            const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new($type_key);
        }

        impl $struct_name {
            /// Creates a new boat entity.
            #[must_use]
            pub fn new(
                entity_type: EntityTypeRef,
                id: i32,
                position: DVec3,
                world: Weak<World>,
            ) -> Self {
                Self::from_parts(
                    EntityBase::new(id, position, entity_type.dimensions, world),
                    entity_type,
                )
            }

            /// Creates a boat entity from saved data.
            #[must_use]
            pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
                Self::from_parts(
                    EntityBase::from_load(load, entity_type.dimensions),
                    entity_type,
                )
            }
        }

        impl Entity for $struct_name {
            fn base(&self) -> &EntityBase {
                &self.base
            }

            fn entity_type(&self) -> EntityTypeRef {
                self.entity_type
            }

            fn tick(self: Arc<Self>) {
                self.boat_tick();
            }

            fn is_pickable(&self) -> bool {
                !self.is_removed()
            }

            fn is_pushable(&self) -> bool {
                true
            }

            fn blocks_building(&self) -> bool {
                true
            }

            fn can_be_collided_with(&self, _other: Option<&dyn Entity>) -> bool {
                true
            }

            fn dimension_changing_delay(&self) -> i32 {
                10
            }

            fn get_relative_portal_position(
                &self,
                axis: steel_utils::axis::Axis,
                portal_area: steel_utils::block_util::FoundRectangle,
            ) -> DVec3 {
                reset_forward_direction_of_relative_portal_position(
                    crate::portal::portal_shape::PortalShape::get_relative_position(
                        portal_area,
                        axis,
                        self.position(),
                        self.dimensions_for_pose(self.pose()),
                    ),
                )
            }

            fn get_default_gravity(&self) -> f64 {
                BOAT_GRAVITY
            }

            fn get_air_drag(&self) -> f64 {
                BOAT_AIR_DRAG
            }

            fn movement_emission(&self) -> EntityMovementEmission {
                EntityMovementEmission::Events
            }

            fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
                Some(&self.entity_data)
            }

            fn save_additional(&self, nbt: &mut NbtCompound) {
                self.save_boat_data(nbt);
            }

            fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
                self.load_boat_data(nbt);
            }

            fn is_under_water(&self) -> bool {
                self.status().is_under_water()
            }

            fn passenger_attachment_point(&self, passenger: &dyn Entity) -> DVec3 {
                self.boat_passenger_attachment(passenger)
            }

            fn can_add_passenger(&self, _passenger: &dyn Entity) -> bool {
                self.passengers().len() < self.max_passengers() && !self.is_eye_in_water()
            }

            fn controlling_passenger(&self) -> Option<SharedEntity> {
                self.first_passenger()
                    .filter(|passenger| passenger.as_player().is_some())
            }

            fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
                self.hurt_vehicle(world, source, amount)
            }

            fn check_fall_damage(
                self: Arc<Self>,
                vertical_movement: f64,
                on_ground: bool,
                _on_state: BlockStateId,
                _pos: BlockPos,
                world: &Arc<World>,
            ) {
                self.boat_check_fall_damage(vertical_movement, on_ground, world);
            }

            fn on_above_bubble_column(&self, drag_down: bool, pos: BlockPos) {
                self.boat_on_above_bubble_column(drag_down, pos);
            }

            fn interact(
                &self,
                player: &Player,
                _hand: InteractionHand,
                _location: DVec3,
            ) -> InteractionResult {
                self.boat_interact(player)
            }

            fn as_leashable(&self) -> Option<&dyn Leashable> {
                Some(self)
            }
        }
    };
}

/// Generates the public boat API shared by all four structs.
macro_rules! impl_boat_public_api {
    ($struct_name:ident) => {
        impl $struct_name {
            /// Sets the position and resets interpolation anchors, vanilla `setInitialPos`.
            pub fn set_initial_pos(&self, position: DVec3) {
                let _ = self.try_set_position(position);
                self.set_old_position(position);
            }

            /// Applies paddle input, vanilla `setInput`.
            ///
            /// The protocol lane feeds this from `ServerboundPaddleBoatPacket`.
            pub fn set_input(&self, left: bool, right: bool, up: bool, down: bool) {
                let mut state = self.boat.lock();
                state.input_left = left;
                state.input_right = right;
                state.input_up = up;
                state.input_down = down;
            }

            /// Returns this boat's current float status.
            #[must_use]
            pub fn status(&self) -> BoatStatus {
                AbstractBoat::status(self)
            }

            /// Sets the synced paddle flags, vanilla `setPaddleState`.
            pub fn set_paddle_state(&self, left: bool, right: bool) {
                AbstractBoat::set_paddle_state(self, left, right);
            }

            /// Returns the synced paddle flag for `side` (0 = left, 1 = right).
            #[must_use]
            pub fn get_paddle_state(&self, side: usize) -> bool {
                AbstractBoat::get_paddle_state(self, side)
            }
        }
    };
}

/// Generates the chest-boat API and storage hooks for a chest-boat struct.
macro_rules! impl_chest_boat {
    ($struct_name:ident) => {
        impl $struct_name {
            /// Returns the chest storage and deferred loot state.
            #[must_use]
            pub fn chest(&self) -> &SyncMutex<ChestBoatStorage> {
                &self.chest
            }

            /// Sets the deferred loot table used when the container is first opened.
            pub fn set_loot_table(&self, loot_table: Identifier, seed: i64) {
                self.chest.lock().set_loot_table(loot_table, seed);
            }

            /// Vanilla `ContainerEntity.isChestVehicleStillValid`.
            #[must_use]
            pub fn is_chest_still_valid(&self, player: &Player) -> bool {
                !self.is_removed()
                    && player.is_within_entity_interaction_range(self.bounding_box(), 4.0)
            }
        }

        impl DropChestContents for $struct_name {
            fn drop_chest_contents(&self) {
                let stacks = {
                    let mut chest = self.chest.lock();
                    let stacks = chest.items().to_vec();
                    chest.clear();
                    stacks
                };
                for stack in stacks {
                    if !stack.is_empty() {
                        let _ = self.spawn_at_location(stack, 0.0);
                    }
                }
            }
        }
    };
}

/// Boat entity, vanilla `Boat`: a two-seat wood hull.
#[entity_behavior(class = "Boat")]
pub struct BoatEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<AbstractBoatEntityData>,
    boat: SyncMutex<BoatState>,
    leash_data: SyncMutex<Option<LeashData>>,
}

impl BoatEntity {
    fn from_parts(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat: SyncMutex::new(BoatState::new()),
            leash_data: SyncMutex::new(None),
        }
    }

    /// Writes vanilla `AbstractBoat` save data (leash data only).
    fn save_boat_data(&self, nbt: &mut NbtCompound) {
        if let Some(leash) = self.leash_data.lock().as_ref() {
            leash.save(nbt);
        }
    }

    /// Reads vanilla `AbstractBoat` save data (leash data only).
    fn load_boat_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        *self.leash_data.lock() = LeashData::load(nbt);
    }
}

impl AbstractBoat for BoatEntity {
    fn boat_state(&self) -> &SyncMutex<BoatState> {
        &self.boat
    }

    fn boat_data(&self) -> &SyncMutex<AbstractBoatEntityData> {
        &self.entity_data
    }

    fn ride_height(&self, dimensions: EntityDimensions) -> f64 {
        f64::from(dimensions.height) / 3.0
    }

    fn single_passenger_x_offset(&self) -> f32 {
        0.0
    }

    fn max_passengers(&self) -> usize {
        2
    }
}

impl Leashable for BoatEntity {
    fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        &self.leash_data
    }
}

/// Raft entity, vanilla `Raft` (bamboo): a two-seat flat hull with a taller ride height.
#[entity_behavior(class = "Raft")]
pub struct RaftEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<AbstractBoatEntityData>,
    boat: SyncMutex<BoatState>,
    leash_data: SyncMutex<Option<LeashData>>,
}

impl RaftEntity {
    fn from_parts(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat: SyncMutex::new(BoatState::new()),
            leash_data: SyncMutex::new(None),
        }
    }

    /// Writes vanilla `AbstractBoat` save data (leash data only).
    fn save_boat_data(&self, nbt: &mut NbtCompound) {
        if let Some(leash) = self.leash_data.lock().as_ref() {
            leash.save(nbt);
        }
    }

    /// Reads vanilla `AbstractBoat` save data (leash data only).
    fn load_boat_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        *self.leash_data.lock() = LeashData::load(nbt);
    }
}

impl AbstractBoat for RaftEntity {
    fn boat_state(&self) -> &SyncMutex<BoatState> {
        &self.boat
    }

    fn boat_data(&self) -> &SyncMutex<AbstractBoatEntityData> {
        &self.entity_data
    }

    fn ride_height(&self, dimensions: EntityDimensions) -> f64 {
        f64::from(dimensions.height) * 0.888_888_9
    }

    fn single_passenger_x_offset(&self) -> f32 {
        0.0
    }

    fn max_passengers(&self) -> usize {
        2
    }
}

impl Leashable for RaftEntity {
    fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        &self.leash_data
    }
}

/// Chest boat entity, vanilla `ChestBoat`: a single-seat wood hull with a 27-slot chest.
#[entity_behavior(class = "ChestBoat")]
pub struct ChestBoatEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<AbstractBoatEntityData>,
    boat: SyncMutex<BoatState>,
    leash_data: SyncMutex<Option<LeashData>>,
    chest: SyncMutex<ChestBoatStorage>,
}

impl ChestBoatEntity {
    fn from_parts(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat: SyncMutex::new(BoatState::new()),
            leash_data: SyncMutex::new(None),
            chest: SyncMutex::new(ChestBoatStorage::new()),
        }
    }

    /// Writes vanilla `AbstractChestBoat` save data (leash plus chest storage).
    fn save_boat_data(&self, nbt: &mut NbtCompound) {
        if let Some(leash) = self.leash_data.lock().as_ref() {
            leash.save(nbt);
        }
        self.chest.lock().save(nbt);
    }

    /// Reads vanilla `AbstractChestBoat` save data (leash plus chest storage).
    fn load_boat_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        *self.leash_data.lock() = LeashData::load(nbt);
        self.chest.lock().load(nbt);
    }
}

impl AbstractBoat for ChestBoatEntity {
    fn boat_state(&self) -> &SyncMutex<BoatState> {
        &self.boat
    }

    fn boat_data(&self) -> &SyncMutex<AbstractBoatEntityData> {
        &self.entity_data
    }

    fn ride_height(&self, dimensions: EntityDimensions) -> f64 {
        f64::from(dimensions.height) / 3.0
    }

    fn single_passenger_x_offset(&self) -> f32 {
        0.15
    }

    fn max_passengers(&self) -> usize {
        1
    }

    /// Vanilla `AbstractChestBoat.destroy`: drop the boat, then the chest contents.
    fn destroy_vehicle(&self, world: &World, source: &DamageSource) {
        self.destroy_hull(world, source);
        self.drop_chest_contents();
    }
}

impl Leashable for ChestBoatEntity {
    fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        &self.leash_data
    }
}

/// Chest raft entity, vanilla `ChestRaft`: a single-seat bamboo hull with a 27-slot chest.
#[entity_behavior(class = "ChestRaft")]
pub struct ChestRaftEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<AbstractBoatEntityData>,
    boat: SyncMutex<BoatState>,
    leash_data: SyncMutex<Option<LeashData>>,
    chest: SyncMutex<ChestBoatStorage>,
}

impl ChestRaftEntity {
    fn from_parts(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(AbstractBoatEntityData::new()),
            boat: SyncMutex::new(BoatState::new()),
            leash_data: SyncMutex::new(None),
            chest: SyncMutex::new(ChestBoatStorage::new()),
        }
    }

    /// Writes vanilla `AbstractChestBoat` save data (leash plus chest storage).
    fn save_boat_data(&self, nbt: &mut NbtCompound) {
        if let Some(leash) = self.leash_data.lock().as_ref() {
            leash.save(nbt);
        }
        self.chest.lock().save(nbt);
    }

    /// Reads vanilla `AbstractChestBoat` save data (leash plus chest storage).
    fn load_boat_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        *self.leash_data.lock() = LeashData::load(nbt);
        self.chest.lock().load(nbt);
    }
}

impl AbstractBoat for ChestRaftEntity {
    fn boat_state(&self) -> &SyncMutex<BoatState> {
        &self.boat
    }

    fn boat_data(&self) -> &SyncMutex<AbstractBoatEntityData> {
        &self.entity_data
    }

    fn ride_height(&self, dimensions: EntityDimensions) -> f64 {
        f64::from(dimensions.height) * 0.888_888_9
    }

    fn single_passenger_x_offset(&self) -> f32 {
        0.15
    }

    fn max_passengers(&self) -> usize {
        1
    }

    /// Vanilla `AbstractChestBoat.destroy`: drop the raft, then the chest contents.
    fn destroy_vehicle(&self, world: &World, source: &DamageSource) {
        self.destroy_hull(world, source);
        self.drop_chest_contents();
    }
}

impl Leashable for ChestRaftEntity {
    fn leash_data(&self) -> &SyncMutex<Option<LeashData>> {
        &self.leash_data
    }
}

/// Chest content drops shared by the two chest-boat structs, vanilla
/// `Containers.dropContents`.
trait DropChestContents {
    /// Spawns every occupied slot as a dropped item and empties the chest.
    fn drop_chest_contents(&self);
}

impl_boat_entity!(BoatEntity, "steel:entity/boat");
impl_boat_entity!(RaftEntity, "steel:entity/raft");
impl_boat_entity!(ChestBoatEntity, "steel:entity/chest_boat");
impl_boat_entity!(ChestRaftEntity, "steel:entity/chest_raft");

impl_boat_public_api!(BoatEntity);
impl_boat_public_api!(RaftEntity);
impl_boat_public_api!(ChestBoatEntity);
impl_boat_public_api!(ChestRaftEntity);

impl_chest_boat!(ChestBoatEntity);
impl_chest_boat!(ChestRaftEntity);

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use glam::DVec3;
    use simdnbt::borrow::read_compound as read_borrowed_compound;
    use simdnbt::owned::NbtCompound;
    use steel_registry::init_vanilla_registry;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::vanilla_entities;
    use steel_registry::vanilla_items;
    use steel_utils::Identifier;

    use crate::entity::Entity;
    use crate::world::World;

    use super::{
        AbstractBoat as _, BoatEntity, BoatStatus, ChestBoatEntity, ChestBoatStorage,
        ChestRaftEntity, RaftEntity,
    };

    fn boat() -> BoatEntity {
        BoatEntity::new(
            &vanilla_entities::OAK_BOAT,
            1,
            DVec3::ZERO,
            Weak::<World>::new(),
        )
    }

    #[test]
    fn boat_starts_airborne_with_vanilla_gravity() {
        let boat = boat();
        assert_eq!(boat.status(), BoatStatus::InAir);
        assert!((boat.get_default_gravity() - 0.04).abs() < f64::EPSILON);
        assert_eq!(boat.get_air_drag(), 1.0);
    }

    #[test]
    fn boat_paddle_flags_are_synced_but_need_a_controller() {
        let boat = boat();
        boat.set_paddle_state(true, false);

        // Without a controlling passenger vanilla reports both paddles idle.
        assert!(!boat.get_paddle_state(0));
        assert!(!boat.get_paddle_state(1));

        let data = boat.entity_data.lock();
        assert!(*data.id_paddle_left.get());
        assert!(!*data.id_paddle_right.get());
        assert_eq!(*data.id_bubble_time.get(), 0);
    }

    #[test]
    fn boat_status_under_water_matches_vanilla_predicate() {
        assert!(BoatStatus::UnderWater.is_under_water());
        assert!(BoatStatus::UnderFlowingWater.is_under_water());
        assert!(!BoatStatus::InWater.is_under_water());
        assert!(!BoatStatus::OnLand.is_under_water());
        assert!(!BoatStatus::InAir.is_under_water());
    }

    #[test]
    fn raft_ride_height_matches_vanilla_factor() {
        let raft = RaftEntity::new(
            &vanilla_entities::BAMBOO_RAFT,
            2,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        let dimensions = raft.base().dimensions();
        let height = f64::from(dimensions.height);
        assert!((raft.ride_height(dimensions) - height * 0.888_888_9).abs() < 1.0e-9);
        assert_eq!(raft.max_passengers(), 2);
        assert_eq!(raft.single_passenger_x_offset(), 0.0);
    }

    #[test]
    fn chest_boat_limits_seating_and_offsets_the_single_passenger() {
        let boat = ChestBoatEntity::new(
            &vanilla_entities::OAK_CHEST_BOAT,
            3,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        assert_eq!(boat.max_passengers(), 1);
        assert_eq!(boat.single_passenger_x_offset(), 0.15);
        assert_eq!(boat.chest().lock().items().len(), 27);
        assert!(boat.chest().lock().is_empty());
    }

    #[test]
    fn chest_storage_round_trips_slot_items() {
        init_vanilla_registry();
        let boat = ChestBoatEntity::new(
            &vanilla_entities::OAK_CHEST_BOAT,
            4,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        {
            let mut chest = boat.chest().lock();
            chest.set_item(3, ItemStack::with_count(&vanilla_items::OAK_BOAT, 5));
            chest.set_item(26, ItemStack::with_count(&vanilla_items::STICK, 2));
        }

        let mut nbt = NbtCompound::new();
        boat.save_additional(&mut nbt);
        assert!(nbt.list("Items").is_some());

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let view = read_borrowed_compound(&mut cursor)
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
        let mut restored = ChestBoatStorage::new();
        restored.load((&view).into());

        assert_eq!(restored.get_item(3).map(ItemStack::count), Some(5));
        assert_eq!(restored.get_item(26).map(ItemStack::count), Some(2));
        assert!(restored.get_item(0).is_some_and(ItemStack::is_empty));
        assert_eq!(restored.loot_table(), None);
    }

    #[test]
    fn chest_storage_saves_loot_table_instead_of_items() {
        let raft = ChestRaftEntity::new(
            &vanilla_entities::BAMBOO_CHEST_RAFT,
            5,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        {
            let mut chest = raft.chest().lock();
            chest.set_item(0, ItemStack::with_count(&vanilla_items::OAK_BOAT, 1));
            chest.set_loot_table(
                Identifier::new_static("minecraft", "chests/stronghold_library"),
                9,
            );
        }

        let mut nbt = NbtCompound::new();
        raft.save_additional(&mut nbt);

        assert_eq!(
            nbt.string("LootTable").map(ToString::to_string),
            Some("minecraft:chests/stronghold_library".to_owned())
        );
        assert_eq!(nbt.long("LootTableSeed"), Some(9));
        assert!(nbt.list("Items").is_none());

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let view = read_borrowed_compound(&mut cursor)
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
        let mut restored = ChestBoatStorage::new();
        restored.load((&view).into());
        assert_eq!(
            restored.loot_table().map(ToString::to_string),
            Some("minecraft:chests/stronghold_library".to_owned())
        );
        assert_eq!(restored.loot_table_seed(), 9);
        assert!(restored.is_empty());
    }

    #[test]
    fn boat_save_data_has_no_leash_or_chest_tags_by_default() {
        let boat = boat();
        let mut nbt = NbtCompound::new();
        boat.save_additional(&mut nbt);

        assert!(nbt.string("leash").is_none());
        assert!(nbt.list("Items").is_none());

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let view = read_borrowed_compound(&mut cursor)
            .unwrap_or_else(|error| panic!("test nbt should reborrow: {error}"));
        boat.load_additional((&view).into());
        assert!(boat.leash_data.lock().is_none());
    }

    #[test]
    fn boat_variants_keep_their_own_entity_type() {
        let oak = boat();
        let spruce = BoatEntity::new(
            &vanilla_entities::SPRUCE_BOAT,
            6,
            DVec3::ZERO,
            Weak::<World>::new(),
        );
        assert_eq!(oak.entity_type().key.to_string(), "minecraft:oak_boat");
        assert_eq!(
            spruce.entity_type().key.to_string(),
            "minecraft:spruce_boat"
        );
    }
}
