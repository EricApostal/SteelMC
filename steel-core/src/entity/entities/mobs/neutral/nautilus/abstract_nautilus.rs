//! Vanilla `AbstractNautilus` (`world/entity/animal/nautilus/AbstractNautilus.java`):
//! the shared tameable, saddled, rider-jumping and inventory-carrying water mount
//! layer of [`NautilusEntity`](super::NautilusEntity) and
//! [`ZombieNautilusEntity`](super::ZombieNautilusEntity).
//!
//! Vanilla models both nautili as subclasses of one
//! `AbstractNautilus extends TamableAnimal implements PlayerRideableJumping,
//! HasCustomInventoryScreen`. Steel has no inheritance, so the vanilla
//! non-synchronized state lives in [`AbstractNautilusParts`] (composition) and the
//! shared behaviour in the free functions below. [`Nautilus`] is the accessor
//! contract those functions need from a concrete nautilus; the concrete entity
//! files implement it next to their own trait impls.
//!
//! ## Deviations
//!
//! - Vanilla `SmoothSwimmingMoveControl`/`SmoothSwimmingLookControl` and
//!   `WaterBoundPathNavigation` are not ported; Steel's shared move/look controls
//!   and navigation are used with vanilla's `WATER` pathfinding malus of 0.
//! - `Entity.getSlot(int)`/`SlotAccess` does not exist in Steel, so the
//!   [`INVENTORY_SLOT_OFFSET`] container window is reachable only through the mount
//!   menu; the inventory itself is persisted in full.
//! - `Entity.isLocalInstanceAuthoritative()` has no Steel counterpart: the server
//!   entity is the authoritative instance, so the guard in `tickRidden` is
//!   trivially true.
//! - `PlayerRideableJumping` has no Steel trait or packet hook yet (the
//!   `StartRidingJump`/`StopRidingJump` player-command actions are still TODO in
//!   `player/movement`), so [`on_player_jump`], [`handle_start_jump`] and
//!   [`handle_stop_jump`] are the complete entity-side implementations for the lane
//!   that wires those packets.
//! - Vanilla `Entity.onSyncedDataUpdated(DASH)` has no Steel hook; the
//!   `!firstTick` cooldown rule is [`on_dash_synced`], called from the local setter
//!   ([`handle_start_jump`]) that can observe a zero cooldown.

use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::components::FoodProperties;
use steel_registry::data_components::vanilla_components;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::ParticleData;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_blocks,
    vanilla_game_events, vanilla_items, vanilla_mob_effects, vanilla_particle_types,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::{IntoShared, Shared, SyncMutex};
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, UuidExt as _};
use uuid::Uuid;

use crate::behavior::{ITEM_BEHAVIORS, InteractionResult};
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::ai::goal::Tameable;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    AgeableMob, Animal, Entity, EntitySpawnReason, LivingEntity, Mob, MobEffectInstance,
    SharedEntity,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::kinds::{get_inventory_size, mount_inventory_menu};
use crate::physics::{MoveResult, MoverType};
use crate::player::Player;
use crate::player::advancement::triggers::raise_tame_animal;
use crate::player::player_inventory::Prediction;
use crate::world::World;
use steel_protocol::packets::game::RandomizationType;

/// Vanilla `AbstractNautilus.INVENTORY_SLOT_OFFSET`.
pub const INVENTORY_SLOT_OFFSET: i32 = 500;
/// Vanilla `AbstractNautilus.INVENTORY_ROWS`.
pub const INVENTORY_ROWS: i32 = 3;
/// Vanilla `AbstractNautilus.SMALL_RESTRICTION_RADIUS`.
pub const SMALL_RESTRICTION_RADIUS: i32 = 16;
/// Vanilla `AbstractNautilus.LARGE_RESTRICTION_RADIUS`.
pub const LARGE_RESTRICTION_RADIUS: i32 = 32;
/// Vanilla `AbstractNautilus.RESTRICTION_RADIUS_BUFFER`.
pub const RESTRICTION_RADIUS_BUFFER: i32 = 8;
/// Vanilla `AbstractNautilus.EFFECT_DURATION`.
const EFFECT_DURATION: i32 = 60;
/// Vanilla `AbstractNautilus.EFFECT_REFRESH_RATE`.
const EFFECT_REFRESH_RATE: i64 = 40;
/// Vanilla `AbstractNautilus.NAUTILUS_WATER_RESISTANCE`.
const NAUTILUS_WATER_RESISTANCE: f64 = 0.9;
/// Vanilla `AbstractNautilus.RIDDEN_SPEED_MODIFIER_IN_WATER`.
const RIDDEN_SPEED_MODIFIER_IN_WATER: f32 = 0.0325;
/// Vanilla `AbstractNautilus.RIDDEN_SPEED_MODIFIER_ON_LAND`.
const RIDDEN_SPEED_MODIFIER_ON_LAND: f32 = 0.02;
/// Vanilla `AbstractNautilus.DASH_COOLDOWN_TICKS`.
pub const DASH_COOLDOWN_TICKS: i32 = 40;
/// Vanilla `AbstractNautilus.DASH_MINIMUM_DURATION_TICKS`.
const DASH_MINIMUM_DURATION_TICKS: i32 = 5;
/// Vanilla `AbstractNautilus.DASH_MOMENTUM_IN_WATER`.
const DASH_MOMENTUM_IN_WATER: f32 = 1.2;
/// Vanilla `AbstractNautilus.DASH_MOMENTUM_ON_LAND`.
const DASH_MOMENTUM_ON_LAND: f32 = 0.5;
/// Vanilla `AbstractNautilus.BUBBLE_SPREAD_FACTOR`.
const BUBBLE_SPREAD_FACTOR: f64 = 0.8;
/// Vanilla `AbstractNautilus.BUBBLE_DIRECTION_SCALE`.
const BUBBLE_DIRECTION_SCALE: f64 = 1.1;
/// Vanilla `AbstractNautilus.BUBBLE_Y_OFFSET`.
const BUBBLE_Y_OFFSET: f64 = 0.25;
/// Vanilla `AbstractNautilus.BUBBLE_PROBABILITY_MULTIPLIER`.
const BUBBLE_PROBABILITY_MULTIPLIER: f64 = 2.0;
/// Vanilla `AbstractNautilus.BUBBLE_PROBABILITY_MIN`.
const BUBBLE_PROBABILITY_MIN: f64 = 0.15;
/// Vanilla `AbstractNautilus.BUBBLE_PROBABILITY_MAX`.
const BUBBLE_PROBABILITY_MAX: f64 = 1.0;
/// Vanilla `AbstractNautilus.tryToTame` roll: `nextInt(3) == 0`.
const TAMING_ROLL_BOUND: i32 = 3;
/// Vanilla `TamableAnimal.feed` default heal when the stack has no food component.
const FEED_DEFAULT_HEAL: f32 = 1.0;
/// Vanilla `TamableAnimal.feed` healing factor.
const FEED_HEALING_FACTOR: f32 = 2.0;

/// Vanilla `AbstractNautilus` non-synchronized runtime state.
///
/// The synchronized `dash` flag and the `TamableAnimal` state live in the
/// generated entity-data layer; everything else vanilla keeps in plain fields is
/// collected here.
pub struct AbstractNautilusParts {
    /// Vanilla `AbstractNautilus.inventory`.
    inventory: Shared<InventoryCarrier>,
    /// Bumped when [`AbstractNautilusParts::create_inventory`] rebuilds the
    /// storage; the mount menu compares it like vanilla's
    /// `AbstractNautilus.hasInventoryChanged` identity check.
    inventory_generation: Arc<AtomicU32>,
    /// Vanilla `AbstractNautilus.playerJumpPendingScale`.
    player_jump_pending_scale: SyncMutex<f32>,
    /// Vanilla `AbstractNautilus.dashCooldown`.
    dash_cooldown: SyncMutex<i32>,
    /// Vanilla `TamableAnimal.orderedToSit`.
    ordered_to_sit: SyncMutex<bool>,
}

impl AbstractNautilusParts {
    /// Creates the shared state for a nautilus with `inventory_columns` storage
    /// columns (`0` for both vanilla concrete types).
    #[must_use]
    pub(crate) fn new(inventory_columns: i32) -> Self {
        Self {
            inventory: InventoryCarrier::new(get_inventory_size(inventory_columns)).into_shared(),
            inventory_generation: Arc::new(AtomicU32::new(0)),
            player_jump_pending_scale: SyncMutex::new(0.0),
            dash_cooldown: SyncMutex::new(0),
            ordered_to_sit: SyncMutex::new(false),
        }
    }

    /// Vanilla `AbstractNautilus.inventory` as a menu container.
    #[must_use]
    pub(crate) fn inventory_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.inventory))
    }

    /// The rebuild token an open mount menu compares against.
    #[must_use]
    pub(crate) fn inventory_generation(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.inventory_generation)
    }

    /// Vanilla `AbstractNautilus.createInventory`: rebuild the storage, copying
    /// the slots that still fit.
    pub(crate) fn create_inventory(&self, size: usize) {
        let mut inventory = self.inventory.lock();
        let mut rebuilt = InventoryCarrier::new(size);
        let copy_count = inventory.size().min(size);
        for slot in 0..copy_count {
            rebuilt.set_item(slot, inventory.get_item(slot).clone());
        }
        *inventory = rebuilt;
        // Vanilla's menu notices the rebuild through `this.inventory != oldInventory`;
        // Steel compares this token instead.
        self.inventory_generation.fetch_add(1, Ordering::Relaxed);
    }

    /// Vanilla `AbstractNautilus.addAdditionalSaveData` `Items` list
    /// (`ItemStackWithSlot` compounds).
    pub(crate) fn save_items(&self, nbt: &mut NbtCompound) {
        let inventory = self.inventory.lock();
        let mut items = Vec::new();
        for slot in 0..inventory.size() {
            let stack = inventory.get_item(slot);
            if stack.is_empty() {
                continue;
            }
            if let NbtTag::Compound(mut compound) = stack.to_nbt_tag_ref() {
                compound.insert("Slot", i8::try_from(slot).unwrap_or(i8::MAX));
                items.push(compound);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
    }

    /// Vanilla `AbstractNautilus.readAdditionalSaveData` `Items` list.
    pub(crate) fn load_items(&self, nbt: &BorrowedNbtCompoundView<'_, '_>) {
        let size = self.inventory.lock().size();
        self.create_inventory(size);
        let Some(compounds) = nbt.list("Items").and_then(|list| list.compounds()) else {
            return;
        };

        let mut inventory = self.inventory.lock();
        for compound in compounds {
            let Some(slot) = compound.byte("Slot") else {
                continue;
            };
            let Ok(slot) = usize::try_from(slot) else {
                continue;
            };
            if slot >= inventory.size() {
                continue;
            }
            if let Some(stack) = ItemStack::from_borrowed_compound(&compound) {
                inventory.set_item(slot, stack);
            }
        }
    }

    /// Vanilla `AbstractNautilus.playerJumpPendingScale`.
    #[must_use]
    pub(crate) fn player_jump_pending_scale(&self) -> f32 {
        *self.player_jump_pending_scale.lock()
    }

    /// Sets vanilla `AbstractNautilus.playerJumpPendingScale`.
    pub(crate) fn set_player_jump_pending_scale(&self, value: f32) {
        *self.player_jump_pending_scale.lock() = value;
    }

    /// Vanilla `AbstractNautilus.dashCooldown`.
    #[must_use]
    pub(crate) fn dash_cooldown(&self) -> i32 {
        *self.dash_cooldown.lock()
    }

    /// Sets vanilla `AbstractNautilus.dashCooldown`.
    pub(crate) fn set_dash_cooldown(&self, cooldown: i32) {
        *self.dash_cooldown.lock() = cooldown;
    }

    /// Vanilla `TamableAnimal.orderedToSit`.
    #[must_use]
    pub(crate) fn ordered_to_sit(&self) -> bool {
        *self.ordered_to_sit.lock()
    }

    /// Sets vanilla `TamableAnimal.orderedToSit`.
    pub(crate) fn set_ordered_to_sit(&self, ordered_to_sit: bool) {
        *self.ordered_to_sit.lock() = ordered_to_sit;
    }
}

/// Accessors the shared `AbstractNautilus` behaviour needs from a concrete
/// nautilus.
///
/// Vanilla reaches these through inheritance; Steel keeps them in one trait so the
/// free functions below stay the single implementation of the shared behaviour.
pub(crate) trait Nautilus: Entity + Animal + Tameable {
    /// Vanilla `AbstractNautilus` non-synchronized state.
    fn parts(&self) -> &AbstractNautilusParts;

    /// Vanilla `AbstractNautilus.isDashing`.
    fn is_dashing(&self) -> bool;

    /// Vanilla `AbstractNautilus.setDashing`.
    fn set_dashing(&self, dashing: bool);

    /// Vanilla `AbstractNautilus.getDashSound`.
    fn dash_sound(&self) -> Option<SoundEventRef>;

    /// Vanilla `AbstractNautilus.getDashReadySound`.
    fn dash_ready_sound(&self) -> Option<SoundEventRef>;

    /// Vanilla `TamableAnimal.setTame`.
    fn set_tame_flag(&self, tame: bool);

    /// Vanilla `TamableAnimal.setOwnerReference`.
    fn set_owner_uuid(&self, owner: Option<Uuid>);

    /// Vanilla `TamableAnimal.getOwnerReference`, reduced to its UUID.
    fn owner_uuid(&self) -> Option<Uuid>;

    /// The entity's brain, equivalent to `LivingEntity.getBrain()`.
    fn brain_mutex(&self) -> &SyncMutex<Brain>;

    /// Vanilla `AbstractNautilus.getInventoryColumns`.
    fn inventory_columns(&self) -> i32 {
        0
    }
}

/// Vanilla `AbstractNautilus.isFood`.
#[must_use]
pub(crate) fn is_food<E: Nautilus>(entity: &E, item_stack: &ItemStack) -> bool {
    if !entity.is_tame() && !AgeableMob::is_baby(entity) {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::NAUTILUS_TAMING_ITEMS)
    } else {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::NAUTILUS_FOOD)
    }
}

/// Vanilla `AbstractNautilus.usePlayerItem`: a `NAUTILUS_BUCKET_FOOD` item is
/// emptied into a water bucket instead of being consumed.
pub(crate) fn use_player_item<E: Nautilus>(
    entity: &E,
    player: &Player,
    hand: InteractionHand,
    item_stack: &ItemStack,
) {
    if REGISTRY
        .items
        .is_in_tag(item_stack.item(), &ItemTag::NAUTILUS_BUCKET_FOOD)
    {
        let overflow = {
            let mut inventory = player.inventory.lock();
            inventory.apply_filled_result(
                hand,
                ItemStack::new(&vanilla_items::WATER_BUCKET),
                player.has_infinite_materials(),
                false,
            )
        };
        if !overflow.is_empty() {
            let _ = player.drop_item(overflow, false, Prediction::Predicted);
        }
        return;
    }

    Mob::use_player_item(entity, player, hand);
}

/// Vanilla `AbstractNautilus.checkNautilusSpawnRules`.
#[must_use]
pub fn check_nautilus_spawn_rules(
    _entity_type: EntityTypeRef,
    level: &World,
    _spawn_reason: EntitySpawnReason,
    pos: BlockPos,
    _random: &mut RandomSource,
) -> bool {
    let sea_level = level.sea_level;
    let min_spawn_level = sea_level - 25;
    pos.y() >= min_spawn_level
        && pos.y() <= sea_level - 5
        && level
            .get_block_state(pos.below())
            .get_fluid_state()
            .is_water()
        && level.get_block_state(pos.above()).get_block() == &vanilla_blocks::WATER
}

/// Vanilla `EntityGetter.isUnobstructed(entity, ignore)` as used by
/// `AbstractNautilus.checkSpawnObstruction`.
///
/// Steel has no `isUnobstructed` helper, so this reproduces the vanilla loop: no
/// other non-removed building-block entity may intersect the hitbox. Vanilla's
/// `Mob.checkSpawnObstruction` additionally rejects liquid-filled hitboxes, which
/// is why the shared [`Mob`] default is not reused here.
#[must_use]
pub(crate) fn check_spawn_obstruction(entity: &dyn Entity, world: &Arc<World>) -> bool {
    let aabb = entity.bounding_box();
    !world.has_entity_in_aabb_matching(&aabb, |other| {
        !other.is_removed()
            && other.id() != entity.id()
            && other.blocks_building()
            && !other.is_passenger_of_same_vehicle(entity.as_entity_event_source())
    })
}

/// Vanilla `AbstractNautilus.canUseSlot`.
///
/// Vanilla delegates non-saddle/body slots to `super.canUseSlot`, whose Steel
/// default is `true`; returning it directly avoids the UFCS self-recursion that a
/// `LivingEntity::can_use_slot` call on the concrete type would cause.
#[must_use]
pub(crate) fn can_use_slot<E: Nautilus>(entity: &E, slot: EquipmentSlot) -> bool {
    if slot != EquipmentSlot::Saddle && slot != EquipmentSlot::Body {
        return true;
    }
    Entity::is_alive(entity) && !AgeableMob::is_baby(entity) && entity.is_tame()
}

/// Vanilla `AbstractNautilus.canDispenserEquipIntoSlot`.
#[must_use]
pub(crate) fn can_dispenser_equip_into_slot<E: Nautilus>(entity: &E, slot: EquipmentSlot) -> bool {
    slot == EquipmentSlot::Body || slot == EquipmentSlot::Saddle || Mob::can_pick_up_loot(entity)
}

/// Vanilla `AbstractNautilus.canAddPassenger`.
#[must_use]
pub(crate) fn can_add_passenger<E: Nautilus>(entity: &E) -> bool {
    !entity.is_vehicle()
}

/// Vanilla `AbstractNautilus.getControllingPassenger`.
#[must_use]
pub(crate) fn controlling_passenger<E: Nautilus>(entity: &E) -> Option<SharedEntity> {
    if entity.is_saddled()
        && let Some(passenger) = entity.first_passenger()
        && passenger.as_player().is_some()
    {
        return Some(passenger);
    }

    entity.controlling_passenger_mob()
}

/// Vanilla `AbstractNautilus.getRiddenInput`.
#[must_use]
pub(crate) fn ridden_input(controller: &Player) -> DVec3 {
    let input = LivingEntity::travel_input(controller);
    let strafe = f64::from(input.sideways());
    let mut forward = 0.0;
    let mut up = 0.0;

    if input.forward() != 0.0 {
        let (_, pitch) = controller.rotation();
        let pitch_radians = pitch.to_radians();
        let mut forward_look = pitch_radians.cos();
        let mut up_look = -pitch_radians.sin();
        if input.forward() < 0.0 {
            forward_look *= -0.5;
            up_look *= -0.5;
        }

        up = f64::from(up_look);
        forward = f64::from(forward_look);
    }

    DVec3::new(strafe, up, forward)
}

/// Vanilla `AbstractNautilus.getRiddenRotation`: pitch halved, yaw unchanged.
#[must_use]
pub(crate) fn ridden_rotation(controller: &Player) -> (f32, f32) {
    let (yaw, pitch) = controller.rotation();
    (pitch * 0.5, yaw)
}

/// Vanilla `AbstractNautilus.tickRidden`.
pub(crate) fn tick_ridden<E: Nautilus>(entity: &E, controller: &Player) {
    let (rotation_pitch, rotation_yaw) = ridden_rotation(controller);
    let mut yaw = entity.rotation().0;
    let diff = steel_math::wrap_degrees(rotation_yaw - yaw);
    yaw += diff * 0.5;
    entity.set_rotation((yaw, rotation_pitch));
    entity.base().set_old_yaw_to_current();
    entity.set_y_body_rot(yaw);
    entity.set_y_head_rot(yaw);

    // Vanilla guards this with `isLocalInstanceAuthoritative()`; the server entity
    // always is.
    if entity.parts().player_jump_pending_scale() > 0.0 && !LivingEntity::is_jumping(entity) {
        let scale = entity.parts().player_jump_pending_scale();
        execute_riders_jump(entity, scale, controller);
    }

    entity.parts().set_player_jump_pending_scale(0.0);
}

/// Vanilla `AbstractNautilus.travelInWater`.
pub(crate) fn travel_in_water(entity: &SharedEntity, input: DVec3) -> Option<MoveResult> {
    let living = entity.as_living_entity()?;
    entity.move_relative(living.get_speed(), input);
    let result = Arc::clone(entity).move_entity(MoverType::SelfMovement, entity.velocity());
    entity.set_velocity(entity.velocity() * NAUTILUS_WATER_RESISTANCE);
    result
}

/// Vanilla `AbstractNautilus.getRiddenSpeed`.
#[must_use]
pub(crate) fn ridden_speed<E: Nautilus>(entity: &E) -> f32 {
    let movement_speed = entity
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32;
    if entity.is_in_water() {
        RIDDEN_SPEED_MODIFIER_IN_WATER * movement_speed
    } else {
        RIDDEN_SPEED_MODIFIER_ON_LAND * movement_speed
    }
}

/// Vanilla `AbstractNautilus.doPlayerRide`.
pub(crate) fn do_player_ride<E: Nautilus>(entity: &E, player: &Player) {
    let Some(world) = entity.level() else {
        return;
    };
    let Some(vehicle) = world.get_entity_by_id(entity.id()) else {
        return;
    };

    player.start_riding(&vehicle);
    if !entity.is_vehicle() {
        entity.clear_home();
    }
}

/// Vanilla `AbstractNautilus.getNautilusRestrictionRadius`.
#[must_use]
pub(crate) fn restriction_radius<E: Nautilus>(entity: &E) -> i32 {
    let mut saddle_empty = true;
    entity.with_equipment_slot(EquipmentSlot::Saddle, &mut |stack| {
        saddle_empty = stack.is_empty();
    });
    if !AgeableMob::is_baby(entity) && saddle_empty {
        LARGE_RESTRICTION_RADIUS
    } else {
        SMALL_RESTRICTION_RADIUS
    }
}

/// Vanilla `Vec3i.closerThan`.
fn block_pos_closer_than(a: BlockPos, b: BlockPos, distance: f64) -> bool {
    let dx = f64::from(a.x()) - f64::from(b.x());
    let dy = f64::from(a.y()) - f64::from(b.y());
    let dz = f64::from(a.z()) - f64::from(b.z());
    dx * dx + dy * dy + dz * dz < distance * distance
}

/// Vanilla `AbstractNautilus.checkRestriction`.
pub(crate) fn check_restriction<E: Nautilus>(entity: &E) {
    if entity.is_leashed() || entity.is_vehicle() || !entity.is_tame() {
        return;
    }

    let radius = restriction_radius(entity);
    let position = entity.block_position();
    if !entity.has_home()
        || !block_pos_closer_than(
            entity.home_position(),
            position,
            f64::from(radius + RESTRICTION_RADIUS_BUFFER),
        )
        || radius != entity.home_radius()
    {
        entity.set_home_to(position, radius);
    }
}

/// Vanilla `AbstractNautilus.applyEffects`: keep the first rider breathing.
fn apply_effects<E: Nautilus>(entity: &E, world: &World) {
    let Some(passenger) = entity.first_passenger() else {
        return;
    };
    let Some(player) = passenger.as_player() else {
        return;
    };

    let has_effect = player.has_mob_effect(vanilla_mob_effects::BREATH_OF_THE_NAUTILUS);
    let should_refresh = world.game_time() % EFFECT_REFRESH_RATE == 0;
    if !has_effect || should_refresh {
        player.add_mob_effect(
            MobEffectInstance::with_duration(
                vanilla_mob_effects::BREATH_OF_THE_NAUTILUS,
                EFFECT_DURATION,
                0,
            )
            .with_ambient(true)
            .with_visible(true)
            .with_show_icon(true),
        );
    }
}

/// Vanilla `AbstractNautilus.spawnBubbles`.
fn spawn_bubbles<E: Nautilus>(entity: &E, world: &World) {
    let speed = entity.velocity().length();
    let bubble_probability = (speed * BUBBLE_PROBABILITY_MULTIPLIER)
        .clamp(BUBBLE_PROBABILITY_MIN, BUBBLE_PROBABILITY_MAX);
    if f64::from(rand::random::<f32>()) >= bubble_probability {
        return;
    }

    let (yaw, pitch) = entity.rotation();
    let mouth_direction = entity.calculate_view_vector(pitch.clamp(-10.0, 10.0), yaw);
    let spread = rand::random::<f64>() * BUBBLE_SPREAD_FACTOR * (1.0 + speed);
    let dx = (f64::from(rand::random::<f32>()) - 0.5) * spread;
    let dy = (f64::from(rand::random::<f32>()) - 0.5) * spread;
    let dz = (f64::from(rand::random::<f32>()) - 0.5) * spread;

    let position = entity.position();
    world.send_particles_with_velocity(
        ParticleData::simple(&vanilla_particle_types::BUBBLE),
        false,
        false,
        DVec3::new(
            position.x - mouth_direction.x * BUBBLE_DIRECTION_SCALE,
            position.y - mouth_direction.y + BUBBLE_Y_OFFSET,
            position.z - mouth_direction.z * BUBBLE_DIRECTION_SCALE,
        ),
        1,
        DVec3::ZERO,
        DVec3::new(dx, dy, dz),
        RandomizationType::default(),
    );
}

/// Vanilla `AbstractNautilus.tick`'s non-`super` half.
pub(crate) fn tick<E: Nautilus>(entity: &E) {
    if let Some(world) = entity.level() {
        apply_effects(entity, &world);
    }

    if entity.is_dashing() && entity.parts().dash_cooldown() < DASH_MINIMUM_DURATION_TICKS {
        entity.set_dashing(false);
    }

    if entity.parts().dash_cooldown() > 0 {
        let cooldown = entity.parts().dash_cooldown() - 1;
        entity.parts().set_dash_cooldown(cooldown);
        if cooldown == 0 {
            entity.make_sound(entity.dash_ready_sound());
        }
    }

    if entity.is_in_water()
        && let Some(world) = entity.level()
    {
        spawn_bubbles(entity, &world);
    }
}

/// Vanilla `AbstractNautilus.canJump`.
#[must_use]
pub(crate) fn can_jump<E: Nautilus>(entity: &E) -> bool {
    entity.is_saddled()
}

/// Vanilla `AbstractNautilus.onPlayerJump`.
pub(crate) fn on_player_jump<E: Nautilus>(entity: &E, jump_amount: i32) {
    if entity.is_saddled() && entity.parts().dash_cooldown() <= 0 {
        entity
            .parts()
            .set_player_jump_pending_scale(player_jump_pending_scale(jump_amount));
    }
}

/// Vanilla `PlayerRideableJumping.getPlayerJumpPendingScale`.
#[must_use]
pub(crate) fn player_jump_pending_scale(jump_amount: i32) -> f32 {
    if jump_amount >= 90 {
        1.0
    } else {
        0.4 + 0.4 * jump_amount as f32 / 90.0
    }
}

/// Vanilla `AbstractNautilus.executeRidersJump`.
pub(crate) fn execute_riders_jump<E: Nautilus>(entity: &E, amount: f32, controller: &Player) {
    let momentum = if entity.is_in_water() {
        DASH_MOMENTUM_IN_WATER
    } else {
        DASH_MOMENTUM_ON_LAND
    };
    let movement_speed = entity
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32;
    let scale =
        f64::from(momentum * amount * movement_speed) * f64::from(entity.block_speed_factor());
    entity.set_velocity(entity.velocity() + controller.look_angle() * scale);
    entity.parts().set_dash_cooldown(DASH_COOLDOWN_TICKS);
    entity.set_dashing(true);
    // Vanilla sets `needsSync = true` to force a velocity packet; Steel's entity
    // tracker syncs velocity for every moving entity each tick.
}

/// Vanilla `Entity.onSyncedDataUpdated(DASH)` for the `!firstTick` case.
pub(crate) fn on_dash_synced<E: Nautilus>(entity: &E) {
    if entity.is_first_tick() {
        return;
    }

    let parts = entity.parts();
    if parts.dash_cooldown() == 0 {
        parts.set_dash_cooldown(DASH_COOLDOWN_TICKS);
    }
}

/// Vanilla `AbstractNautilus.handleStartJump`.
pub(crate) fn handle_start_jump<E: Nautilus>(entity: &E) {
    entity.make_sound(entity.dash_sound());
    entity.game_event(&vanilla_game_events::ENTITY_ACTION);
    entity.set_dashing(true);
    on_dash_synced(entity);
}

/// Vanilla `AbstractNautilus.getJumpCooldown`.
#[must_use]
pub(crate) fn get_jump_cooldown<E: Nautilus>(entity: &E) -> i32 {
    entity.parts().dash_cooldown()
}

/// Vanilla `AbstractNautilus.handleStopJump` is empty.
pub(crate) const fn handle_stop_jump() {}

/// Vanilla `AbstractNautilus.isMobControlled`.
#[must_use]
pub(crate) fn is_mob_controlled<E: Nautilus>(entity: &E) -> bool {
    entity
        .first_passenger()
        .is_some_and(|passenger| passenger.as_mob().is_some())
}

/// Vanilla `AbstractNautilus.isAggravated`.
#[must_use]
pub(crate) fn is_aggravated<E: Nautilus>(entity: &E) -> bool {
    let brain = entity.brain_mutex().lock();
    brain.has_memory_value(&mem::ANGRY_AT) || brain.has_memory_value(&mem::ATTACK_TARGET)
}

/// Vanilla `AbstractNautilus.openCustomInventoryScreen`.
pub(crate) fn open_custom_inventory_screen<E: Nautilus>(entity: &E, player: &Player) {
    if !entity.is_tame() {
        return;
    }

    let ridden_by_other = entity.is_vehicle()
        && !entity
            .passengers()
            .iter()
            .any(|passenger| passenger.id() == player.id());
    if ridden_by_other {
        return;
    }

    open_mount_inventory(entity, player);
}

/// Sends the mount screen packet and installs the matching menu.
fn open_mount_inventory<E: Nautilus>(entity: &E, player: &Player) {
    let Some(world) = entity.level() else {
        return;
    };
    let Some(mount) = world.get_entity_by_id(entity.id()) else {
        return;
    };

    let inventory = Arc::clone(&player.inventory);
    let parts = entity.parts();
    let mount_inventory = parts.inventory_container();
    let generation = parts.inventory_generation();
    let inventory_columns = entity.inventory_columns();
    player.open_mount_menu(entity.id(), inventory_columns, move |context| {
        mount_inventory_menu(
            inventory,
            context.container_id,
            mount,
            mount_inventory,
            inventory_columns,
            generation,
        )
    });
}

/// Vanilla `AbstractNautilus.mobInteract`.
pub(crate) fn mob_interact<E: Nautilus>(
    entity: &E,
    player: &Player,
    hand: InteractionHand,
) -> InteractionResult {
    let item_stack = {
        let inventory = player.inventory.lock();
        let item_stack = inventory.get_item_in_hand(hand);
        item_stack.copy_with_count(item_stack.count())
    };

    if AgeableMob::is_baby(entity) {
        return Animal::mob_interact_animal(entity, player, hand);
    }

    if entity.is_tame() && player.is_secondary_use_active() {
        open_custom_inventory_screen(entity, player);
        return InteractionResult::Success;
    }

    if !item_stack.is_empty() {
        if !entity.is_tame() && is_food(entity, &item_stack) {
            use_player_item(entity, player, hand, &item_stack);
            try_to_tame(entity, player);
            return InteractionResult::SuccessServer;
        }

        if is_food(entity, &item_stack) && entity.get_health() < entity.get_max_health() {
            feed(entity, player, hand, &item_stack);
            return InteractionResult::Success;
        }

        let interaction_result =
            entity
                .as_living_entity()
                .map_or(InteractionResult::Pass, |target| {
                    let mut inventory = player.inventory.lock();
                    let stack = inventory.get_item_in_hand_mut(hand);
                    ITEM_BEHAVIORS
                        .get_behavior(stack.item())
                        .interact_living_entity(stack, player, target, hand)
                });
        if interaction_result.consumes_action() {
            return interaction_result;
        }
    }

    if entity.is_tame() && !player.is_secondary_use_active() && !is_food(entity, &item_stack) {
        do_player_ride(entity, player);
        return InteractionResult::Success;
    }

    Animal::mob_interact_animal(entity, player, hand)
}

/// Vanilla `TamableAnimal.feed`.
fn feed<E: Nautilus>(entity: &E, player: &Player, hand: InteractionHand, item_stack: &ItemStack) {
    let food_nutrition = item_stack
        .get(vanilla_components::FOOD)
        .map(FoodProperties::nutrition);
    use_player_item(entity, player, hand, item_stack);
    let heal = food_nutrition.map_or(FEED_DEFAULT_HEAL, |nutrition| {
        FEED_HEALING_FACTOR * nutrition as f32
    });
    entity.heal(heal);
    entity.play_eating_sound();
}

/// Vanilla `AbstractNautilus.tryToTame`.
pub(crate) fn try_to_tame<E: Nautilus>(entity: &E, player: &Player) {
    if rand::random_range(0..TAMING_ROLL_BOUND) == 0 {
        tame(entity, player);
        entity.mob_base().navigation().lock().stop();
        entity.broadcast_entity_event(EntityStatus::TamingSucceeded);
    } else {
        entity.broadcast_entity_event(EntityStatus::TamingFailed);
    }

    entity.play_eating_sound();
}

/// Vanilla `TamableAnimal.tame`.
///
/// Raises `TAME_ANIMAL` for the taming player, matching vanilla.
pub(crate) fn tame<E: Nautilus>(entity: &E, player: &Player) {
    entity.set_tame_flag(true);
    entity.set_owner_uuid(Some(player.uuid()));
    raise_tame_animal(player, entity);
}

/// Vanilla `AbstractNautilus.requiresCustomPersistence`.
#[must_use]
pub(crate) fn requires_custom_persistence<E: Nautilus>(entity: &E) -> bool {
    (entity.is_passenger() || entity.is_leashed()) || entity.is_tame()
}

/// Vanilla `AbstractNautilus.addAdditionalSaveData`, including the inherited
/// `AgeableMob`/`Animal`/`TamableAnimal` fields.
pub(crate) fn save_additional<E: Nautilus>(entity: &E, nbt: &mut NbtCompound) {
    entity.save_mob(nbt);
    entity.save_ageable_mob(nbt);
    entity.save_animal(nbt);
    if let Some(owner) = entity.owner_uuid() {
        nbt.insert("Owner", NbtTag::IntArray(owner.to_int_array().to_vec()));
    }
    nbt.insert("Sitting", i8::from(entity.is_ordered_to_sit()));
    entity.parts().save_items(nbt);
}

/// Vanilla `AbstractNautilus.readAdditionalSaveData`, including the inherited
/// `AgeableMob`/`Animal`/`TamableAnimal` fields.
pub(crate) fn load_additional<E: Nautilus>(entity: &E, nbt: BorrowedNbtCompoundView<'_, '_>) {
    entity.load_mob(nbt);
    entity.load_ageable_mob(nbt);
    entity.load_animal(nbt);

    let owner = nbt
        .int_array("Owner")
        .and_then(|owner| Uuid::from_int_array(&owner));
    entity.set_owner_uuid(owner);
    entity.set_tame_flag(owner.is_some());

    let ordered_to_sit = nbt.byte("Sitting").is_some_and(|value| value != 0);
    entity.parts().set_ordered_to_sit(ordered_to_sit);
    entity.set_in_sitting_pose(ordered_to_sit);

    entity.parts().load_items(&nbt);
}

/// Vanilla `AbstractNautilus.getEquipSound`.
pub(crate) fn equip_sound<E: Nautilus>(
    entity: &E,
    slot: EquipmentSlot,
    stack: &ItemStack,
) -> Option<SoundEventRef> {
    if slot == EquipmentSlot::Saddle {
        return Some(if entity.is_under_water() {
            &sound_events::ITEM_NAUTILUS_SADDLE_UNDERWATER_EQUIP
        } else {
            &sound_events::ITEM_NAUTILUS_SADDLE_EQUIP
        });
    }

    // `LivingEntity::equip_sound` default body inlined: a UFCS call on the
    // concrete type resolves back to the override and recurses.
    let equippable = stack.get_equippable()?;
    (slot == equippable.slot)
        .then(|| equippable.equip_sound.registry_ref())
        .flatten()
}

/// Vanilla `AbstractNautilus.canBeAffected`: poison is rejected.
#[must_use]
pub(crate) fn can_be_affected<E: Nautilus>(entity: &E, effect: &MobEffectInstance) -> bool {
    if ptr::eq(effect.effect(), vanilla_mob_effects::POISON) {
        return false;
    }

    LivingEntity::default_can_be_affected(entity, effect)
}

/// Vanilla `AbstractNautilus.removeWhenFarAway`.
#[must_use]
pub(crate) const fn remove_when_far_away() -> bool {
    true
}

/// Vanilla `AbstractNautilus.getInventoryColumns` default.
pub(crate) const DEFAULT_INVENTORY_COLUMNS: i32 = 0;
