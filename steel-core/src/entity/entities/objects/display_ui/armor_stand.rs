//! Vanilla armor stand entity: poses, equipment, damage response, interaction.
//!
//! Vanilla reference: `ArmorStand` (`world/entity/decoration/ArmorStand.java`).
//!
//! Steel builds the stand on the shared [`LivingEntity`] machinery because
//! vanilla armor stands *are* living entities: the generated
//! [`ArmorStandEntityData`] embeds `LivingEntityData` (health, equipment,
//! effects) next to the six synced pose [`Rotations`] and the client-flags
//! byte. Only `living_base`, `get_health` and `set_health` are required
//! implementations; everything else is a vanilla override.
//!
//! Known gaps (reported, not stubbed):
//! - Steel has no `ignoreExplosion` capability query, so the vanilla
//!   "explosions only affect block-like armor stands" rule is not expressible.
//! - Steel has no `thunderHit` hook; lightning leaves armor stands untouched
//!   (which is also vanilla's behavior, so this is inert).
//! - The vanilla death loot table (`entities/armor_stand`) is empty, so
//!   `dropAllDeathLoot` is a no-op beyond the equipment pass implemented here.
//! - `noPhysics` is vanilla's load-time snapshot of `!hasPhysics()`. Steel
//!   keeps that snapshot in sync on NBT load and marker changes only, because
//!   Steel has no per-tick `travel` override to consult `hasPhysics()`.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::data_components::vanilla_components::CUSTOM_NAME;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::entity_data::Rotations;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::{EquipmentSlot, EquipmentSlotType};
use steel_registry::item_stack::ItemStack;
use steel_registry::particle_type::{BlockParticleOption, ParticleData};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::ArmorStandEntityData;
use steel_registry::vanilla_game_rules::MOB_GRIEFING;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_game_events, vanilla_items, vanilla_particle_types,
};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, RemovalReason, SharedEntity,
};
use crate::player::Player;
use crate::world::World;

/// Vanilla `ArmorStand.WOBBLE_TIME`: ticks between a wobble hit and a break.
const WOBBLE_TIME: i64 = 5;
/// Vanilla `ArmorStand.DEFAULT_HEAD_POSE`.
pub const DEFAULT_HEAD_POSE: Rotations = Rotations::new(0.0, 0.0, 0.0);
/// Vanilla `ArmorStand.DEFAULT_BODY_POSE`.
pub const DEFAULT_BODY_POSE: Rotations = Rotations::new(0.0, 0.0, 0.0);
/// Vanilla `ArmorStand.DEFAULT_LEFT_ARM_POSE`.
pub const DEFAULT_LEFT_ARM_POSE: Rotations = Rotations::new(-10.0, 0.0, -10.0);
/// Vanilla `ArmorStand.DEFAULT_RIGHT_ARM_POSE`.
pub const DEFAULT_RIGHT_ARM_POSE: Rotations = Rotations::new(-15.0, 0.0, 10.0);
/// Vanilla `ArmorStand.DEFAULT_LEFT_LEG_POSE`.
pub const DEFAULT_LEFT_LEG_POSE: Rotations = Rotations::new(-1.0, 0.0, -1.0);
/// Vanilla `ArmorStand.DEFAULT_RIGHT_LEG_POSE`.
pub const DEFAULT_RIGHT_LEG_POSE: Rotations = Rotations::new(1.0, 0.0, 1.0);
/// Vanilla `ArmorStand.CLIENT_FLAG_SMALL`.
const CLIENT_FLAG_SMALL: i8 = 1;
/// Vanilla `ArmorStand.CLIENT_FLAG_SHOW_ARMS`.
const CLIENT_FLAG_SHOW_ARMS: i8 = 4;
/// Vanilla `ArmorStand.CLIENT_FLAG_NO_BASEPLATE`.
const CLIENT_FLAG_NO_BASEPLATE: i8 = 8;
/// Vanilla `ArmorStand.CLIENT_FLAG_MARKER`.
const CLIENT_FLAG_MARKER: i8 = 16;
/// Vanilla `ArmorStand.DISABLE_TAKING_OFFSET`.
const DISABLE_TAKING_OFFSET: u32 = 8;
/// Vanilla `ArmorStand.DISABLE_PUTTING_OFFSET`.
const DISABLE_PUTTING_OFFSET: u32 = 16;
/// Vanilla baby eye height (`BABY_DIMENSIONS.withEyeHeight(0.9875F)`).
const BABY_EYE_HEIGHT: f32 = 0.9875;

/// Vanilla armor stand entity.
#[entity_behavior(class = "ArmorStand")]
pub struct ArmorStandEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    entity_data: SyncMutex<ArmorStandEntityData>,
    /// Vanilla `lastHit` game time; `0` matches the field's initial value.
    last_hit: SyncMutex<i64>,
    /// Vanilla `disabledSlots` bitmask.
    disabled_slots: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ArmorStandEntity`.
unsafe impl DowncastType for ArmorStandEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/armor_stand");
}

impl ArmorStandEntity {
    /// Creates an armor stand at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an armor stand from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mut entity_data = ArmorStandEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        Self {
            base,
            entity_type,
            living_base,
            entity_data: SyncMutex::new(entity_data),
            last_hit: SyncMutex::new(0),
            disabled_slots: SyncMutex::new(0),
        }
    }

    /// Vanilla `ArmorStand.setBit` over the synced client-flags byte.
    fn set_client_flag(&self, bit: i8, value: bool) {
        let mut data = self.entity_data.lock();
        let current = *data.client_flags.get();
        let updated = if value { current | bit } else { current & !bit };
        data.client_flags.set(updated);
    }

    /// Vanilla `ArmorStand.hasPhysics`: markers and no-gravity stands skip physics.
    #[must_use]
    pub fn has_physics(&self) -> bool {
        !self.is_marker() && !self.is_no_gravity()
    }

    /// Mirrors vanilla's `noPhysics = !hasPhysics()` snapshot.
    fn sync_no_physics(&self) {
        self.set_no_physics(!self.has_physics());
    }

    /// Returns whether this armor stand renders at half scale.
    #[must_use]
    pub fn is_small(&self) -> bool {
        *self.entity_data.lock().client_flags.get() & CLIENT_FLAG_SMALL != 0
    }

    /// Toggles vanilla's `Small` flag.
    pub fn set_small(&self, small: bool) {
        self.set_client_flag(CLIENT_FLAG_SMALL, small);
        self.refresh_dimensions();
    }

    /// Returns whether this armor stand renders arms.
    #[must_use]
    pub fn show_arms(&self) -> bool {
        *self.entity_data.lock().client_flags.get() & CLIENT_FLAG_SHOW_ARMS != 0
    }

    /// Toggles vanilla's `ShowArms` flag.
    pub fn set_show_arms(&self, show_arms: bool) {
        self.set_client_flag(CLIENT_FLAG_SHOW_ARMS, show_arms);
    }

    /// Returns whether this armor stand renders its base plate.
    #[must_use]
    pub fn show_base_plate(&self) -> bool {
        *self.entity_data.lock().client_flags.get() & CLIENT_FLAG_NO_BASEPLATE == 0
    }

    /// Toggles vanilla's `NoBasePlate` flag.
    pub fn set_no_base_plate(&self, no_base_plate: bool) {
        self.set_client_flag(CLIENT_FLAG_NO_BASEPLATE, no_base_plate);
    }

    /// Returns whether this armor stand is a zero-size marker.
    #[must_use]
    pub fn is_marker(&self) -> bool {
        *self.entity_data.lock().client_flags.get() & CLIENT_FLAG_MARKER != 0
    }

    /// Toggles vanilla's `Marker` flag.
    pub fn set_marker(&self, marker: bool) {
        let changed = self.is_marker() != marker;
        self.set_client_flag(CLIENT_FLAG_MARKER, marker);
        self.sync_no_physics();
        if changed {
            self.refresh_dimensions();
        }
    }

    /// Returns the vanilla `DisabledSlots` bitmask.
    #[must_use]
    pub fn disabled_slots(&self) -> i32 {
        *self.disabled_slots.lock()
    }

    /// Sets the vanilla `DisabledSlots` bitmask.
    pub fn set_disabled_slots(&self, disabled_slots: i32) {
        *self.disabled_slots.lock() = disabled_slots;
    }

    /// Returns vanilla `lastHit` (game time of the last wobble hit).
    #[must_use]
    pub fn last_hit(&self) -> i64 {
        *self.last_hit.lock()
    }

    /// Returns the synced head pose.
    #[must_use]
    pub fn head_pose(&self) -> Rotations {
        *self.entity_data.lock().head_pose.get()
    }

    /// Sets the synced head pose.
    pub fn set_head_pose(&self, pose: Rotations) {
        self.entity_data.lock().head_pose.set(pose);
    }

    /// Returns the synced body pose.
    #[must_use]
    pub fn body_pose(&self) -> Rotations {
        *self.entity_data.lock().body_pose.get()
    }

    /// Sets the synced body pose.
    pub fn set_body_pose(&self, pose: Rotations) {
        self.entity_data.lock().body_pose.set(pose);
    }

    /// Returns the synced left arm pose.
    #[must_use]
    pub fn left_arm_pose(&self) -> Rotations {
        *self.entity_data.lock().left_arm_pose.get()
    }

    /// Sets the synced left arm pose.
    pub fn set_left_arm_pose(&self, pose: Rotations) {
        self.entity_data.lock().left_arm_pose.set(pose);
    }

    /// Returns the synced right arm pose.
    #[must_use]
    pub fn right_arm_pose(&self) -> Rotations {
        *self.entity_data.lock().right_arm_pose.get()
    }

    /// Sets the synced right arm pose.
    pub fn set_right_arm_pose(&self, pose: Rotations) {
        self.entity_data.lock().right_arm_pose.set(pose);
    }

    /// Returns the synced left leg pose.
    #[must_use]
    pub fn left_leg_pose(&self) -> Rotations {
        *self.entity_data.lock().left_leg_pose.get()
    }

    /// Sets the synced left leg pose.
    pub fn set_left_leg_pose(&self, pose: Rotations) {
        self.entity_data.lock().left_leg_pose.set(pose);
    }

    /// Returns the synced right leg pose.
    #[must_use]
    pub fn right_leg_pose(&self) -> Rotations {
        *self.entity_data.lock().right_leg_pose.get()
    }

    /// Sets the synced right leg pose.
    pub fn set_right_leg_pose(&self, pose: Rotations) {
        self.entity_data.lock().right_leg_pose.set(pose);
    }

    /// Returns vanilla `ArmorStand.getArmorStandPose`.
    #[must_use]
    pub fn armor_stand_pose(&self) -> ArmorStandPose {
        ArmorStandPose {
            head: self.head_pose(),
            body: self.body_pose(),
            left_arm: self.left_arm_pose(),
            right_arm: self.right_arm_pose(),
            left_leg: self.left_leg_pose(),
            right_leg: self.right_leg_pose(),
        }
    }

    /// Applies vanilla `ArmorStand.setArmorStandPose`.
    pub fn set_armor_stand_pose(&self, pose: ArmorStandPose) {
        self.set_head_pose(pose.head);
        self.set_body_pose(pose.body);
        self.set_left_arm_pose(pose.left_arm);
        self.set_right_arm_pose(pose.right_arm);
        self.set_left_leg_pose(pose.left_leg);
        self.set_right_leg_pose(pose.right_leg);
    }

    /// Vanilla `LivingEntity.getEquipmentSlotForItem` with the stand's slot gate.
    fn equipment_slot_for_item(&self, item_stack: &ItemStack) -> EquipmentSlot {
        match item_stack.get_equippable() {
            Some(equippable) if self.can_use_slot(equippable.slot) => equippable.slot,
            _ => EquipmentSlot::MainHand,
        }
    }

    /// Vanilla `ArmorStand.isDisabled`: `DisabledSlots` bit plus the arms gate.
    fn is_disabled(&self, slot: EquipmentSlot) -> bool {
        let disabled = self.disabled_slots();
        (disabled & (1 << slot.id())) != 0
            || (slot.slot_type() == EquipmentSlotType::Hand && !self.show_arms())
    }

    /// Vanilla `ArmorStand.getClickedSlot`: picks the slot from the click height.
    ///
    /// `click_y` is relative to the stand's feet, matching the client's
    /// `hitResult.getLocation().subtract(entity.getX(), getY(), getZ())`.
    fn clicked_slot(&self, click_y: f64) -> EquipmentSlot {
        let small = self.is_small();
        let scale = self.get_scale() * self.get_age_scale();
        let click_position = click_y / f64::from(scale.max(f32::EPSILON));
        if click_position >= 0.1
            && click_position < 0.1 + if small { 0.8 } else { 0.45 }
            && self.has_item_in_slot(EquipmentSlot::Feet)
        {
            EquipmentSlot::Feet
        } else if click_position >= 0.9 + if small { 0.3 } else { 0.0 }
            && click_position < 0.9 + if small { 1.0 } else { 0.7 }
            && self.has_item_in_slot(EquipmentSlot::Chest)
        {
            EquipmentSlot::Chest
        } else if click_position >= 0.4
            && click_position < 0.4 + if small { 1.0 } else { 0.8 }
            && self.has_item_in_slot(EquipmentSlot::Legs)
        {
            EquipmentSlot::Legs
        } else if click_position >= 1.6 && self.has_item_in_slot(EquipmentSlot::Head) {
            EquipmentSlot::Head
        } else if !self.has_item_in_slot(EquipmentSlot::MainHand)
            && self.has_item_in_slot(EquipmentSlot::OffHand)
        {
            EquipmentSlot::OffHand
        } else {
            EquipmentSlot::MainHand
        }
    }

    /// Vanilla `ArmorStand.swapItem`.
    ///
    /// `player_stack` is the player's held stack and is updated in place to the
    /// stand's previous item, exactly like vanilla's `player.setItemInHand`.
    fn swap_item(
        &self,
        slot: EquipmentSlot,
        player_stack: &mut ItemStack,
        has_infinite_materials: bool,
    ) -> bool {
        let mut current = ItemStack::empty();
        self.with_equipment_slot(slot, &mut |item| {
            current = item.copy_with_count(item.count());
        });

        let disabled = self.disabled_slots();
        if !current.is_empty()
            && (disabled & (1 << (slot.id() as u32 + DISABLE_TAKING_OFFSET))) != 0
        {
            return false;
        }
        if current.is_empty()
            && (disabled & (1 << (slot.id() as u32 + DISABLE_PUTTING_OFFSET))) != 0
        {
            return false;
        }

        if has_infinite_materials && current.is_empty() && !player_stack.is_empty() {
            let placed = player_stack.copy_with_count(1);
            self.with_equipment_slot_mut(slot, &mut |item| *item = placed.clone());
            return true;
        }

        if player_stack.is_empty() || player_stack.count() <= 1 {
            let placed = player_stack.copy_with_count(player_stack.count());
            self.with_equipment_slot_mut(slot, &mut |item| *item = placed.clone());
            *player_stack = current;
            true
        } else if current.is_empty() {
            let placed = player_stack.split(1);
            self.with_equipment_slot_mut(slot, &mut |item| *item = placed.clone());
            true
        } else {
            false
        }
    }

    /// Vanilla `ArmorStand.interact`.
    fn interact_stand(
        &self,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> InteractionResult {
        let held_item = player.inventory.lock().get_item_in_hand(hand).clone();
        if self.is_marker() || held_item.is(&vanilla_items::NAME_TAG) {
            // Vanilla defers to `super.interact` so the item behavior (naming)
            // can run; Steel's entity-interaction path does the same on `Pass`.
            return InteractionResult::Pass;
        }
        if player.is_spectator() {
            return InteractionResult::Success;
        }

        let target_slot = if held_item.is_empty() {
            let clicked = self.clicked_slot(location.y);
            if self.is_disabled(clicked) {
                self.equipment_slot_for_item(&held_item)
            } else {
                clicked
            }
        } else {
            let slot = self.equipment_slot_for_item(&held_item);
            if self.is_disabled(slot) {
                return InteractionResult::Fail;
            }
            if slot.slot_type() == EquipmentSlotType::Hand && !self.show_arms() {
                return InteractionResult::Fail;
            }
            slot
        };

        if held_item.is_empty() && !self.has_item_in_slot(target_slot) {
            return InteractionResult::Pass;
        }

        let has_infinite_materials = player.has_infinite_materials();
        let mut player_stack = held_item;
        if !self.swap_item(target_slot, &mut player_stack, has_infinite_materials) {
            return InteractionResult::Pass;
        }
        player
            .inventory
            .lock()
            .mutate_item_in_hand(hand, |stack| *stack = player_stack);
        self.refresh_equipment_attribute_modifiers(target_slot);
        InteractionResult::SuccessServer
    }

    /// Vanilla `ArmorStand.hurtServer`.
    fn hurt_stand(&self, world: &World, source: &DamageSource, _amount: f32) -> bool {
        if self.is_removed() {
            return false;
        }
        if !world.get_game_rule(&MOB_GRIEFING)
            && source
                .causing_entity()
                .is_some_and(|entity| entity.is_mob())
        {
            return false;
        }
        if source.is(&DamageTypeTag::BYPASSES_INVULNERABILITY) {
            self.kill_attributed(source.causing_entity().map(AsRef::as_ref));
            return false;
        }
        if self.is_invulnerable_to(world, source) || self.is_invisible() || self.is_marker() {
            return false;
        }

        if source.is(&DamageTypeTag::IS_EXPLOSION) {
            self.broken_by_anything();
            self.kill_attributed(source.causing_entity().map(AsRef::as_ref));
            return false;
        }
        if source.is(&DamageTypeTag::IGNITES_ARMOR_STANDS) {
            if self.is_on_fire() {
                self.cause_damage(0.15);
            } else {
                self.ignite_for_ticks(5 * 20);
            }
            return false;
        }
        if source.is(&DamageTypeTag::BURNS_ARMOR_STANDS) && self.get_health() > 0.5 {
            self.cause_damage(4.0);
            return false;
        }

        let can_break = source.is(&DamageTypeTag::CAN_BREAK_ARMOR_STAND);
        let always_kills = source.is(&DamageTypeTag::ALWAYS_KILLS_ARMOR_STANDS);
        if !can_break && !always_kills {
            return false;
        }
        if let Some(player) = source
            .causing_entity()
            .and_then(|entity| entity.as_player())
            && !player.abilities.lock().may_build
        {
            return false;
        }

        if source.is_creative_player() {
            self.play_broken_sound();
            self.show_breaking_particles();
            self.kill_attributed(source.causing_entity().map(AsRef::as_ref));
            return true;
        }

        let time = world.game_time();
        if time - self.last_hit() > WOBBLE_TIME && !always_kills {
            *self.last_hit.lock() = time;
            self.broadcast_entity_event(EntityStatus::ArmorstandWobble);
            self.game_event_with_source_entity(
                &vanilla_game_events::ENTITY_DAMAGE,
                source.causing_entity().map(AsRef::as_ref),
            );
        } else {
            self.broken_by_player();
            self.show_breaking_particles();
            self.kill_attributed(source.causing_entity().map(AsRef::as_ref));
        }
        true
    }

    /// Vanilla `ArmorStand.causeDamage`: chips the stand's own health pool.
    fn cause_damage(&self, damage: f32) {
        let health = self.get_health() - damage;
        if health <= 0.5 {
            self.broken_by_anything();
            self.kill_attributed(None);
        } else {
            self.set_health(health);
            self.game_event(&vanilla_game_events::ENTITY_DAMAGE);
        }
    }

    /// Vanilla `ArmorStand.brokenByPlayer`: pops the armor stand item.
    fn broken_by_player(&self) {
        let mut stack = ItemStack::new(&vanilla_items::ARMOR_STAND);
        if let Some(custom_name) = self.custom_name() {
            stack.set(CUSTOM_NAME, custom_name);
        }
        self.pop_at_block(self.block_position(), stack);
        self.broken_by_anything();
    }

    /// Vanilla `ArmorStand.brokenByAnything`: break sound plus equipment drops.
    fn broken_by_anything(&self) {
        self.play_broken_sound();
        self.drop_equipment();
    }

    /// Vanilla `ArmorStand.playBrokenSound`.
    fn play_broken_sound(&self) {
        self.play_sound(&sound_events::ENTITY_ARMOR_STAND_BREAK, 1.0, 1.0);
    }

    /// Vanilla `ArmorStand.showBreakingParticles`: 10 oak-plank block particles.
    fn show_breaking_particles(&self) {
        let Some(world) = self.level() else {
            return;
        };
        let position = self.position();
        let dimensions = self.base.dimensions();
        world.send_particles(
            ParticleData::new(
                &vanilla_particle_types::BLOCK,
                BlockParticleOption::new(vanilla_blocks::OAK_PLANKS.default_state()),
            ),
            DVec3::new(
                position.x,
                position.y + f64::from(dimensions.height) * 2.0 / 3.0,
                position.z,
            ),
            10,
            DVec3::new(
                f64::from(dimensions.width) / 4.0,
                f64::from(dimensions.height) / 4.0,
                f64::from(dimensions.width) / 4.0,
            ),
            0.05,
        );
    }

    /// Drops every equipment slot, honoring vanilla's prevent-drop enchantment.
    fn drop_equipment(&self) {
        let block_above = self.block_position().above();
        for slot in EquipmentSlot::ALL {
            let mut can_drop = false;
            self.with_equipment_slot(slot, &mut |item| {
                can_drop = !item.is_empty()
                    && !item
                        .has_enchantment_effect(EnchantmentEffectComponent::PreventEquipmentDrop);
            });
            if !can_drop {
                continue;
            }
            let item = self.living_base.equipment().lock().take(slot);
            self.pop_at_block(block_above, item);
        }
    }

    /// Vanilla `Block.popResource`: spawns the stack at the block's center.
    fn pop_at_block(&self, pos: BlockPos, stack: ItemStack) {
        if stack.is_empty() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };
        let center = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()) + 0.5,
            f64::from(pos.z()) + 0.5,
        );
        let _ = world.spawn_item(center, stack);
    }

    /// Vanilla `ArmorStand.kill(level, attributedTo)`.
    fn kill_attributed(&self, attributed_to: Option<&dyn Entity>) {
        self.set_removed(RemovalReason::Killed);
        self.game_event_with_source_entity(
            &vanilla_game_events::ENTITY_DIE,
            attributed_to.or(Some(self as &dyn Entity)),
        );
    }

    /// Pushes vanilla living-effect flags into synced data before tracking sync.
    fn update_dirty_effect_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }

    /// Reads a vanilla `Rotations` float list (`Pose.<part>`).
    fn read_rotations(nbt: BorrowedNbtCompoundView<'_, '_>, key: &str) -> Option<Rotations> {
        let floats = nbt.list(key)?.floats()?;
        if floats.len() != 3 {
            return None;
        }
        Some(Rotations::new(floats[0], floats[1], floats[2]))
    }

    /// Writes a vanilla `Rotations` float list.
    fn write_rotations(nbt: &mut NbtCompound, key: &str, pose: Rotations) {
        nbt.insert(key, NbtList::Float(vec![pose.x, pose.y, pose.z]));
    }
}

/// Vanilla `ArmorStand.ArmorStandPose`: the six synced body-part rotations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ArmorStandPose {
    /// Head rotation.
    pub head: Rotations,
    /// Torso rotation.
    pub body: Rotations,
    /// Left arm rotation.
    pub left_arm: Rotations,
    /// Right arm rotation.
    pub right_arm: Rotations,
    /// Left leg rotation.
    pub left_leg: Rotations,
    /// Right leg rotation.
    pub right_leg: Rotations,
}

impl ArmorStandPose {
    /// Vanilla `ArmorStandPose.DEFAULT`.
    pub const DEFAULT: Self = Self {
        head: DEFAULT_HEAD_POSE,
        body: DEFAULT_BODY_POSE,
        left_arm: DEFAULT_LEFT_ARM_POSE,
        right_arm: DEFAULT_RIGHT_ARM_POSE,
        left_leg: DEFAULT_LEFT_LEG_POSE,
        right_leg: DEFAULT_RIGHT_LEG_POSE,
    };
}

impl Default for ArmorStandPose {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl Entity for ArmorStandEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        self.base_tick_living_entity();
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        if self.is_marker() {
            EntityDimensions::new(0.0, 0.0, 0.0)
        } else if self.is_small() {
            let mut dimensions = self.entity_type.dimensions.scale(0.5);
            dimensions.eye_height = BABY_EYE_HEIGHT;
            dimensions
        } else {
            self.entity_type.dimensions
        }
    }

    fn refresh_dimensions(&self) {
        // Vanilla `ArmorStand.refreshDimensions` restores the pre-refresh
        // position, cancelling the inherited "fudge position after growing"
        // step, so only the dimensions/pose need to be committed here.
        let pose = self.pose();
        let dimensions = self.dimensions_for_pose(pose);
        self.base().set_pose_and_dimensions(pose, dimensions);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_effect_data();
    }

    fn is_pushable(&self) -> bool {
        false
    }

    fn attackable(&self) -> bool {
        false
    }

    fn is_effective_ai(&self) -> bool {
        self.has_physics() && self.is_server_driven_movement()
    }

    fn blocks_building(&self) -> bool {
        !self.is_marker()
    }

    fn is_marker_armor_stand(&self) -> bool {
        self.is_marker()
    }

    fn is_ignoring_block_triggers(&self) -> bool {
        self.is_marker()
    }

    fn is_pickable(&self) -> bool {
        !self.is_marker() && !self.is_removed()
    }

    fn skip_attack_interaction(&self, source: &SharedEntity) -> bool {
        let Some(player) = source.as_player() else {
            return false;
        };
        source
            .level()
            .is_some_and(|world| !world.may_interact(player, self.block_position()))
    }

    fn fall_sounds(&self) -> (SoundEventRef, SoundEventRef) {
        (
            &sound_events::ENTITY_ARMOR_STAND_FALL,
            &sound_events::ENTITY_ARMOR_STAND_FALL,
        )
    }

    fn interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        location: DVec3,
    ) -> InteractionResult {
        self.interact_stand(player, hand, location)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.hurt_stand(world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_equipment(nbt);
        nbt.insert("Invisible", i8::from(self.is_invisible()));
        nbt.insert("Small", i8::from(self.is_small()));
        nbt.insert("ShowArms", i8::from(self.show_arms()));
        nbt.insert("DisabledSlots", self.disabled_slots());
        nbt.insert("NoBasePlate", i8::from(!self.show_base_plate()));
        if self.is_marker() {
            nbt.insert("Marker", i8::from(true));
        }

        // Vanilla always writes `Pose`; writing it only when non-default keeps
        // the payload identical for untouched stands.
        if self.armor_stand_pose() != ArmorStandPose::DEFAULT {
            let mut pose = NbtCompound::new();
            Self::write_rotations(&mut pose, "Head", self.head_pose());
            Self::write_rotations(&mut pose, "Body", self.body_pose());
            Self::write_rotations(&mut pose, "LeftArm", self.left_arm_pose());
            Self::write_rotations(&mut pose, "RightArm", self.right_arm_pose());
            Self::write_rotations(&mut pose, "LeftLeg", self.left_leg_pose());
            Self::write_rotations(&mut pose, "RightLeg", self.right_leg_pose());
            nbt.insert("Pose", NbtTag::Compound(pose));
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_equipment(nbt);
        self.entity_data
            .set_base_invisible_flag(nbt.byte("Invisible").is_some_and(|value| value != 0));
        self.set_small(nbt.byte("Small").is_some_and(|value| value != 0));
        self.set_show_arms(nbt.byte("ShowArms").is_some_and(|value| value != 0));
        self.set_disabled_slots(nbt.int("DisabledSlots").unwrap_or(0));
        self.set_no_base_plate(nbt.byte("NoBasePlate").is_some_and(|value| value != 0));
        self.set_marker(nbt.byte("Marker").is_some_and(|value| value != 0));

        if let Some(pose) = nbt.compound("Pose") {
            let mut armor_stand_pose = ArmorStandPose::DEFAULT;
            if let Some(head) = Self::read_rotations(pose, "Head") {
                armor_stand_pose.head = head;
            }
            if let Some(body) = Self::read_rotations(pose, "Body") {
                armor_stand_pose.body = body;
            }
            if let Some(left_arm) = Self::read_rotations(pose, "LeftArm") {
                armor_stand_pose.left_arm = left_arm;
            }
            if let Some(right_arm) = Self::read_rotations(pose, "RightArm") {
                armor_stand_pose.right_arm = right_arm;
            }
            if let Some(left_leg) = Self::read_rotations(pose, "LeftLeg") {
                armor_stand_pose.left_leg = left_leg;
            }
            if let Some(right_leg) = Self::read_rotations(pose, "RightLeg") {
                armor_stand_pose.right_leg = right_leg;
            }
            self.set_armor_stand_pose(armor_stand_pose);
        }

        self.sync_no_physics();
    }
}

impl LivingEntity for ArmorStandEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn can_use_slot(&self, slot: EquipmentSlot) -> bool {
        !matches!(slot, EquipmentSlot::Body | EquipmentSlot::Saddle) && !self.is_disabled(slot)
    }

    fn is_baby(&self) -> bool {
        self.is_small()
    }

    fn can_be_seen_by_anyone(&self) -> bool {
        !self.is_invisible() && !self.is_marker()
    }

    fn is_affected_by_potions(&self) -> bool {
        false
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ARMOR_STAND_HIT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ARMOR_STAND_BREAK)
    }

    fn push_entities(&self) {
        // Vanilla `ArmorStand.pushEntities`: only rideable minecarts are pushed,
        // and only within a squared distance of 0.2.
        let Some(world) = self.level() else {
            return;
        };
        let position = self.position();
        for entity in world.get_entities_in_aabb_matching(&self.bounding_box(), |entity| {
            entity.entity_type().is_abstract_minecart
        }) {
            if entity.distance_to_sqr(position) <= 0.2 {
                self.push_entity(&*entity);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use simdnbt::owned::NbtCompound;
    use std::io::Cursor;
    use steel_registry::vanilla_entities;

    fn new_stand() -> ArmorStandEntity {
        steel_registry::init_vanilla_registry();
        ArmorStandEntity::new(&vanilla_entities::ARMOR_STAND, 1, DVec3::ZERO, Weak::new())
    }

    /// Re-borrows owned NBT the way entity loading does from disk.
    fn with_reborrow<R>(
        nbt: &NbtCompound,
        visitor: impl FnOnce(BorrowedNbtCompoundView<'_, '_>) -> R,
    ) -> R {
        use simdnbt::borrow::read_compound;
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = Cursor::new(bytes.as_slice());
        let base = read_compound(&mut cursor).expect("entity NBT should reborrow");
        let borrowed: BorrowedNbtCompoundView<'_, '_> = (&base).into();
        visitor(borrowed)
    }

    #[test]
    fn defaults_match_vanilla_poses() {
        let stand = new_stand();
        assert_eq!(stand.armor_stand_pose(), ArmorStandPose::DEFAULT);
        assert!(!stand.is_small());
        assert!(!stand.show_arms());
        assert!(stand.show_base_plate());
        assert!(!stand.is_marker());
        assert!(stand.has_physics());
    }

    #[test]
    fn client_flags_round_trip_through_nbt() {
        let stand = new_stand();
        stand.set_small(true);
        stand.set_show_arms(true);
        stand.set_no_base_plate(true);
        stand.set_marker(true);
        stand.set_disabled_slots(0b1_0000_0000);
        stand.set_head_pose(Rotations::new(1.0, 2.0, 3.0));

        let mut nbt = NbtCompound::new();
        stand.save_additional(&mut nbt);
        assert_eq!(nbt.byte("Small"), Some(1));
        assert_eq!(nbt.byte("ShowArms"), Some(1));
        assert_eq!(nbt.byte("NoBasePlate"), Some(1));
        assert_eq!(nbt.byte("Marker"), Some(1));
        assert_eq!(nbt.int("DisabledSlots"), Some(0b1_0000_0000));

        let reloaded = new_stand();
        with_reborrow(&nbt, |borrowed| reloaded.load_additional(borrowed));
        assert!(reloaded.is_small());
        assert!(reloaded.show_arms());
        assert!(!reloaded.show_base_plate());
        assert!(reloaded.is_marker());
        assert_eq!(reloaded.disabled_slots(), 0b1_0000_0000);
        assert_eq!(reloaded.head_pose(), Rotations::new(1.0, 2.0, 3.0));
        assert_eq!(reloaded.body_pose(), DEFAULT_BODY_POSE);
    }

    #[test]
    fn marker_and_small_dimensions_match_vanilla() {
        let stand = new_stand();
        assert_eq!(
            stand.dimensions_for_pose(EntityPose::Standing),
            vanilla_entities::ARMOR_STAND.dimensions
        );

        stand.set_small(true);
        let baby = stand.dimensions_for_pose(EntityPose::Standing);
        assert!((baby.width - 0.25).abs() < 1.0e-6);
        assert!((baby.height - 0.9875).abs() < 1.0e-6);
        assert!((baby.eye_height - BABY_EYE_HEIGHT).abs() < 1.0e-6);

        stand.set_marker(true);
        let marker = stand.dimensions_for_pose(EntityPose::Standing);
        assert_eq!(marker.width, 0.0);
        assert_eq!(marker.height, 0.0);
        assert!(!stand.has_physics());
        assert!(stand.no_physics());
    }

    #[test]
    fn disabled_slots_follow_vanilla_bit_layout() {
        let stand = new_stand();
        stand.set_show_arms(true);
        // Bit 0 is MAINHAND.
        stand.set_disabled_slots(1);
        assert!(stand.is_disabled(EquipmentSlot::MainHand));
        assert!(!stand.is_disabled(EquipmentSlot::Head));
        // Bit 8 disables taking from the main hand.
        stand.set_disabled_slots(1 << 8);
        assert!(!stand.is_disabled(EquipmentSlot::MainHand));
        // Hands are always disabled while arms are hidden.
        stand.set_show_arms(false);
        assert!(stand.is_disabled(EquipmentSlot::OffHand));
        assert!(!stand.is_disabled(EquipmentSlot::Head));
    }

    #[test]
    fn pose_nbt_uses_vanilla_float_lists() {
        let stand = new_stand();
        stand.set_left_arm_pose(Rotations::new(-90.0, 0.0, 0.0));
        let mut nbt = NbtCompound::new();
        stand.save_additional(&mut nbt);

        let pose = nbt.compound("Pose").expect("pose compound");
        let left_arm = pose.list("LeftArm").expect("left arm list");
        assert_eq!(left_arm.floats(), Some(vec![-90.0, 0.0, 0.0]));
        assert!(
            pose.list("Head").is_some(),
            "vanilla writes every pose part once Pose is present"
        );
    }
}
