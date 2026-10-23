//! Vanilla equine family: `Horse`, `Donkey`, `Mule`, `SkeletonHorse` and
//! `ZombieHorse`.
//!
//! # STATUS: INCOMPLETE — do not declare `mod equine;` yet
//!
//! This file is the finished **shared `AbstractHorse` layer**. The five sibling
//! entity files (`horse.rs`, `donkey.rs`, `mule.rs`, `skeleton_horse.rs`,
//! `zombie_horse.rs`) named by the `mod` declarations below do **not** exist
//! yet, so this directory is intentionally unreferenced from
//! `passive/mod.rs` and must stay that way until they land. Wire it in only
//! with: `mod equine;` + `pub use equine::{DonkeyEntity, HorseEntity,
//! MuleEntity, SkeletonHorseEntity, ZombieHorseEntity};`.
//!
//! `HorseMarkings` is re-exported here but must be declared locally in
//! `horse.rs` — the registry has `entity_variant::HorseVariant` but **no**
//! markings enum (verified: `steel-registry/src/entity_variant.rs:480`).
//!
//! Vanilla models this family as `AbstractHorse` → `AbstractChestedHorse`
//! (`Donkey`/`Mule`) plus three direct subclasses. Steel has no class
//! inheritance, so the shared vanilla `AbstractHorse` state lives in
//! [`AbstractHorseParts`] (composition) and the shared behaviour in the free
//! functions below, which every equine entity calls from its own trait impls.
//!
//! # Shared state
//!
//! * synchronized `id_flags` byte (`Tame`/`Bred`/`EatingHaystack`/standing/
//!   mouth-open) — stored in the generated `AbstractHorseEntityData` layer;
//! * non-synchronized `temper`, `owner`, mount inventory and the animation
//!   counters — stored in [`AbstractHorseParts`].
//!
//! # Goals
//!
//! Vanilla registers `RunAroundLikeCrazyGoal`, `MountPanicGoal` (an inner class
//! of `AbstractHorse`), `RandomStandGoal` and `SkeletonTrapGoal`. Those goals
//! read concrete `AbstractHorse` state, so minimal private ports live here and
//! in [`skeleton_horse`], named exactly like vanilla. `GoalsOwner` may later
//! unify them with the shared goal library.
//!
//! # Known gaps
//!
//! * Rider jump (`PlayerRideableJumping.onPlayerJump` / `handleStartJump`) has
//!   no Steel packet hook yet; [`AbstractHorseParts::player_jump_pending_scale`]
//!   is tracked but never driven.
//! * The `AbstractHorse.hurtServer` 1-in-3 rear-up reaction has no hook:
//!   overriding `hurt_server`/`actually_hurt` and calling the trait default via
//!   UFCS recurses (the same latent bug exists in `armadillo`), so the reaction
//!   is left out rather than shipping a stack overflow.
//! * `AbstractHorse.followMommy` needs a nearby-`AbstractHorse` query that the
//!   world API does not expose yet.
//! * The `eatAnim`/`standAnim`/`mouthAnim`/`tailCounter` animation fields are
//!   client-render state and are not modeled.

mod donkey;
mod horse;
mod mule;
mod skeleton_horse;
mod zombie_horse;

pub use donkey::DonkeyEntity;
pub use horse::{HorseEntity, HorseMarkings};
pub use mule::MuleEntity;
pub use skeleton_horse::SkeletonHorseEntity;
pub use zombie_horse::ZombieHorseEntity;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::AbstractHorseEntityData;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, vanilla_attributes, vanilla_blocks, vanilla_item_tags,
    vanilla_items,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::{IntoShared, Shared, SyncMutex};
use steel_utils::types::InteractionHand;
use steel_utils::{Downcast as _, UuidExt as _};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{Goal, GoalControls, PanicGoal};
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    AgeableMob, AgeableMobGroupData, Animal, Entity, LivingEntity, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::inventory::lock::ContainerRef;
use crate::inventory::menu::kinds::mount_inventory_menu;
use crate::player::Player;

/// Vanilla `AbstractHorse.FLAG_TAME`.
pub(crate) const FLAG_TAME: i8 = 2;
/// Vanilla `AbstractHorse.FLAG_BRED`.
pub(crate) const FLAG_BRED: i8 = 8;
/// Vanilla `AbstractHorse.FLAG_EATING`.
pub(crate) const FLAG_EATING: i8 = 16;
/// Vanilla `AbstractHorse.FLAG_STANDING`.
pub(crate) const FLAG_STANDING: i8 = 32;
/// Vanilla `AbstractHorse.FLAG_OPEN_MOUTH`.
pub(crate) const FLAG_OPEN_MOUTH: i8 = 64;

/// Vanilla `AbstractHorse.DEFAULT_TEMPER`.
pub(crate) const DEFAULT_TEMPER: i32 = 0;
/// Vanilla `AbstractHorse.getMaxTemper`.
pub(crate) const MAX_TEMPER: i32 = 100;
/// Vanilla `AbstractMountInventoryMenu.getInventorySize`: columns × 3 rows.
pub(crate) const INVENTORY_ROWS: usize = 3;
/// Vanilla `AbstractHorse.getAmbientSoundInterval` / `getAmbientStandInterval`.
pub(crate) const AMBIENT_STAND_INTERVAL: i32 = 400;

/// Vanilla `AbstractHorse` non-synchronized runtime state.
///
/// Steel models the vanilla `AbstractHorse` class with composition rather than
/// inheritance, so every equine entity owns one of these next to its generated
/// entity-data layer.
pub(crate) struct AbstractHorseParts {
    /// Vanilla `AbstractHorse.temper`.
    temper: SyncMutex<i32>,
    /// Vanilla `AbstractHorse.owner` (`EntityReference` reduced to its UUID).
    owner: SyncMutex<Option<uuid::Uuid>>,
    /// Vanilla `AbstractHorse.inventory`; empty while `getInventoryColumns` is 0.
    ///
    /// Shared so the mount menu can expose it as a [`ContainerRef`] while the
    /// entity keeps owning the same storage.
    inventory: Shared<InventoryCarrier>,
    /// Bumped whenever [`AbstractHorseParts::create_inventory`] rebuilds the
    /// storage; an open mount menu compares it like vanilla's
    /// `AbstractHorse.hasInventoryChanged` identity check.
    inventory_generation: Arc<AtomicU32>,
    /// Vanilla `AbstractHorse.eatingCounter`.
    eating_counter: SyncMutex<i32>,
    /// Vanilla `AbstractHorse.mouthCounter`.
    mouth_counter: SyncMutex<i32>,
    /// Vanilla `AbstractHorse.standCounter`.
    stand_counter: SyncMutex<i32>,
    /// Vanilla `AbstractHorse.playerJumpPendingScale`.
    player_jump_pending_scale: SyncMutex<f32>,
    /// Vanilla `AbstractHorse.allowStandSliding`.
    allow_stand_sliding: SyncMutex<bool>,
    /// Vanilla `AbstractHorse.canGallop`; false for `AbstractChestedHorse`.
    can_gallop: bool,
    /// Vanilla `AbstractHorse.gallopSoundCounter`.
    gallop_sound_counter: SyncMutex<i32>,
}

impl AbstractHorseParts {
    /// Creates the shared state for an equine entity.
    #[must_use]
    pub(crate) fn new(can_gallop: bool) -> Self {
        Self {
            temper: SyncMutex::new(DEFAULT_TEMPER),
            owner: SyncMutex::new(None),
            inventory: InventoryCarrier::new(0).into_shared(),
            inventory_generation: Arc::new(AtomicU32::new(0)),
            eating_counter: SyncMutex::new(0),
            mouth_counter: SyncMutex::new(0),
            stand_counter: SyncMutex::new(0),
            player_jump_pending_scale: SyncMutex::new(0.0),
            allow_stand_sliding: SyncMutex::new(false),
            can_gallop,
            gallop_sound_counter: SyncMutex::new(0),
        }
    }

    /// Vanilla `AbstractHorse.getTemper`.
    #[must_use]
    pub(crate) fn temper(&self) -> i32 {
        *self.temper.lock()
    }

    /// Vanilla `AbstractHorse.setTemper`.
    pub(crate) fn set_temper(&self, temper: i32) {
        *self.temper.lock() = temper;
    }

    /// Vanilla `AbstractHorse.modifyTemper`: clamped to `0..=getMaxTemper()`.
    pub(crate) fn modify_temper(&self, amount: i32) -> i32 {
        let temper = (self.temper() + amount).clamp(0, MAX_TEMPER);
        self.set_temper(temper);
        temper
    }

    /// Vanilla `AbstractHorse.getOwnerReference`, reduced to the owner UUID.
    #[must_use]
    pub(crate) fn owner(&self) -> Option<uuid::Uuid> {
        *self.owner.lock()
    }

    /// Vanilla `AbstractHorse.setOwner`.
    pub(crate) fn set_owner(&self, owner: Option<uuid::Uuid>) {
        *self.owner.lock() = owner;
    }

    /// Vanilla `AbstractHorse.inventory`.
    #[must_use]
    pub(crate) fn inventory(&self) -> &Shared<InventoryCarrier> {
        &self.inventory
    }

    /// The mount inventory as a menu container.
    #[must_use]
    pub(crate) fn inventory_container(&self) -> ContainerRef {
        ContainerRef::from(self.inventory.clone())
    }

    /// The rebuild token an open mount menu compares against.
    #[must_use]
    pub(crate) fn inventory_generation(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.inventory_generation)
    }

    /// Vanilla `AbstractHorse.createInventory`: rebuilds storage, copying slots.
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

    /// Vanilla `AbstractChestedHorse.addAdditionalSaveData` `Items` list.
    ///
    /// Each entry is an `ItemStackWithSlot`: the stack compound plus a `Slot`
    /// byte, matching vanilla's record codec.
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

    /// Vanilla `AbstractChestedHorse.readAdditionalSaveData` `Items` list.
    pub(crate) fn load_items(&self, nbt: &BorrowedNbtCompoundView<'_, '_>, size: usize) {
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

    /// Vanilla `AbstractHorse.addAdditionalSaveData` fields.
    pub(crate) fn save_abstract_horse(
        &self,
        data: &AbstractHorseEntityData,
        nbt: &mut NbtCompound,
    ) {
        nbt.insert("EatingHaystack", i8::from(get_flag(data, FLAG_EATING)));
        nbt.insert("Bred", i8::from(get_flag(data, FLAG_BRED)));
        nbt.insert("Temper", self.temper());
        nbt.insert("Tame", i8::from(get_flag(data, FLAG_TAME)));
        if let Some(owner) = self.owner() {
            nbt.insert("Owner", NbtTag::IntArray(owner.to_int_array().to_vec()));
        }
    }

    /// Vanilla `AbstractHorse.readAdditionalSaveData` fields.
    pub(crate) fn load_abstract_horse(
        &self,
        data: &mut AbstractHorseEntityData,
        nbt: &BorrowedNbtCompoundView<'_, '_>,
    ) {
        set_flag(
            data,
            FLAG_EATING,
            nbt.byte("EatingHaystack").is_some_and(|v| v != 0),
        );
        set_flag(data, FLAG_BRED, nbt.byte("Bred").is_some_and(|v| v != 0));
        self.set_temper(nbt.int("Temper").unwrap_or(DEFAULT_TEMPER));
        set_flag(data, FLAG_TAME, nbt.byte("Tame").is_some_and(|v| v != 0));

        // Vanilla `EntityReference.readWithOldOwnerConversion` also accepts a
        // player-name string. Steel only implements the modern UUID form.
        self.set_owner(
            nbt.int_array("Owner")
                .and_then(|uuid| uuid::Uuid::from_int_array(&uuid)),
        );
    }

    /// Vanilla `AbstractHorse.getInventoryColumns` × [`INVENTORY_ROWS`].
    #[must_use]
    pub(crate) fn inventory_size(columns: i32) -> usize {
        usize::try_from(columns.max(0)).unwrap_or(0) * INVENTORY_ROWS
    }

    /// Vanilla `AbstractHorse.canGallop`.
    #[must_use]
    pub(crate) fn can_gallop(&self) -> bool {
        self.can_gallop
    }

    /// Vanilla `AbstractHorse.gallopSoundCounter`.
    #[must_use]
    pub(crate) fn gallop_sound_counter(&self) -> i32 {
        *self.gallop_sound_counter.lock()
    }

    /// Sets vanilla `AbstractHorse.gallopSoundCounter`.
    pub(crate) fn set_gallop_sound_counter(&self, value: i32) {
        *self.gallop_sound_counter.lock() = value;
    }

    /// Vanilla `AbstractHorse.playerJumpPendingScale`.
    #[must_use]
    pub(crate) fn player_jump_pending_scale(&self) -> f32 {
        *self.player_jump_pending_scale.lock()
    }

    /// Sets vanilla `AbstractHorse.playerJumpPendingScale`.
    pub(crate) fn set_player_jump_pending_scale(&self, value: f32) {
        *self.player_jump_pending_scale.lock() = value;
    }

    /// Vanilla `AbstractHorse.allowStandSliding`.
    #[must_use]
    pub(crate) fn allow_stand_sliding(&self) -> bool {
        *self.allow_stand_sliding.lock()
    }

    /// Sets vanilla `AbstractHorse.allowStandSliding`.
    pub(crate) fn set_allow_stand_sliding(&self, value: bool) {
        *self.allow_stand_sliding.lock() = value;
    }
}

/// Vanilla `AbstractHorse.getFlag`.
#[must_use]
pub(crate) fn get_flag(data: &AbstractHorseEntityData, flag: i8) -> bool {
    *data.id_flags.get() & flag != 0
}

/// Vanilla `AbstractHorse.setFlag`.
pub(crate) fn set_flag(data: &mut AbstractHorseEntityData, flag: i8, value: bool) {
    let current = *data.id_flags.get();
    let next = if value {
        current | flag
    } else {
        current & !flag
    };
    data.id_flags.set(next);
}

/// Vanilla `AbstractHorse.isTamed`.
#[must_use]
pub(crate) fn is_tamed(data: &AbstractHorseEntityData) -> bool {
    get_flag(data, FLAG_TAME)
}

/// Vanilla `AbstractHorse.isEating`.
#[must_use]
pub(crate) fn is_eating(data: &AbstractHorseEntityData) -> bool {
    get_flag(data, FLAG_EATING)
}

/// Vanilla `AbstractHorse.isStanding`.
#[must_use]
pub(crate) fn is_standing(data: &AbstractHorseEntityData) -> bool {
    get_flag(data, FLAG_STANDING)
}

/// Vanilla `AbstractHorse.isBred`.
#[must_use]
pub(crate) fn is_bred(data: &AbstractHorseEntityData) -> bool {
    get_flag(data, FLAG_BRED)
}

/// Vanilla `AbstractHorse.setEating`.
pub(crate) fn set_eating(data: &mut AbstractHorseEntityData, eating: bool) {
    set_flag(data, FLAG_EATING, eating);
}

/// Vanilla `AbstractHorse.setBred`.
pub(crate) fn set_bred(data: &mut AbstractHorseEntityData, bred: bool) {
    set_flag(data, FLAG_BRED, bred);
}

/// Vanilla `AbstractHorse.setStanding`.
pub(crate) fn set_standing(
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    ticks: i32,
) {
    set_eating(data, false);
    set_flag(data, FLAG_STANDING, true);
    *parts.stand_counter.lock() = ticks;
}

/// Vanilla `AbstractHorse.clearStanding`.
pub(crate) fn clear_standing(parts: &AbstractHorseParts, data: &mut AbstractHorseEntityData) {
    set_flag(data, FLAG_STANDING, false);
    *parts.stand_counter.lock() = 0;
}

/// Vanilla `AbstractHorse.generateMaxHealth`: `15 + f(8) + f(9)`.
#[must_use]
pub(crate) fn generate_max_health() -> f32 {
    15.0 + rand::random_range(0..8) as f32 + rand::random_range(0..9) as f32
}

/// Vanilla `AbstractHorse.generateJumpStrength`: `0.4 + 3 × [0,0.2)`.
#[must_use]
pub(crate) fn generate_jump_strength() -> f64 {
    0.4 + rand::random::<f64>() * 0.2 + rand::random::<f64>() * 0.2 + rand::random::<f64>() * 0.2
}

/// Vanilla `AbstractHorse.generateSpeed`: `(0.45 + 3 × [0,0.3)) * 0.25`.
#[must_use]
pub(crate) fn generate_speed() -> f64 {
    (0.45 + rand::random::<f64>() * 0.3 + rand::random::<f64>() * 0.3 + rand::random::<f64>() * 0.3)
        * 0.25
}

/// Vanilla `AbstractHorse.createOffspringAttribute`.
#[must_use]
pub(crate) fn create_offspring_attribute(
    parent_a_value: f64,
    parent_b_value: f64,
    attribute_range_min: f64,
    attribute_range_max: f64,
) -> f64 {
    let parent_a_value = parent_a_value.clamp(attribute_range_min, attribute_range_max);
    let parent_b_value = parent_b_value.clamp(attribute_range_min, attribute_range_max);
    let margin = 0.15 * (attribute_range_max - attribute_range_min);
    let range = (parent_a_value - parent_b_value).abs() + margin * 2.0;
    let average = (parent_a_value + parent_b_value) / 2.0;
    let baby_quality =
        (rand::random::<f64>() + rand::random::<f64>() + rand::random::<f64>()) / 3.0 - 0.5;
    let new_value = average + range * baby_quality;
    if new_value > attribute_range_max {
        attribute_range_max - (new_value - attribute_range_max)
    } else if new_value < attribute_range_min {
        attribute_range_min + (attribute_range_min - new_value)
    } else {
        new_value
    }
}

/// Vanilla `AbstractHorse.MIN_HEALTH` (`generateMaxHealth(i -> 0)`).
pub(crate) const MIN_HEALTH: f64 = 15.0;
/// Vanilla `AbstractHorse.MAX_HEALTH` (`generateMaxHealth(i -> i - 1)`).
pub(crate) const MAX_HEALTH: f64 = 30.0;
/// Vanilla `AbstractHorse.MIN_JUMP_STRENGTH`.
pub(crate) const MIN_JUMP_STRENGTH: f64 = 0.4;
/// Vanilla `AbstractHorse.MAX_JUMP_STRENGTH`.
pub(crate) const MAX_JUMP_STRENGTH: f64 = 1.0;
/// Vanilla `AbstractHorse.MIN_MOVEMENT_SPEED`.
pub(crate) const MIN_MOVEMENT_SPEED: f64 = 0.1125;
/// Vanilla `AbstractHorse.MAX_MOVEMENT_SPEED`.
pub(crate) const MAX_MOVEMENT_SPEED: f64 = 0.3375;

/// Vanilla `AbstractHorse.setOffspringAttributes`.
pub(crate) fn set_offspring_attributes(
    parent: &dyn Animal,
    partner: &dyn crate::entity::AgeableMob,
    baby: &dyn Animal,
) {
    let ranges = [
        (vanilla_attributes::MAX_HEALTH, MIN_HEALTH, MAX_HEALTH),
        (
            vanilla_attributes::JUMP_STRENGTH,
            MIN_JUMP_STRENGTH,
            MAX_JUMP_STRENGTH,
        ),
        (
            vanilla_attributes::MOVEMENT_SPEED,
            MIN_MOVEMENT_SPEED,
            MAX_MOVEMENT_SPEED,
        ),
    ];

    for (attribute, min, max) in ranges {
        let parent_a = parent
            .attributes()
            .lock()
            .get_instance(attribute)
            .map_or(0.0, |instance| instance.base_value());
        let parent_b = partner
            .attributes()
            .lock()
            .get_instance(attribute)
            .map_or(0.0, |instance| instance.base_value());
        let value = create_offspring_attribute(parent_a, parent_b, min, max);
        baby.attributes().lock().set_base_value(attribute, value);
    }
}

/// Vanilla `AbstractHorse.isFood`: `ItemTags.HORSE_FOOD`.
#[must_use]
pub(crate) fn is_horse_food(item_stack: &ItemStack) -> bool {
    REGISTRY
        .items
        .is_in_tag(item_stack.item(), &vanilla_item_tags::ItemTag::HORSE_FOOD)
}

/// Vanilla `AbstractHorse.getEquipSound`: saddle uses `HORSE_SADDLE`.
#[must_use]
pub(crate) fn equip_sound(slot: EquipmentSlot) -> Option<SoundEventRef> {
    (slot == EquipmentSlot::Saddle).then_some(&steel_registry::sound_events::ENTITY_HORSE_SADDLE)
}

/// Vanilla `AbstractChestedHorse.equipChest`: consume the chest and rebuild the
/// mount inventory with `columns` columns.
///
/// Callers set the chest flag first (`AbstractChestedHorse.equipChest` does
/// `setChest(true)` before `createInventory`).
pub(crate) fn equip_chest<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    player: &Player,
    hand: InteractionHand,
    chest_sound: SoundEventRef,
    columns: i32,
) {
    entity.play_sound(
        chest_sound,
        1.0,
        (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0,
    );

    let mut inventory = player.inventory.lock();
    inventory
        .get_item_in_hand_mut(hand)
        .consume(1, player.has_infinite_materials());
    drop(inventory);

    parts.create_inventory(AbstractHorseParts::inventory_size(columns));
}

/// Vanilla `AbstractHorse.addAdditionalSaveData` fields, as a free function.
pub(crate) fn save_abstract_horse(
    parts: &AbstractHorseParts,
    data: &AbstractHorseEntityData,
    nbt: &mut NbtCompound,
) {
    parts.save_abstract_horse(data, nbt);
}

/// Vanilla `AbstractHorse.readAdditionalSaveData` fields, as a free function.
pub(crate) fn load_abstract_horse(
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    nbt: &BorrowedNbtCompoundView<'_, '_>,
) {
    parts.load_abstract_horse(data, nbt);
}

/// Vanilla `AbstractHorse.equipBodyArmor`.
pub(crate) fn equip_body_armor<E: LivingEntity + ?Sized>(
    entity: &E,
    player: &Player,
    hand: InteractionHand,
) {
    let equipped = {
        let mut inventory = player.inventory.lock();
        inventory
            .get_item_in_hand_mut(hand)
            .consume_and_return(1, player.has_infinite_materials())
    };
    let mut equipped = Some(equipped);
    entity.with_equipment_slot_mut(EquipmentSlot::Body, &mut |slot| {
        *slot = equipped.take().expect("equipment visitor is invoked once");
    });
}

/// Vanilla `AbstractHorse.doPlayerRide`.
pub(crate) fn do_player_ride<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    player: &Player,
) {
    set_eating(data, false);
    clear_standing(parts, data);

    let (yaw, pitch) = entity.rotation();
    player.set_rotation((yaw, pitch));

    if let Some(world) = entity.level()
        && let Some(vehicle) = world.get_entity_by_id(entity.id())
    {
        player.start_riding(&vehicle);
    }
}

/// Vanilla `AbstractHorse.openCustomInventoryScreen`.
///
/// Opens the mount inventory screen through `ClientboundMountScreenOpenPacket`
/// (vanilla 26.3 has no `horse` menu type) unless the horse is being ridden by
/// someone else or is untamed.
pub(crate) fn open_custom_inventory_screen<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &AbstractHorseEntityData,
    player: &Player,
    inventory_columns: i32,
) {
    let ridden_by_other = entity.is_vehicle()
        && !entity
            .passengers()
            .iter()
            .any(|passenger| passenger.id() == player.id());
    if ridden_by_other || !is_tamed(data) {
        return;
    }

    open_mount_inventory(entity, parts, player, inventory_columns);
}

/// Sends the mount screen packet and installs the matching menu.
///
/// Shared by the equines (which gate on taming) and the camel (which is always
/// tamed and skips the vehicle check), matching each vanilla override.
pub(crate) fn open_mount_inventory<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    player: &Player,
    inventory_columns: i32,
) {
    let Some(world) = entity.level() else {
        return;
    };
    let Some(mount) = world.get_entity_by_id(entity.id()) else {
        return;
    };

    let inventory = Arc::clone(&player.inventory);
    let mount_inventory = parts.inventory_container();
    let generation = parts.inventory_generation();
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

/// Vanilla `AbstractHorse.mobInteract`.
pub(crate) fn abstract_horse_mob_interact<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    player: &Player,
    hand: InteractionHand,
    inventory_columns: i32,
) -> InteractionResult {
    if entity.is_vehicle() || AgeableMob::is_baby(entity) {
        return Animal::mob_interact_animal(entity, player, hand);
    }

    if is_tamed(data) && player.is_secondary_use_active() {
        open_custom_inventory_screen(entity, parts, data, player, inventory_columns);
        return InteractionResult::Success;
    }

    let item_stack = {
        let inventory = player.inventory.lock();
        let item_stack = inventory.get_item_in_hand(hand);
        item_stack.copy_with_count(item_stack.count())
    };

    if !item_stack.is_empty() {
        let result = LivingEntity::interact_living_entity_with_equippable(entity, player, hand);
        if result.consumes_action() {
            return result;
        }

        if entity.is_equippable_in_slot(&item_stack, EquipmentSlot::Body)
            && !entity.has_item_in_slot(EquipmentSlot::Body)
        {
            equip_body_armor(entity, player, hand);
            return InteractionResult::Success;
        }
    }

    do_player_ride(entity, parts, data, player);
    InteractionResult::Success
}

/// Vanilla `AbstractHorse.fedFood`.
pub(crate) fn fed_food<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    player: &Player,
    hand: InteractionHand,
    eating_sound: Option<SoundEventRef>,
) -> InteractionResult {
    let item_stack = {
        let inventory = player.inventory.lock();
        let item_stack = inventory.get_item_in_hand(hand);
        item_stack.copy_with_count(item_stack.count())
    };

    let ate_food = handle_eating(entity, parts, data, player, &item_stack, eating_sound);
    if ate_food {
        let mut inventory = player.inventory.lock();
        inventory
            .get_item_in_hand_mut(hand)
            .consume(1, player.has_infinite_materials());
    }

    if ate_food {
        InteractionResult::SuccessServer
    } else {
        InteractionResult::Pass
    }
}

/// Vanilla `AbstractHorse.handleEating`.
pub(crate) fn handle_eating<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    player: &Player,
    item_stack: &ItemStack,
    eating_sound: Option<SoundEventRef>,
) -> bool {
    let mut item_used = false;
    let mut heal = 0.0_f32;
    let mut age_up = 0_i32;
    let mut temper = 0_i32;

    if item_stack.is(&vanilla_items::WHEAT) {
        heal = 2.0;
        age_up = 20;
        temper = 3;
    } else if item_stack.is(&vanilla_items::SUGAR) {
        heal = 1.0;
        age_up = 30;
        temper = 3;
    } else if item_stack.is(&vanilla_items::HAY_BLOCK) {
        heal = 20.0;
        age_up = 180;
    } else if item_stack.is(&vanilla_items::APPLE) {
        heal = 3.0;
        age_up = 60;
        temper = 3;
    } else if item_stack.is(&vanilla_items::RED_MUSHROOM) {
        heal = 3.0;
        age_up = 0;
        temper = 3;
    } else if item_stack.is(&vanilla_items::CARROT) {
        heal = 3.0;
        age_up = 60;
        temper = 3;
    } else if item_stack.is(&vanilla_items::GOLDEN_CARROT) {
        heal = 4.0;
        age_up = 60;
        temper = 5;
        if is_tamed(data) && entity.get_age() == 0 && !entity.is_in_love() {
            item_used = true;
            entity.set_in_love(Some(player));
        }
    } else if item_stack.is(&vanilla_items::GOLDEN_APPLE)
        || item_stack.is(&vanilla_items::ENCHANTED_GOLDEN_APPLE)
    {
        heal = 10.0;
        age_up = 240;
        temper = 10;
        if is_tamed(data) && entity.get_age() == 0 && !entity.is_in_love() {
            item_used = true;
            entity.set_in_love(Some(player));
        }
    }

    if entity.get_health() < entity.get_max_health() && heal > 0.0 {
        entity.heal(heal);
        item_used = true;
    }

    if AgeableMob::is_baby(entity) && age_up > 0 && !entity.is_age_locked() {
        // VANILLA CLIENT-LOCAL: `HAPPY_VILLAGER` particles around the baby.
        entity.age_up(age_up, false);
        item_used = true;
    }

    if temper > 0 && (item_used || !is_tamed(data)) && parts.temper() < MAX_TEMPER {
        parts.modify_temper(temper);
        item_used = true;
    }

    if item_used {
        eating(entity, parts, data, eating_sound);
        // GAP: `GameEvent.EAT` is not dispatched from entity code yet.
    }

    item_used
}

/// Vanilla `AbstractHorse.eating`: opens the mouth and plays the eating sound.
pub(crate) fn eating<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    eating_sound: Option<SoundEventRef>,
) {
    *parts.mouth_counter.lock() = 1;
    set_flag(data, FLAG_OPEN_MOUTH, true);

    if !entity.is_silent()
        && let Some(sound) = eating_sound
    {
        let pitch = 1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.2;
        entity.play_sound(sound, 1.0, pitch);
    }
}

/// Vanilla `AbstractHorse.standIfPossible`.
pub(crate) fn stand_if_possible<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    can_perform_rearing: bool,
) {
    if can_perform_rearing && (entity.is_effective_ai() || entity.level().is_some()) {
        set_standing(parts, data, 20);
    }
}

/// Vanilla `AbstractHorse.makeMad`.
pub(crate) fn make_mad<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    can_perform_rearing: bool,
    angry_sound: Option<SoundEventRef>,
) {
    if !is_standing(data) {
        stand_if_possible(entity, parts, data, can_perform_rearing);
        if let Some(sound) = angry_sound {
            entity.play_sound(sound, entity.sound_volume(), 1.0);
        }
    }
}

/// Vanilla `AbstractHorse.tameWithName`.
pub(crate) fn tame_with_name<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    player: &SharedEntity,
) {
    parts.set_owner(Some(player.uuid()));
    set_flag(data, FLAG_TAME, true);
    // GAP: `CriteriaTriggers.TAME_ANIMAL` advancement trigger is not wired yet.
    entity.broadcast_entity_event(EntityStatus::TamingSucceeded);
}

/// Vanilla `AbstractHorse.aiStep` non-movement bookkeeping.
///
/// Covers the mouth/stand counters, the periodic 1 HP regeneration, and the
/// `canEatGrass` state machine. Vanilla's tail counter and the `eatAnim` /
/// `standAnim` / `mouthAnim` interpolation are client-render state.
pub(crate) fn tick_abstract_horse<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &mut AbstractHorseEntityData,
    can_eat_grass: bool,
) {
    {
        let mut mouth_counter = parts.mouth_counter.lock();
        if *mouth_counter > 0 {
            *mouth_counter += 1;
            if *mouth_counter > 30 {
                *mouth_counter = 0;
                set_flag(data, FLAG_OPEN_MOUTH, false);
            }
        }
    }

    {
        let mut stand_counter = parts.stand_counter.lock();
        if *stand_counter > 0 {
            *stand_counter -= 1;
            if *stand_counter <= 0 {
                set_flag(data, FLAG_STANDING, false);
            }
        }
    }

    if !Entity::is_alive(entity) {
        return;
    }

    // Vanilla `AbstractHorse.aiStep` server branch.
    if rand::random_range(0..900) == 0 && !entity.is_dead_or_dying() {
        entity.heal(1.0);
    }

    if can_eat_grass {
        if !is_eating(data)
            && !entity.is_vehicle()
            && rand::random_range(0..300) == 0
            && entity.level().is_some_and(|world| {
                world
                    .get_block_state(entity.block_position().below())
                    .get_block()
                    == &vanilla_blocks::GRASS_BLOCK
            })
        {
            set_eating(data, true);
        }

        if is_eating(data) {
            let mut counter = parts.eating_counter.lock();
            *counter += 1;
            if *counter > 50 {
                *counter = 0;
                set_eating(data, false);
            }
        }
    }

    // GAP: `AbstractHorse.followMommy` needs a nearby-`AbstractHorse` query.
}

/// Vanilla `AbstractHorse.isImmobile` tail: `isEating() || isStanding()`.
#[must_use]
pub(crate) fn is_immobile_from_state(data: &AbstractHorseEntityData) -> bool {
    is_eating(data) || is_standing(data)
}

/// Vanilla `AbstractHorse.tickRidden`.
pub(crate) fn tick_ridden<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    controller: &Player,
    ridden_input: DVec3,
) {
    let (yaw, pitch) = controller.rotation();
    entity.set_rotation((yaw, pitch * 0.5));
    entity.base().set_old_yaw_to_current();
    let yaw = entity.rotation().0;
    entity.set_y_body_rot(yaw);
    entity.set_y_head_rot(yaw);

    if ridden_input.z <= 0.0 {
        parts.set_gallop_sound_counter(0);
    }

    // GAP: `PlayerRideableJumping` jump input has no Steel packet hook, so
    // `executeRidersJump` cannot run; `playerJumpPendingScale` stays 0.
}

/// Vanilla `AbstractHorse.getRiddenInput`.
#[must_use]
pub(crate) fn ridden_input<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    data: &AbstractHorseEntityData,
    controller: &Player,
) -> DVec3 {
    if entity.on_ground()
        && parts.player_jump_pending_scale() == 0.0
        && is_standing(data)
        && !parts.allow_stand_sliding()
    {
        return DVec3::ZERO;
    }

    let controller_input = LivingEntity::travel_input(controller);
    let sideways = f64::from(controller_input.sideways()) * 0.5;
    let mut forward = f64::from(controller_input.forward());
    if forward <= 0.0 {
        forward *= 0.25;
    }

    DVec3::new(sideways, 0.0, forward)
}

/// Vanilla `AbstractHorse.getRiddenSpeed`.
#[must_use]
pub(crate) fn ridden_speed<E: Animal + ?Sized>(entity: &E) -> f32 {
    entity
        .attributes()
        .lock()
        .required_value(vanilla_attributes::MOVEMENT_SPEED) as f32
}

/// Vanilla `AbstractHorse.getControllingPassenger`.
#[must_use]
pub(crate) fn controlling_passenger<E: Animal + ?Sized>(entity: &E) -> Option<SharedEntity> {
    if entity.is_saddled()
        && let Some(passenger) = entity.first_passenger()
        && passenger.as_player().is_some()
    {
        return Some(passenger);
    }

    entity.controlling_passenger_mob()
}

/// Vanilla `AbstractHorse.playStepSound`.
///
/// GAP: vanilla derives volume/pitch from the block's `SoundType` and swaps in
/// `HORSE_STEP_WOOD` for wood and snow; Steel has no `SoundType` accessor, so
/// this uses vanilla's base step volume and always the plain step sound.
pub(crate) fn play_step_sound<E: Animal + ?Sized>(
    entity: &E,
    parts: &AbstractHorseParts,
    baby_step_sound: SoundEventRef,
    step_sound: SoundEventRef,
    gallop_sound: SoundEventRef,
) {
    if entity.is_vehicle() && parts.can_gallop() {
        let counter = parts.gallop_sound_counter() + 1;
        parts.set_gallop_sound_counter(counter);
        if counter > 5 && counter % 3 == 0 {
            entity.play_sound(gallop_sound, 0.15, 1.0);
        } else if counter <= 5 {
            entity.play_sound(
                &steel_registry::sound_events::ENTITY_HORSE_STEP_WOOD,
                0.15,
                1.0,
            );
        }
    } else if AgeableMob::is_baby(entity) {
        entity.play_sound(baby_step_sound, 0.15, 1.0);
    } else {
        entity.play_sound(step_sound, 0.15, 1.0);
    }
}

/// Vanilla `AbstractHorse` capability surface the local equine goals read.
///
/// This mirrors `AbstractHorse`'s methods exactly. `GoalsOwner` may later
/// replace it with the shared `ai::goal` capability trait; every equine entity
/// already implements it so the swap is mechanical.
pub(crate) trait Equine: Send + Sync + 'static {
    /// Vanilla `AbstractHorse.isTamed`.
    fn is_tamed(&self) -> bool;

    /// Vanilla `AbstractHorse.getTemper`.
    fn temper(&self) -> i32;

    /// Vanilla `AbstractHorse.getMaxTemper`.
    fn max_temper(&self) -> i32;

    /// Vanilla `AbstractHorse.modifyTemper`.
    fn modify_temper(&self, amount: i32) -> i32;

    /// Vanilla `AbstractHorse.tameWithName`.
    fn tame_with_name(&self, player: &SharedEntity);

    /// Vanilla `AbstractHorse.makeMad`.
    fn make_mad(&self, mob: &SharedEntity);

    /// Vanilla `AbstractHorse.standIfPossible`.
    fn stand_if_possible(&self);

    /// Vanilla `AbstractHorse.isMobControlled`.
    fn is_mob_controlled(&self) -> bool {
        false
    }

    /// Vanilla `AbstractHorse.getAmbientStandInterval`.
    fn ambient_stand_interval(&self) -> i32 {
        AMBIENT_STAND_INTERVAL
    }

    /// Vanilla `AbstractHorse.getAmbientStandSound`.
    fn ambient_stand_sound(&self) -> Option<SoundEventRef>;
}

/// Resolves the shared equine capability from an erased pathfinder mob.
fn equine(mob: &dyn PathfinderMob) -> Option<&dyn Equine> {
    if let Some(horse) = mob.downcast_ref::<HorseEntity>() {
        return Some(horse);
    }
    if let Some(donkey) = mob.downcast_ref::<DonkeyEntity>() {
        return Some(donkey);
    }
    if let Some(mule) = mob.downcast_ref::<MuleEntity>() {
        return Some(mule);
    }
    if let Some(horse) = mob.downcast_ref::<SkeletonHorseEntity>() {
        return Some(horse);
    }
    if let Some(horse) = mob.downcast_ref::<ZombieHorseEntity>() {
        return Some(horse);
    }
    None
}

/// Vanilla `RunAroundLikeCrazyGoal`: an untamed ridden horse bucks the player
/// off and slowly raises its temper until it is tamed.
pub(crate) struct RunAroundLikeCrazyGoal {
    speed_modifier: f64,
    target: Option<DVec3>,
}

impl RunAroundLikeCrazyGoal {
    /// Vanilla `RunAroundLikeCrazyGoal(AbstractHorse, double)`.
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64) -> Self {
        Self {
            speed_modifier,
            target: None,
        }
    }
}

impl Goal for RunAroundLikeCrazyGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(horse) = equine(mob) else {
            return false;
        };
        if horse.is_mob_controlled() || horse.is_tamed() || !mob.is_vehicle() {
            return false;
        }

        let Some(target) = random_nearby_position(mob, 5, 4) else {
            return false;
        };
        self.target = Some(target);
        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(target) = self.target {
            mob.move_to_pos(target, self.speed_modifier);
        }
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        equine(mob).is_some_and(|horse| !horse.is_tamed())
            && !mob.mob_base().navigation().lock().is_done()
            && mob.is_vehicle()
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        let Some(horse) = equine(mob) else {
            return;
        };
        if horse.is_tamed() || rand::random_range(0..50) != 0 {
            return;
        }

        let Some(passenger) = mob.first_passenger() else {
            return;
        };

        if passenger.as_player().is_some() {
            let temper = horse.temper();
            let max_temper = horse.max_temper();
            if max_temper > 0 && rand::random_range(0..max_temper) < temper {
                horse.tame_with_name(entity);
                return;
            }
            horse.modify_temper(5);
        }

        // Vanilla `ejectPassengers()`; Steel stops the relationship per passenger.
        passenger.stop_riding();
        horse.make_mad(entity);
        entity.broadcast_entity_event(EntityStatus::TamingFailed);
    }
}

/// Vanilla `AbstractHorse.MountPanicGoal`: panic unless a mob is in control.
pub(crate) struct MountPanicGoal {
    inner: PanicGoal,
}

impl MountPanicGoal {
    /// Vanilla `MountPanicGoal(double speedModifier)`.
    #[must_use]
    pub(crate) const fn new(speed_modifier: f64) -> Self {
        Self {
            inner: PanicGoal::new(speed_modifier),
        }
    }
}

impl Goal for MountPanicGoal {
    fn controls(&self) -> GoalControls {
        self.inner.controls()
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let mob_controlled = equine(mob).is_some_and(|horse| horse.is_mob_controlled());
        !mob_controlled && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn is_interruptable(&self) -> bool {
        self.inner.is_interruptable()
    }

    fn is_panic_goal(&self) -> bool {
        true
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}

/// Vanilla `RandomStandGoal`: horses occasionally rear up.
pub(crate) struct RandomStandGoal {
    next_stand: i32,
    /// Vanilla `RandomStandGoal` reads `AbstractHorse.getAmbientStandInterval`;
    /// Steel cannot borrow the mob during goal construction, so the interval is
    /// passed in explicitly instead of a `&dyn PathfinderMob`.
    interval: i32,
}

impl RandomStandGoal {
    /// Vanilla `RandomStandGoal(AbstractHorse)`, taking the horse's
    /// `getAmbientStandInterval` value.
    #[must_use]
    pub(crate) const fn new(interval: i32) -> Self {
        Self {
            next_stand: -interval,
            interval,
        }
    }

    fn reset_stand_interval(&mut self) {
        self.next_stand = -self.interval;
    }
}

impl Goal for RandomStandGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.next_stand += 1;
        if self.next_stand > 0 && rand::random_range(0..1000) < self.next_stand {
            self.reset_stand_interval();
            !mob.is_immobile() && rand::random_range(0..10) == 0
        } else {
            false
        }
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(horse) = equine(mob) else {
            return;
        };
        horse.stand_if_possible();
        if let Some(sound) = horse.ambient_stand_sound() {
            mob.play_sound(sound, 1.0, 1.0);
        }
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }
}

/// Minimal `DefaultRandomPos.getPos` port used by [`RunAroundLikeCrazyGoal`].
fn random_nearby_position(
    mob: &dyn PathfinderMob,
    horizontal_dist: i32,
    vertical_dist: i32,
) -> Option<DVec3> {
    let world = mob.level()?;
    let origin = mob.block_position();

    for _ in 0..10 {
        let pos = origin.offset(
            rand::random_range(-horizontal_dist..=horizontal_dist),
            rand::random_range(-vertical_dist..=vertical_dist),
            rand::random_range(-horizontal_dist..=horizontal_dist),
        );
        if world.get_block_state(pos).is_air()
            && world.get_block_state(pos.below()).is_solid_render()
        {
            let (x, y, z) = pos.get_center();
            return Some(DVec3::new(x, y, z));
        }
    }

    None
}

/// Vanilla `AbstractHorse.canParent`.
#[must_use]
pub(crate) fn can_parent<E: Animal + ?Sized>(entity: &E, data: &AbstractHorseEntityData) -> bool {
    !entity.is_vehicle()
        && !entity.is_passenger()
        && is_tamed(data)
        && !AgeableMob::is_baby(entity)
        && entity.get_health() >= entity.get_max_health()
        && entity.is_in_love()
}

/// Vanilla `Horse.canMate` / `Donkey.canMate`: only `Horse`/`Donkey` partners.
#[must_use]
pub(crate) fn can_mate_horse_or_donkey<E: Animal + ?Sized>(
    entity: &E,
    partner: &dyn Animal,
    data: &AbstractHorseEntityData,
) -> bool {
    if entity.uuid() == partner.uuid() {
        return false;
    }

    let partner_can_parent = if let Some(horse) = partner.downcast_ref::<HorseEntity>() {
        horse.can_parent()
    } else if let Some(donkey) = partner.downcast_ref::<DonkeyEntity>() {
        donkey.can_parent()
    } else {
        return false;
    };

    can_parent(entity, data) && partner_can_parent
}

/// Vanilla `AbstractHorse.finalizeSpawn` group-data default (`0.2` baby chance).
#[must_use]
pub(crate) fn default_horse_group_data(
    group_data: Option<SpawnGroupData>,
) -> Option<SpawnGroupData> {
    group_data.or_else(|| {
        Some(SpawnGroupData::AgeableMob(
            AgeableMobGroupData::with_baby_spawn_chance(0.2),
        ))
    })
}

/// Vanilla `AbstractHorse` legacy equipment keys, kept for older saves.
///
/// Vanilla 1.21+ persists equipment in the `equipment` compound (handled by
/// `save_mob`/`load_mob`). The pre-1.21 `SaddleItem`/`ArmorItem` keys are still
/// written so older worlds round-trip; vanilla ignores unknown keys.
pub(crate) fn save_legacy_equipment<E: LivingEntity + ?Sized>(entity: &E, nbt: &mut NbtCompound) {
    entity.with_equipment_slot(EquipmentSlot::Saddle, &mut |stack| {
        if !stack.is_empty() {
            nbt.insert("SaddleItem", stack.to_nbt_tag_ref());
        }
    });
    entity.with_equipment_slot(EquipmentSlot::Body, &mut |stack| {
        if !stack.is_empty() {
            nbt.insert("ArmorItem", stack.to_nbt_tag_ref());
        }
    });
}

/// Loads the legacy `SaddleItem`/`ArmorItem` keys when `equipment` is absent.
pub(crate) fn load_legacy_equipment<E: LivingEntity + ?Sized>(
    entity: &E,
    nbt: &BorrowedNbtCompoundView<'_, '_>,
) {
    if let Some(stack) = nbt
        .compound("SaddleItem")
        .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
        && !entity.has_item_in_slot(EquipmentSlot::Saddle)
    {
        let mut stack = Some(stack);
        entity.with_equipment_slot_mut(EquipmentSlot::Saddle, &mut |slot| {
            *slot = stack.take().expect("equipment visitor is invoked once");
        });
    }

    if let Some(stack) = nbt
        .compound("ArmorItem")
        .and_then(|compound| ItemStack::from_borrowed_compound(&compound))
        && !entity.has_item_in_slot(EquipmentSlot::Body)
    {
        let mut stack = Some(stack);
        entity.with_equipment_slot_mut(EquipmentSlot::Body, &mut |slot| {
            *slot = stack.take().expect("equipment visitor is invoked once");
        });
    }
}

/// Vanilla `AbstractHorse.dropEquipment` inventory tail: drops every carried
/// stack that does not prevent equipment drops.
///
/// `AbstractChestedHorse.dropEquipment` additionally drops the chest block
/// itself; the chested entities do that after calling this.
pub(crate) fn drop_carried_items<E: LivingEntity + ?Sized>(entity: &E, parts: &AbstractHorseParts) {
    let stacks = {
        let inventory = parts.inventory().lock();
        (0..inventory.size())
            .map(|slot| inventory.get_item(slot).clone())
            .collect::<Vec<_>>()
    };

    for stack in stacks {
        if stack.is_empty()
            || stack.has_enchantment_effect(EnchantmentEffectComponent::PreventEquipmentDrop)
        {
            continue;
        }
        let _ = entity.spawn_at_location(stack, 0.0);
    }
}

/// Shared `Entity::max_up_step` for equines: the `step_height` attribute.
#[must_use]
pub(crate) fn max_up_step<E: LivingEntity + ?Sized>(entity: &E) -> f32 {
    entity
        .attributes()
        .lock()
        .get_value(vanilla_attributes::STEP_HEIGHT)
        .unwrap_or(1.0) as f32
}

/// Vanilla `AbstractHorse` baby passenger attachment for the horse-shaped
/// variants: `attach(PASSENGER, 0, height - 0.125, 0)` then `scale(0.7)`.
#[must_use]
pub(crate) const fn horse_baby_passenger_attachments(
    height: f32,
) -> [steel_registry::entity_type::EntityAttachmentPoint; 1] {
    [steel_registry::entity_type::EntityAttachmentPoint::new(
        0.0,
        (height as f64 - 0.125) * 0.7,
        0.0,
    )]
}

/// Vanilla `AbstractChestedHorse` baby passenger attachment:
/// `attach(PASSENGER, 0, height + 0.03125, -0.3125)` then `scale(0.5)`.
#[must_use]
pub(crate) const fn chested_horse_baby_passenger_attachments(
    height: f32,
) -> [steel_registry::entity_type::EntityAttachmentPoint; 1] {
    [steel_registry::entity_type::EntityAttachmentPoint::new(
        0.0,
        (height as f64 + 0.03125) * 0.5,
        -0.3125 * 0.5,
    )]
}
