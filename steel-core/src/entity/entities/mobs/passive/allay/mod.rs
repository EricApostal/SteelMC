//! Vanilla `Allay`: a flying helper that carries a duplicate of one item, is
//! lured by note blocks and duplicates on amethyst chimes while dancing.
//!
//! Scope: the entity shell (synced dancing/duplication flags, single-slot
//! `InventoryCarrier`, duplication cooldown, item give/take interaction,
//! inventory pickup, sounds), the vanilla AI-step upkeep and the `AllayAi`
//! brain provider in [`brain`]. The noteblock/jukebox vibration listeners and
//! the flying navigation are **not** implemented here; they are listed as gaps
//! in the wave report.

pub(crate) mod brain;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::{CTakeItemEntity, SoundSource};
use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::AllayEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_entities,
    vanilla_game_rules,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, ChunkPos, Downcast as _, DowncastType, DowncastTypeKey};
use uuid::Uuid;

use crate::behavior::InteractionResult;
use crate::entity::ai::brain::Brain;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::items::ItemEntity;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    ENTITIES, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySyncedData, LivingEntity,
    LivingEntityBase, Mob, MobBase, PathfinderMob, RemovalReason, SharedEntity,
};
use crate::inventory::container::Container as _;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::player::player_inventory::Prediction;
use crate::world::World;

/// Vanilla `Allay.ITEM_PICKUP_REACH`.
const ITEM_PICKUP_REACH: DVec3 = DVec3::new(1.0, 1.0, 1.0);
/// Vanilla `Allay.DEFAULT_DUPLICATION_COOLDOWN`.
const DEFAULT_DUPLICATION_COOLDOWN: i64 = 0;
/// Vanilla `Allay.DUPLICATION_COOLDOWN_TICKS`.
const DUPLICATION_COOLDOWN_TICKS: i64 = 6000;
/// Vanilla `Allay.MAX_NOTEBLOCK_DISTANCE`.
const MAX_NOTEBLOCK_DISTANCE: f64 = 1024.0;
/// Vanilla `Allay.createAttributes` movement-speed attribute default (`0.1`).
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `GameEvent.JUKEBOX_PLAY`/`JUKEBOX_STOP_PLAY` notification radius.
const JUKEBOX_NOTIFICATION_RADIUS: f64 = 10.0;
/// Vanilla `Allay.aiStep` heal cadence and amount.
const HEAL_INTERVAL_TICKS: i32 = 10;
const HEAL_AMOUNT: f32 = 1.0;
/// Vanilla `Allay.aiStep` stop-dancing cadence.
const STOP_DANCING_INTERVAL_TICKS: i32 = 20;

#[entity_behavior(class = "Allay")]
/// Vanilla allay with dancing state, duplication clock and carried item.
pub struct AllayEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<AllayEntityData>,
    /// Vanilla `InventoryCarrier` (`Allay.inventory`, a single slot).
    inventory: SyncMutex<InventoryCarrier>,
    /// Vanilla `Mob.brain`; the provider is wired in `allay/brain.rs`.
    brain: SyncMutex<Brain>,
    /// Vanilla `Allay.jukeboxPos`: the jukebox this allay is dancing to.
    jukebox_pos: SyncMutex<Option<BlockPos>>,
    /// Vanilla `Allay.duplicationCooldown`.
    duplication_cooldown: SyncMutex<i64>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `AllayEntity`.
unsafe impl DowncastType for AllayEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/allay");
}

impl AllayEntity {
    /// Creates a new allay at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an allay from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        // Vanilla `Allay.createNavigation` -> `FlyingPathNavigation`
        // (`Allay.java:136`).
        mob_base.navigation().lock().set_flying(true);
        let mut entity_data = AllayEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        let entity = Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            inventory: SyncMutex::new(InventoryCarrier::new(1)),
            brain: SyncMutex::new(Brain::default()),
            jukebox_pos: SyncMutex::new(None),
            duplication_cooldown: SyncMutex::new(DEFAULT_DUPLICATION_COOLDOWN),
        };
        // Vanilla `Allay.makeBrain` -> `BRAIN_PROVIDER.makeBrain`.
        {
            let mut brain = entity.brain.lock();
            *brain = brain::make_brain(&entity);
            // `GoToWantedItem` reads the cooldown this entity's own pickup gate
            // checks; registering it keeps both halves in sync.
            brain.register_memory(&mem::ITEM_PICKUP_COOLDOWN_TICKS);
        }
        entity
    }

    /// Vanilla `Allay.getInventory`.
    #[must_use]
    pub fn get_inventory(&self) -> &SyncMutex<InventoryCarrier> {
        &self.inventory
    }

    /// Returns the carried items (vanilla `InventoryCarrier`).
    #[must_use]
    pub fn carried_items(&self) -> &SyncMutex<InventoryCarrier> {
        &self.inventory
    }

    /// Vanilla `Allay.isDancing`.
    #[must_use]
    pub fn is_dancing(&self) -> bool {
        *self.entity_data.lock().dancing.get()
    }

    /// Vanilla `Allay.setDancing`: ignored on the client, when the allay has no
    /// effective AI, or when it is panicking.
    pub fn set_dancing(&self, is_dancing: bool) {
        if !self.is_effective_ai() || (is_dancing && PathfinderMob::is_panicking(self)) {
            return;
        }
        self.entity_data.lock().dancing.set(is_dancing);
    }

    /// Vanilla `Allay.canDuplicate` (the synced `DATA_CAN_DUPLICATE` flag).
    #[must_use]
    pub fn can_duplicate(&self) -> bool {
        *self.entity_data.lock().can_duplicate.get()
    }

    /// Vanilla `Allay.duplicationCooldown`.
    #[must_use]
    pub fn duplication_cooldown(&self) -> i64 {
        *self.duplication_cooldown.lock()
    }

    /// Vanilla `Allay.setDuplicationCooldown`.
    pub fn set_duplication_cooldown(&self, duplication_cooldown: i64) {
        *self.duplication_cooldown.lock() = duplication_cooldown;
        self.entity_data
            .lock()
            .can_duplicate
            .set(duplication_cooldown == 0);
    }

    /// Vanilla `Allay.resetDuplicationCooldown`.
    pub fn reset_duplication_cooldown(&self) {
        self.set_duplication_cooldown(DUPLICATION_COOLDOWN_TICKS);
    }

    /// Vanilla `Allay.hasItemInHand`.
    #[must_use]
    pub fn has_item_in_hand(&self) -> bool {
        !self.equipment_in_slot(EquipmentSlot::MainHand).is_empty()
    }

    /// Vanilla `Allay.jukeboxPos`.
    #[must_use]
    pub fn jukebox_pos(&self) -> Option<BlockPos> {
        *self.jukebox_pos.lock()
    }

    /// Vanilla `Allay.setJukeboxPlaying`: starts or stops dancing for `jukebox`.
    pub fn set_jukebox_playing(&self, jukebox: BlockPos, is_playing: bool) {
        if is_playing {
            if !self.is_dancing() {
                *self.jukebox_pos.lock() = Some(jukebox);
                self.set_dancing(true);
            }
            return;
        }

        let current = *self.jukebox_pos.lock();
        if current == Some(jukebox) || current.is_none() {
            *self.jukebox_pos.lock() = None;
            self.set_dancing(false);
        }
    }

    /// Vanilla `Allay.MAX_NOTEBLOCK_DISTANCE` reach check used by the noteblock
    /// memory (`LIKED_NOTEBLOCK_POSITION`).
    #[must_use]
    pub fn is_close_enough_to_noteblock(&self, pos: BlockPos) -> bool {
        let me = self.block_position();
        let dx = (pos.x() - me.x()) as f64;
        let dy = (pos.y() - me.y()) as f64;
        let dz = (pos.z() - me.z()) as f64;
        dx * dx + dy * dy + dz * dz <= MAX_NOTEBLOCK_DISTANCE * MAX_NOTEBLOCK_DISTANCE
    }

    /// Vanilla `Allay.shouldStopDancing`: the jukebox is gone, out of range or
    /// no longer a jukebox.
    fn should_stop_dancing(&self) -> bool {
        let Some(jukebox_pos) = *self.jukebox_pos.lock() else {
            return true;
        };
        if !jukebox_pos.closer_to_center_than(self.position(), JUKEBOX_NOTIFICATION_RADIUS) {
            return true;
        }

        use steel_registry::blocks::block_state_ext::BlockStateExt as _;
        let Some(world) = self.level() else {
            return true;
        };
        world.get_block_state(jukebox_pos).get_block() != &steel_registry::vanilla_blocks::JUKEBOX
    }

    /// Vanilla `Allay.isOnPickupCooldown`: `ITEM_PICKUP_COOLDOWN_TICKS` present.
    fn is_on_pickup_cooldown(&self) -> bool {
        self.brain
            .lock()
            .has_memory_value(&mem::ITEM_PICKUP_COOLDOWN_TICKS)
    }

    /// Vanilla `Allay.isLikedPlayer`: the `LIKED_PLAYER` memory holds this uuid.
    fn is_liked_player(&self, uuid: Option<Uuid>) -> bool {
        let Some(uuid) = uuid else {
            return false;
        };
        self.brain
            .lock()
            .memories()
            .get(&mem::LIKED_PLAYER)
            .is_some_and(|liked| liked == uuid)
    }

    /// Vanilla `Allay.updateDuplicationCooldown`.
    fn update_duplication_cooldown(&self) {
        let cooldown = self.duplication_cooldown.lock();
        if *cooldown > 0 {
            let next = *cooldown - 1;
            drop(cooldown);
            self.set_duplication_cooldown(next);
        }
    }

    /// Vanilla `Allay.duplicateAllay`: spawn an allay copy and reset both clocks.
    fn duplicate_allay(&self) {
        let Some(world) = self.level() else {
            return;
        };

        let Some(allay) = ENTITIES.create(
            &vanilla_entities::ALLAY,
            crate::entity::next_entity_id(),
            self.position(),
            Arc::downgrade(&world),
        ) else {
            return;
        };

        if let Some(allay) = allay.downcast_ref::<AllayEntity>() {
            allay.set_persistence_required();
            allay.reset_duplication_cooldown();
        }
        self.reset_duplication_cooldown();
        let _ = world.try_add_entity(allay);
    }

    /// Vanilla `Allay.removeInteractionItem`: consume one of the player's held items.
    fn remove_interaction_item(&self, player: &Player, hand: InteractionHand) {
        player
            .inventory
            .lock()
            .get_item_in_hand_mut(hand)
            .consume(1, player.has_infinite_materials());
    }

    /// Vanilla `Allay.allayConsidersItemEqual` + `hasNonMatchingPotion`.
    #[must_use]
    pub fn allay_considers_item_equal(item1: &ItemStack, item2: &ItemStack) -> bool {
        if !ItemStack::is_same_item(item1, item2) {
            return false;
        }

        item1.get(POTION_CONTENTS) == item2.get(POTION_CONTENTS)
    }

    /// Vanilla `BehaviorUtils.throwItem` from this allay toward `target_pos`.
    fn throw_item_from_here(&self, item: ItemStack, target_pos: DVec3) {
        /// Vanilla `BehaviorUtils.throwItem`'s `throwVelocity`.
        const THROW_VELOCITY: f64 = 0.3;
        /// Vanilla `BehaviorUtils.throwItem`'s `handYDistanceFromEye`.
        const HAND_Y_DISTANCE_FROM_EYE: f64 = 0.3;

        if item.is_empty() {
            return;
        }
        let Some(world) = self.level() else {
            return;
        };

        let mut velocity = target_pos - self.position();
        velocity = if velocity.length_squared() < 1.0e-5 {
            DVec3::ZERO
        } else {
            velocity.normalize() * THROW_VELOCITY
        };

        let position = DVec3::new(
            self.position().x,
            self.get_eye_y() - HAND_Y_DISTANCE_FROM_EYE,
            self.position().z,
        );

        let Some(item_entity) = world.spawn_item_with_velocity(position, item, velocity) else {
            return;
        };
        item_entity.set_thrower(self.uuid());
        item_entity.set_default_pickup_delay();
    }

    fn update_dirty_mob_effect_entity_data(&self) {
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
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }
}

impl Entity for AllayEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        Mob::base_tick_mob(self);
    }

    fn dimensions_for_pose(&self, _pose: EntityPose) -> EntityDimensions {
        let scale = LivingEntity::get_scale(self);
        if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        self.attributes()
            .lock()
            .get_value(vanilla_attributes::STEP_HEIGHT)
            .unwrap_or(f64::from(DEFAULT_STEP_HEIGHT)) as f32
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    /// Vanilla `Allay.playStepSound`: allays are silent on their feet.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    /// Vanilla `Allay.isFlapping`: always airborne-flapping off the ground.
    fn is_flapping(&self) -> bool {
        !self.on_ground()
    }

    /// Vanilla `Allay.checkFallDamage`: allays never take fall damage.
    fn check_fall_damage(
        self: Arc<Self>,
        _vertical_movement: f64,
        _on_ground: bool,
        _on_state: BlockStateId,
        _pos: BlockPos,
        _world: &Arc<World>,
    ) {
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.inventory.lock().write_to_tag(nbt);
        nbt.insert("DuplicationCooldown", self.duplication_cooldown());
        // GAP(allay-vibration-data): vanilla also stores the `listener`
        // `VibrationSystem.Data`. Steel has no vibration system yet, so that key
        // is not written.
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.inventory.lock().read_from_tag(&nbt);
        self.set_duplication_cooldown(
            nbt.int("DuplicationCooldown")
                .map_or(DEFAULT_DUPLICATION_COOLDOWN, i64::from),
        );
    }

    /// Vanilla `Allay.hurtServer`: damage from the liked player is refused.
    ///
    /// Steel hooks this on `Entity::hurt` rather than
    /// `LivingEntity::hurt_server`: that method has no `default_*` companion,
    /// and `LivingEntity::hurt_server(self, …)` from inside the override would
    /// resolve to itself and recurse forever.
    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_liked_player(source.causing_entity().map(|entity| entity.uuid())) {
            return false;
        }

        LivingEntity::hurt_server(self, world, source, amount)
    }

    /// Vanilla `Allay.considersEntityAsAlly`: the liked player, or the base
    /// (non-allied) default. The `Entity::is_allied_to` default body returns
    /// `false`; it is inlined here because a UFCS call would recurse.
    fn is_allied_to(&self, other: &dyn Entity) -> bool {
        self.is_liked_player(Some(other.uuid()))
    }
}

impl LivingEntity for AllayEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        // Vanilla `Allay.aiStep`.
        if Entity::is_alive(self) && self.tick_count() % HEAL_INTERVAL_TICKS == 0 {
            self.heal(HEAL_AMOUNT);
        }

        if self.is_dancing()
            && self.should_stop_dancing()
            && self.tick_count() % STOP_DANCING_INTERVAL_TICKS == 0
        {
            self.set_dancing(false);
            *self.jukebox_pos.lock() = None;
        }

        self.update_duplication_cooldown();
        result
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

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ALLAY_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_ALLAY_DEATH)
    }
}

impl Mob for AllayEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn brain(&self) -> Option<&SyncMutex<Brain>> {
        Some(&self.brain)
    }

    /// Vanilla `Allay.customServerAiStep`: tick `AllayAi`, which includes
    /// `AllayAi.updateActivity`.
    fn brain_tick(&self, entity: &SharedEntity) {
        let Some(world) = self.level() else {
            return;
        };

        let mut brain = self.brain.lock();
        brain::tick(&mut brain, &world, entity);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `Allay.getAmbientSound`: distinguishes carrying an item.
    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.has_item_in_hand() {
            &sound_events::ENTITY_ALLAY_AMBIENT_WITH_ITEM
        } else {
            &sound_events::ENTITY_ALLAY_AMBIENT_WITHOUT_ITEM
        })
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    /// Vanilla `Allay.canPickUpLoot`.
    fn can_pick_up_loot(&self) -> bool {
        !self.is_on_pickup_cooldown() && self.has_item_in_hand()
    }

    /// Vanilla `Allay.getPickupReach`.
    fn get_pickup_reach(&self) -> DVec3 {
        ITEM_PICKUP_REACH
    }

    /// Vanilla `Allay.wantsToPickUp`: a single carried item kind, mob-griefing
    /// gated, and only while the inventory slot is free.
    fn wants_to_pick_up(&self, item_stack: &ItemStack) -> bool {
        let item_in_hand = self.equipment_in_slot(EquipmentSlot::MainHand);
        if item_in_hand.is_empty() {
            return false;
        }

        let mob_griefing = self
            .level()
            .is_none_or(|world| world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING));
        mob_griefing
            && self.inventory.lock().can_add_item(item_stack)
            && AllayEntity::allay_considers_item_equal(&item_in_hand, item_stack)
    }

    /// Vanilla `Allay.pickUpItem` through `InventoryCarrier.pickUpItem`.
    fn pick_up_item(&self, world: &Arc<World>, item_entity: &ItemEntity) {
        let item_stack = item_entity.get_item();
        if !self.wants_to_pick_up(&item_stack) {
            return;
        }

        let count = item_stack.count();
        let mut inventory = self.inventory.lock();
        if !inventory.can_add_item(&item_stack) {
            return;
        }
        let remainder = inventory.add_item(&item_stack);
        drop(inventory);

        world.broadcast_to_nearby(
            ChunkPos::from_entity_pos(item_entity.position()),
            CTakeItemEntity::new(item_entity.id(), self.id(), count - remainder.count()),
            None,
        );

        if remainder.is_empty() {
            item_entity.set_removed(RemovalReason::Discarded);
        } else {
            item_entity.set_item(remainder);
        }
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let interaction_item = {
            let inventory = player.inventory.lock();
            let stack = inventory.get_item_in_hand(hand);
            stack.copy_with_count(stack.count())
        };
        let item_in_hand = self.equipment_in_slot(EquipmentSlot::MainHand);

        // Vanilla `Allay.mobInteract`: amethyst-chime duplication while dancing.
        let duplicates_allays = {
            let Some(world) = self.level() else {
                return InteractionResult::Pass;
            };
            REGISTRY
                .items
                .is_in_tag(interaction_item.item(), &ItemTag::DUPLICATES_ALLAYS)
                && self.is_dancing()
                && self.can_duplicate()
                && {
                    world.play_sound(
                        &sound_events::BLOCK_AMETHYST_BLOCK_CHIME,
                        SoundSource::Neutral,
                        self.block_position(),
                        2.0,
                        1.0,
                        None,
                    );
                    true
                }
        };
        if duplicates_allays {
            self.duplicate_allay();
            self.remove_interaction_item(player, hand);
            return InteractionResult::Success;
        }

        // Give this allay the player's item.
        if item_in_hand.is_empty() && !interaction_item.is_empty() {
            self.set_item_in_hand(
                InteractionHand::MainHand,
                interaction_item.copy_with_count(1),
            );
            self.remove_interaction_item(player, hand);
            if let Some(world) = self.level() {
                world.play_sound(
                    &sound_events::ENTITY_ALLAY_ITEM_GIVEN,
                    SoundSource::Neutral,
                    self.block_position(),
                    2.0,
                    1.0,
                    None,
                );
            }
            self.brain
                .lock()
                .set_memory(&mem::LIKED_PLAYER, Some(player.uuid()));
            return InteractionResult::Success;
        }

        // Take the carried item (and everything in the inventory) back.
        if !item_in_hand.is_empty()
            && hand == InteractionHand::MainHand
            && interaction_item.is_empty()
        {
            self.set_item_in_hand(InteractionHand::MainHand, ItemStack::empty());
            if let Some(world) = self.level() {
                world.play_sound(
                    &sound_events::ENTITY_ALLAY_ITEM_TAKEN,
                    SoundSource::Neutral,
                    self.block_position(),
                    2.0,
                    1.0,
                    None,
                );
            }
            let animation = self.interact_animation(InteractionHand::MainHand);
            self.swing(InteractionHand::MainHand, animation, true);

            let removed = {
                let mut inventory = self.inventory.lock();
                let removed: Vec<ItemStack> = inventory
                    .inventory()
                    .items()
                    .iter()
                    .filter(|stack| !stack.is_empty())
                    .cloned()
                    .collect();
                inventory.clear();
                removed
            };
            for stack in removed {
                let _ = self.spawn_at_location(stack, 0.0);
            }

            self.brain.lock().erase_memory(&mem::LIKED_PLAYER);
            player.add_item_or_drop(item_in_hand, Prediction::ServerOnly);
            return InteractionResult::Success;
        }

        InteractionResult::Pass
    }
}

impl PathfinderMob for AllayEntity {}
