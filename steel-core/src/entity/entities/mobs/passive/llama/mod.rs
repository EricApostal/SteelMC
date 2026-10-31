//! Vanilla llama family: `Llama` (chested storage) and `TraderLlama` (the
//! wandering trader's leashed llama with its own despawn clock).
//!
//! Scope: chested storage (`ChestedHorse` + `Items` NBT, strength-sized
//! inventory), strength/variant synced data, llama sounds, the full
//! `AbstractChestedHorse.mobInteract` chain (chest equip, feeding, taming,
//! riding), the shared `AbstractHorse` mount inventory screen
//! (`ClientboundMountScreenOpenPacket` + `mount_menu`) and trader
//! despawn/leash behaviour.
//!
//! The remaining vanilla llama behaviour (breeding offspring identity, carpet
//! decor) is **not** implemented here and is listed as a gap in the wave report.
//! Caravan membership (`LlamaFollowCaravanGoal` + [`CaravanLlama`]) and the
//! spit ranged attack (`RangedAttackGoal` + `LlamaSpit`) are implemented.
//!
//! # Riding
//!
//! Vanilla `Llama` does not override any `AbstractHorse` riding method, so the
//! shared equine helpers (`tick_ridden`, `ridden_input`, `ridden_speed`,
//! `controlling_passenger`, `RunAroundLikeCrazyGoal`) apply unchanged: an
//! untamed llama bucks its rider and slowly builds temper until it is tamed,
//! exactly like the other equines.

mod trader;

pub use trader::TraderLlamaEntity;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::entity_variant::LlamaVariant;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_attributes;
use steel_registry::vanilla_entity_data::{AbstractHorseEntityData, LlamaEntityData};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_entities, vanilla_items,
};
use steel_utils::locks::{Shared, SyncMutex};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BreedGoal, CaravanLlama, FloatGoal, FollowParentGoal, LlamaFollowCaravanGoal, LookAtPlayerGoal,
    PanicGoal, RandomLookAroundGoal, RangedAttackGoal, RangedAttackMob, TemptGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::passive::equine::{
    AbstractHorseParts, Equine, FLAG_TAME, RunAroundLikeCrazyGoal, abstract_horse_mob_interact,
    can_parent, controlling_passenger, drop_carried_items, eating, equip_chest, equip_sound,
    is_immobile_from_state, is_tamed, load_legacy_equipment, make_mad,
    open_custom_inventory_screen, ridden_input, ridden_speed, save_legacy_equipment, set_flag,
    stand_if_possible, tame_with_name, tick_abstract_horse, tick_ridden,
};
use crate::entity::entities::objects::projectiles::LlamaSpitEntity;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::projectile::{Projectile, spawn_projectile};
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Llama.MAX_STRENGTH`.
const MAX_STRENGTH: i32 = 5;
/// Vanilla `Llama.getMaxTemper` (vanilla `AbstractHorse` uses 100).
const LLAMA_MAX_TEMPER: i32 = 30;
/// Vanilla `AbstractChestedHorse` baby passenger attachment/scale.
const LLAMA_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 1.87 - 0.25, -0.3)];

/// Vanilla `Llama.createAttributes` movement speed (`AbstractChestedHorse`).
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

/// Vanilla `Llama.spit`: `target.getY(0.3333F)` partial-tick interpolation.
const SPIT_TARGET_PARTIAL_TICKS: f64 = 0.333_333_333_333_333_3;
/// Vanilla `Llama.spit`: `sqrt(xd * xd + zd * zd) * 0.2`.
const SPIT_TARGET_Y_SCALE: f64 = 0.2;
/// Vanilla `Llama.spit`: `Projectile.spawnProjectileUsingShoot(..., 1.5F, ...)`.
const SPIT_POWER: f32 = 1.5;
/// Vanilla `Llama.rangedAttackUncertainty`.
const SPIT_UNCERTAINTY: f32 = 10.0;
/// Vanilla `Llama.spit` sound pitch jitter `(nextFloat - nextFloat) * 0.2F`.
const SPIT_SOUND_PITCH_JITTER: f32 = 0.2;

const LLAMA_BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    0.9 * 0.5,
    1.87 * 0.5,
    1.7765 * 0.5,
    EntityAttachments::new(&LLAMA_BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

#[entity_behavior(class = "Llama")]
/// Vanilla llama with chested storage, strength and variant.
pub struct LlamaEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    entity_data: SyncMutex<LlamaEntityData>,
    /// Vanilla `AbstractHorse` non-synchronized state: the mount inventory
    /// (`AbstractHorse.inventory`), temper, owner and the animation counters.
    parts: AbstractHorseParts,
    /// Vanilla `Llama.caravanHead`. Weak so a caravan never keeps a removed llama alive.
    caravan_head: SyncMutex<Option<Weak<dyn Entity>>>,
    /// Vanilla `Llama.caravanTail`. Weak for the same reason.
    caravan_tail: SyncMutex<Option<Weak<dyn Entity>>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `LlamaEntity`.
unsafe impl DowncastType for LlamaEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/llama");
}

impl LlamaEntity {
    /// Creates a new llama at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base_inner(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a llama from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base_inner(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base_inner(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = LlamaEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `Llama.registerGoals`; it does not call `AbstractHorse.registerGoals`,
            // so there is no `MountPanicGoal` here.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, RunAroundLikeCrazyGoal::new(1.2));
            goal_selector.add_goal(2, LlamaFollowCaravanGoal::new(2.1));
            goal_selector.add_goal(3, RangedAttackGoal::new(1.25, 40, 20.0));
            goal_selector.add_goal(3, PanicGoal::new(1.2));
            goal_selector.add_goal(4, BreedGoal::new(1.0));
            goal_selector.add_goal(
                5,
                TemptGoal::new(
                    1.25,
                    |item_stack| {
                        REGISTRY
                            .items
                            .is_in_tag(item_stack.item(), &ItemTag::LLAMA_TEMPT_ITEMS)
                    },
                    false,
                ),
            );
            goal_selector.add_goal(6, FollowParentGoal::new(1.0));
            goal_selector.add_goal(7, WaterAvoidingRandomStrollGoal::new(0.7));
            goal_selector.add_goal(8, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(9, RandomLookAroundGoal::new());
        }

        // Vanilla `AbstractChestedHorse` sets `canGallop = false`.
        let mut parts = AbstractHorseParts::new(false);
        parts.set_max_temper(LLAMA_MAX_TEMPER);
        parts.create_inventory(0);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            parts,
            caravan_head: SyncMutex::new(None),
            caravan_tail: SyncMutex::new(None),
        }
    }

    /// Vanilla `Llama.isTraderLlama`; overridden by [`TraderLlamaEntity`].
    #[must_use]
    pub const fn is_trader_llama() -> bool {
        false
    }

    /// Builds a llama body from an existing base, used by the trader llama.
    pub(crate) fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        Self::new_with_base_inner(base, entity_type)
    }

    /// Vanilla `Llama.getStrength`.
    #[must_use]
    pub fn strength(&self) -> i32 {
        *self.entity_data.lock().strength.get()
    }

    /// Vanilla `Llama.setStrength`: clamped to `1..=5`.
    pub fn set_strength(&self, strength: i32) {
        self.entity_data
            .lock()
            .strength
            .set(strength.clamp(1, MAX_STRENGTH));
    }

    /// Vanilla `AbstractChestedHorse.randomizeAttributes`:
    /// `generateMaxHealth(random::nextInt)` = `15 + random(8) + random(9)`.
    ///
    /// Vanilla does this in the entity constructor; Steel's attribute state is not
    /// persisted per instance, so the llama rolls its spawn health here.
    fn randomize_max_health(&self) {
        let max_health =
            15.0 + f64::from(rand::random_range(0..8)) + f64::from(rand::random_range(0..9));
        self.attributes()
            .lock()
            .set_base_value(vanilla_attributes::MAX_HEALTH, max_health);
    }

    /// Vanilla `Llama.setRandomStrength`: `random.nextFloat() < 0.04 ? 5 : 3`.
    fn set_random_strength(&self) {
        let max_strength = if rand::random::<f32>() < 0.04 {
            MAX_STRENGTH
        } else {
            3
        };
        self.set_strength(1 + rand::random_range(0..max_strength));
    }

    /// Vanilla `Llama.getVariant`.
    #[must_use]
    pub fn variant(&self) -> LlamaVariant {
        LlamaVariant::by_id(*self.entity_data.lock().variant.get())
    }

    /// Vanilla `Llama.setVariant`.
    pub fn set_variant(&self, variant: LlamaVariant) {
        self.entity_data.lock().variant.set(variant.id());
    }

    /// Vanilla `AbstractChestedHorse.hasChest`.
    #[must_use]
    pub fn has_chest(&self) -> bool {
        *self
            .entity_data
            .lock()
            .abstract_chested_horse
            .id_chest
            .get()
    }

    /// Vanilla `AbstractChestedHorse.setChest`.
    pub fn set_chest(&self, has_chest: bool) {
        self.entity_data
            .lock()
            .abstract_chested_horse
            .id_chest
            .set(has_chest);
    }

    /// Vanilla `Llama.getInventoryColumns`: `hasChest() ? strength : 0`.
    #[must_use]
    pub fn inventory_columns(&self) -> i32 {
        if self.has_chest() { self.strength() } else { 0 }
    }

    /// Vanilla `AbstractHorse.getInventorySize` = `columns * 3`.
    #[must_use]
    pub fn inventory_size(&self) -> usize {
        AbstractHorseParts::inventory_size(self.inventory_columns())
    }

    /// Vanilla `AbstractHorse.createInventory`: rebuilds the storage, copying slots.
    fn create_inventory(&self) {
        self.parts.create_inventory(self.inventory_size());
    }

    /// Returns the carried items (vanilla `AbstractHorse` inventory).
    #[must_use]
    pub const fn carried_items(&self) -> &Shared<InventoryCarrier> {
        self.parts.inventory()
    }

    /// Vanilla `AbstractHorse.isTamed`.
    #[must_use]
    pub fn is_tamed(&self) -> bool {
        is_tamed(self.entity_data.lock().abstract_horse())
    }

    /// Vanilla `AbstractHorse.setTamed`.
    pub fn set_tamed(&self, tamed: bool) {
        set_flag(
            self.entity_data.lock().abstract_horse_mut(),
            FLAG_TAME,
            tamed,
        );
    }

    /// Vanilla `AbstractHorse.isEating`.
    #[must_use]
    pub fn is_eating(&self) -> bool {
        is_tamed(self.entity_data.lock().abstract_horse())
    }

    /// Vanilla `AbstractHorse.getTemper`.
    #[must_use]
    pub fn temper(&self) -> i32 {
        self.parts.temper()
    }

    /// Vanilla `AbstractHorse.getOwnerReference`, reduced to the owner UUID.
    #[must_use]
    pub fn owner(&self) -> Option<uuid::Uuid> {
        self.parts.owner()
    }

    /// Runs a shared `equine` helper against a detached copy of the synchronized
    /// `AbstractHorse` layer.
    ///
    /// Mirrors the equine entities: the helpers read health, age and level back
    /// through the entity, which re-enters this entity's synced-data mutex, so
    /// the `id_flags` byte is detached, mutated and merged back.
    fn with_horse_data<R>(&self, f: impl FnOnce(&mut AbstractHorseEntityData) -> R) -> R {
        let mut detached = AbstractHorseEntityData::new();
        detached
            .id_flags
            .set(*self.entity_data.lock().abstract_horse().id_flags.get());
        let result = f(&mut detached);
        self.entity_data
            .lock()
            .abstract_horse_mut()
            .id_flags
            .set(*detached.id_flags.get());
        result
    }

    /// Vanilla `Llama.canPerformRearing`: llamas never rear up.
    const fn can_perform_rearing() -> bool {
        false
    }

    /// Vanilla `Llama.isFood`: `ItemTags.LLAMA_FOOD`.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::LLAMA_FOOD)
    }

    /// Vanilla `Llama.canParent`/`Llama.canMate` tail.
    #[must_use]
    pub fn can_parent(&self) -> bool {
        self.with_horse_data(|data| can_parent(self, data))
    }

    /// Vanilla `AbstractHorse.openCustomInventoryScreen`.
    pub fn open_custom_inventory_screen(&self, player: &Player) {
        self.with_horse_data(|data| {
            open_custom_inventory_screen(self, &self.parts, data, player, self.inventory_columns());
        });
    }

    /// Vanilla `AbstractChestedHorse.equipChest`.
    fn equip_chest(&self, player: &Player, hand: InteractionHand) {
        self.set_chest(true);
        equip_chest(
            self,
            &self.parts,
            player,
            hand,
            &sound_events::ENTITY_LLAMA_CHEST,
            self.inventory_columns(),
        );
    }

    /// Vanilla `AbstractHorse.fedFood` over `Llama.handleEating`.
    fn feed(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let item_stack = {
            let inventory = player.inventory.lock();
            let item_stack = inventory.get_item_in_hand(hand);
            item_stack.copy_with_count(item_stack.count())
        };

        let ate_food =
            self.with_horse_data(|data| self.handle_llama_eating(player, &item_stack, data));
        if ate_food {
            let mut inventory = player.inventory.lock();
            inventory
                .get_item_in_hand_mut(hand)
                .consume(1, player.has_infinite_materials());
            InteractionResult::SuccessServer
        } else {
            InteractionResult::Pass
        }
    }

    /// Vanilla `Llama.handleEating`.
    ///
    /// Differs from `AbstractHorse.handleEating`: only wheat and hay blocks are
    /// food, wheat ages a baby by 10 and hay by 90, hay starts love mode, and
    /// `Llama.getMaxTemper` clamps the temper at 30.
    fn handle_llama_eating(
        &self,
        player: &Player,
        item_stack: &ItemStack,
        data: &mut AbstractHorseEntityData,
    ) -> bool {
        let mut age_up = 0;
        let mut temper = 0;
        let mut heal = 0.0_f32;
        let mut item_used = false;

        if item_stack.is(&vanilla_items::WHEAT) {
            age_up = 10;
            temper = 3;
            heal = 2.0;
        } else if item_stack.is(&vanilla_items::HAY_BLOCK) {
            age_up = 90;
            temper = 6;
            heal = 10.0;
            if is_tamed(data) && self.get_age() == 0 && !self.is_in_love() {
                item_used = true;
                self.set_in_love(Some(player));
            }
        }

        if self.get_health() < self.get_max_health() && heal > 0.0 {
            self.heal(heal);
            item_used = true;
        }

        if AgeableMob::is_baby(self) && age_up > 0 && !self.is_age_locked() {
            self.age_up(age_up, false);
            item_used = true;
        }

        if temper > 0
            && (item_used || !is_tamed(data))
            && self.parts.temper() < self.parts.max_temper()
        {
            self.parts.modify_temper(temper);
            item_used = true;
        }

        if item_used {
            eating(
                self,
                &self.parts,
                data,
                Some(&sound_events::ENTITY_LLAMA_EAT),
            );
        }

        item_used
    }

    /// Vanilla `Llama.mobInteract` → `AbstractChestedHorse.mobInteract`.
    fn chested_mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let is_baby = AgeableMob::is_baby(self);
        let should_open_inventory = !is_baby && self.is_tamed() && player.is_secondary_use_active();
        let holding_golden_dandelion =
            player.is_holding(&mut |stack| stack.is(&vanilla_items::GOLDEN_DANDELION));

        if !self.is_vehicle() && !should_open_inventory && (!is_baby || !holding_golden_dandelion) {
            let item_stack = {
                let inventory = player.inventory.lock();
                let item_stack = inventory.get_item_in_hand(hand);
                item_stack.copy_with_count(item_stack.count())
            };

            if !item_stack.is_empty() {
                if LlamaEntity::is_food(&item_stack) {
                    return self.feed(player, hand);
                }

                if !self.is_tamed() {
                    if let Some(world) = self.level()
                        && let Some(entity) = world.get_entity_by_id(self.id())
                    {
                        self.make_mad(&entity);
                    }
                    return InteractionResult::Success;
                }

                if !self.has_chest() && item_stack.is(&vanilla_items::CHEST) {
                    self.equip_chest(player, hand);
                    return InteractionResult::Success;
                }
            }
        }

        // Vanilla `AbstractHorse.mobInteract` tail.
        self.with_horse_data(|data| {
            abstract_horse_mob_interact(
                self,
                &self.parts,
                data,
                player,
                hand,
                self.inventory_columns(),
            )
        })
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

impl Entity for LlamaEntity {
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
        if AgeableMob::is_baby(self) {
            LLAMA_BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        controlling_passenger(self)
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

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_LLAMA_STEP, 0.15, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        self.parts
            .save_abstract_horse(self.entity_data.lock().abstract_horse(), nbt);

        // Vanilla `Llama` + `AbstractChestedHorse` keys.
        // Vanilla `Llama.Variant.LEGACY_CODEC` stores the variant by name.
        nbt.insert("Variant", self.variant().serialized_name());
        nbt.insert("Strength", self.strength());
        nbt.insert("ChestedHorse", i8::from(self.has_chest()));
        if self.has_chest() {
            self.parts.save_items(nbt);
        }
        save_legacy_equipment(self, nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.parts
            .load_abstract_horse(self.entity_data.lock().abstract_horse_mut(), &nbt);

        // Vanilla reads `Strength` before the super call and clamps through `setStrength`.
        if let Some(strength) = nbt.int("Strength") {
            self.set_strength(strength);
        }
        if let Some(variant) = nbt
            .string("Variant")
            .and_then(|name| LlamaVariant::from_serialized_name(name.to_str().as_ref()))
        {
            self.set_variant(variant);
        }
        // Vanilla `AbstractChestedHorse.readAdditionalSaveData`.
        self.set_chest(nbt.byte("ChestedHorse").is_some_and(|value| value != 0));
        self.create_inventory();
        if self.has_chest() {
            self.parts.load_items(&nbt, self.inventory_size());
        }
        load_legacy_equipment(self, &nbt);
    }
}

impl LivingEntity for LlamaEntity {
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

    fn sound_volume(&self) -> f32 {
        0.4
    }

    /// Vanilla `Llama.canUseSlot`: llamas accept every equipment slot.
    fn can_use_slot(&self, _slot: EquipmentSlot) -> bool {
        true
    }

    fn can_dispenser_equip_into_slot(&self, slot: EquipmentSlot) -> bool {
        let body_or_saddle = slot == EquipmentSlot::Body || slot == EquipmentSlot::Saddle;
        (body_or_saddle && self.is_tamed()) || Mob::can_pick_up_loot(self)
    }

    fn equip_sound(&self, slot: EquipmentSlot, stack: &ItemStack) -> Option<SoundEventRef> {
        // Vanilla `AbstractHorse.getEquipSound`: the saddle sound, otherwise the
        // equippable component's own sound.
        equip_sound(slot, stack)
    }

    /// Vanilla `AbstractChestedHorse.dropEquipment`: carried inventory stacks
    /// plus the chest block drop.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        drop_carried_items(self, &self.parts);
        if self.has_chest() {
            let _ = self.spawn_at_location(ItemStack::new(&vanilla_items::CHEST), 0.0);
            self.set_chest(false);
        }
    }

    /// Vanilla `Llama.isImmobile`: `isDeadOrDying() || isEating()`.
    fn is_immobile(&self) -> bool {
        self.is_dead_or_dying() || is_immobile_from_state(self.entity_data.lock().abstract_horse())
    }

    fn tick_ridden(&self, controller: &Player, ridden_input: DVec3) {
        tick_ridden(self, &self.parts, controller, ridden_input);
    }

    fn ridden_input(&self, controller: &Player, _self_input: DVec3) -> DVec3 {
        self.with_horse_data(|data| ridden_input(self, &self.parts, data, controller))
    }

    fn ridden_speed(&self, _controller: &Player) -> f32 {
        ridden_speed(self)
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_LLAMA_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_LLAMA_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `Llama.canEatGrass` is false.
        self.with_horse_data(|data| tick_abstract_horse(self, &self.parts, data, false));

        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for LlamaEntity {
    fn ageable_base(&self) -> &AgeableMobBase {
        &self.ageable_base
    }

    fn is_age_locked(&self) -> bool {
        *self.entity_data.lock().ageable_mob().age_locked.get()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.entity_data
            .lock()
            .ageable_mob_mut()
            .age_locked
            .set(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.entity_data.lock().ageable_mob_mut().baby.set(baby);
    }

    fn age_boundary_changed(&self, _baby: bool) {
        self.refresh_dimensions();
    }
}

impl Animal for LlamaEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    /// Vanilla `Llama.isFood` uses `ItemTags.LLAMA_FOOD`.
    fn is_food(&self, item_stack: &ItemStack) -> bool {
        REGISTRY
            .items
            .is_in_tag(item_stack.item(), &ItemTag::LLAMA_FOOD)
    }
}

impl Mob for LlamaEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn custom_server_ai_step(&self) {
        Animal::custom_server_ai_step_animal(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_LLAMA_AMBIENT)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        self.randomize_max_health();
        self.set_random_strength();

        // Vanilla `Llama.finalizeSpawn`: random variant unless a llama group
        // already chose one. Steel's `SpawnGroupData` has no llama variant slot, so
        // the variant is rolled per llama.
        let variant = LlamaVariant::VALUES[rand::random_range(0..LlamaVariant::VALUES.len())];
        self.set_variant(variant);

        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.chested_mob_interact(player, hand)
    }

    /// Vanilla `AbstractHorse.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        6
    }
}

impl Equine for LlamaEntity {
    fn is_tamed(&self) -> bool {
        LlamaEntity::is_tamed(self)
    }

    fn temper(&self) -> i32 {
        self.parts.temper()
    }

    fn max_temper(&self) -> i32 {
        self.parts.max_temper()
    }

    fn modify_temper(&self, amount: i32) -> i32 {
        self.parts.modify_temper(amount)
    }

    fn tame_with_name(&self, player: &SharedEntity) {
        self.with_horse_data(|data| {
            tame_with_name(self, &self.parts, data, player);
        });
    }

    fn make_mad(&self, _mob: &SharedEntity) {
        let can_perform_rearing = Self::can_perform_rearing();
        self.with_horse_data(|data| {
            make_mad(
                self,
                &self.parts,
                data,
                can_perform_rearing,
                Some(&sound_events::ENTITY_LLAMA_ANGRY),
            );
        });
    }

    fn stand_if_possible(&self) {
        let can_perform_rearing = Self::can_perform_rearing();
        self.with_horse_data(|data| {
            stand_if_possible(self, &self.parts, data, can_perform_rearing);
        });
    }

    fn ambient_stand_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_LLAMA_AMBIENT)
    }
}

impl PathfinderMob for LlamaEntity {}

impl CaravanLlama for LlamaEntity {
    fn in_caravan(&self) -> bool {
        self.caravan_head().is_some()
    }

    fn caravan_head(&self) -> Option<SharedEntity> {
        self.caravan_head.lock().as_ref().and_then(Weak::upgrade)
    }

    fn has_caravan_tail(&self) -> bool {
        self.caravan_tail
            .lock()
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some()
    }

    fn join_caravan(&self, head: &SharedEntity, self_handle: &SharedEntity) {
        // Vanilla `joinCaravan`: `this.caravanHead = head; head.caravanTail = this`.
        *self.caravan_head.lock() = Some(Arc::downgrade(head));
        if let Some(head_llama) = head.as_caravan_llama() {
            head_llama.set_caravan_tail(Some(self_handle));
        }
    }

    fn leave_caravan(&self) {
        // Vanilla `leaveCaravan`: clear this llama's head and that head's tail.
        let head = self
            .caravan_head
            .lock()
            .take()
            .and_then(|head| head.upgrade());
        if let Some(head) = head
            && let Some(head_llama) = head.as_caravan_llama()
        {
            head_llama.set_caravan_tail(None);
        }
    }

    fn set_caravan_tail(&self, tail: Option<&SharedEntity>) {
        *self.caravan_tail.lock() = tail.map(Arc::downgrade);
    }
}

impl RangedAttackMob for LlamaEntity {
    /// Vanilla `Llama.performRangedAttack` -> `Llama.spit`.
    fn perform_ranged_attack(&self, target: &SharedEntity, _power: f32) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        // The owner reference the projectile keeps is the world's handle for this llama
        // (the trader llama shares the same id through its inner llama).
        let Some(owner) = world.get_entity_by_id(self.id()) else {
            return false;
        };

        let spit = LlamaSpitEntity::from_llama(
            &vanilla_entities::LLAMA_SPIT,
            next_entity_id(),
            &owner,
            Arc::downgrade(&world),
        );

        // Vanilla:
        // `xd = target.getX() - getX()`, `yd = target.getY(0.3333) - spit.getY()`,
        // `zd = target.getZ() - getZ()`, `yo = sqrt(xd * xd + zd * zd) * 0.2`,
        // then `shoot(xd, yd + yo, zd, 1.5F, rangedAttackUncertainty(level))`.
        let position = self.position();
        let target_position = target.position();
        let target_y = target.old_position().y
            + (target_position.y - target.old_position().y) * SPIT_TARGET_PARTIAL_TICKS;
        let xd = target_position.x - position.x;
        let yd = target_y - spit.position().y;
        let zd = target_position.z - position.z;
        let yo = (xd * xd + zd * zd).sqrt() * SPIT_TARGET_Y_SCALE;

        let mut item_stack = ItemStack::empty();
        let uncertainty = self.ranged_attack_uncertainty(&world);
        let spawned = spawn_projectile(&world, spit, &mut item_stack, |spit| {
            spit.shoot(DVec3::new(xd, yd + yo, zd), SPIT_POWER, uncertainty);
        });

        if !self.is_silent() {
            // Vanilla `playSound(SoundEvents.LLAMA_SPIT, 1.0F, 1.0F + (random - random) * 0.2F)`.
            let pitch =
                1.0 + (rand::random::<f32>() - rand::random::<f32>()) * SPIT_SOUND_PITCH_JITTER;
            self.play_sound(&sound_events::ENTITY_LLAMA_SPIT, 1.0, pitch);
        }

        spawned.is_some()
    }

    /// Vanilla `Llama.rangedAttackUncertainty` is the constant `10.0F`.
    fn ranged_attack_uncertainty(&self, _world: &World) -> f32 {
        SPIT_UNCERTAINTY
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_damage_types, vanilla_entities,
        vanilla_game_rules,
    };
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos, WorldAabb};

    use super::{LlamaEntity, TraderLlamaEntity};
    use crate::behavior::init_behaviors;
    use crate::entity::ai::goal::CaravanLlama;
    use crate::entity::damage::DamageSource;
    use crate::entity::entities::mobs::npc::wandering_trader::WanderingTraderEntity;
    use crate::entity::entities::{PigEntity, ZombieEntity};
    use crate::entity::leash::Leashable;
    use crate::entity::{
        Entity, Mob, SharedEntity, add_spawned_entity, init_entities, next_entity_id,
    };
    use crate::test_support::{TestWorld, fresh_test_world, insert_entity_ticking_chunks};
    use crate::world::World;

    /// A flat stone floor with the 5x5 entity-ticking neighborhood loaded, so mob AI and
    /// projectiles run through the world entity manager.
    fn llama_test_world(key: &'static str) -> TestWorld {
        init_vanilla_registry();
        init_entities();
        init_behaviors();

        let fixture = fresh_test_world(key);
        let world = &fixture.world;
        insert_entity_ticking_chunks(world, ChunkPos::new(0, 0), 2);
        assert!(world.set_game_rule(&vanilla_game_rules::SPAWN_MOBS, false));

        let stone = vanilla_blocks::STONE.default_state();
        for x in -24..=32 {
            for z in -24..=32 {
                assert!(world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL));
            }
        }
        fixture
    }

    fn spawn(world: &Arc<World>, entity: SharedEntity) -> SharedEntity {
        if let Some(mob) = entity.as_mob() {
            mob.set_persistence_required();
        }
        add_spawned_entity(world, Arc::clone(&entity)).expect("entity should join the world");
        entity
    }

    fn spawn_llama(world: &Arc<World>, id: i32, position: DVec3) -> Arc<LlamaEntity> {
        let llama = Arc::new(LlamaEntity::new(
            &vanilla_entities::LLAMA,
            id,
            position,
            Arc::downgrade(world),
        ));
        spawn(world, Arc::<LlamaEntity>::clone(&llama) as SharedEntity);
        llama
    }

    fn spawn_trader_llama(world: &Arc<World>, id: i32, position: DVec3) -> Arc<TraderLlamaEntity> {
        let llama = Arc::new(TraderLlamaEntity::new(
            &vanilla_entities::TRADER_LLAMA,
            id,
            position,
            Arc::downgrade(world),
        ));
        spawn(
            world,
            Arc::<TraderLlamaEntity>::clone(&llama) as SharedEntity,
        );
        llama
    }

    fn spawn_pig(world: &Arc<World>, id: i32, position: DVec3) -> Arc<PigEntity> {
        let pig = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            id,
            position,
            Arc::downgrade(world),
        ));
        spawn(world, Arc::<PigEntity>::clone(&pig) as SharedEntity);
        pig
    }

    fn spawn_zombie(world: &Arc<World>, id: i32, position: DVec3) -> Arc<ZombieEntity> {
        let zombie = Arc::new(ZombieEntity::new(
            &vanilla_entities::ZOMBIE,
            id,
            position,
            Arc::downgrade(world),
        ));
        spawn(world, Arc::<ZombieEntity>::clone(&zombie) as SharedEntity);
        zombie
    }

    fn tick_world(world: &Arc<World>, ticks: i32) {
        for tick in 0..ticks {
            world.entity_manager().tick_entities(tick, true);
        }
    }

    fn has_llama_spit(world: &World, around: DVec3) -> bool {
        !world
            .get_entities_in_aabb_matching(&WorldAabb::of_size(around, 32.0, 8.0, 32.0), |entity| {
                entity.entity_type() == &vanilla_entities::LLAMA_SPIT
            })
            .is_empty()
    }

    #[test]
    fn llama_spits_at_its_target_from_range() {
        let fixture = llama_test_world("llama_spits_at_target");
        let world = &fixture.world;
        let llama = spawn_llama(world, next_entity_id(), DVec3::new(0.5, 64.0, 0.5));
        let pig = spawn_pig(world, next_entity_id(), DVec3::new(0.5, 64.0, 10.5));
        let target: SharedEntity = Arc::<PigEntity>::clone(&pig) as SharedEntity;

        assert!(
            llama.set_target(Some(&target)),
            "the pig is a valid llama target"
        );

        let mut spit = false;
        for _ in 0..120 {
            tick_world(world, 1);
            if has_llama_spit(world, llama.position()) {
                spit = true;
                break;
            }
        }

        assert!(
            spit,
            "vanilla `RangedAttackGoal` must make the llama spit a `LlamaSpit` at its target"
        );
    }

    #[test]
    fn loose_llama_joins_a_passing_caravan() {
        let fixture = llama_test_world("llama_joins_caravan");
        let world = &fixture.world;
        let lead = spawn_llama(world, next_entity_id(), DVec3::new(0.5, 64.0, 0.5));
        let follower = spawn_llama(world, next_entity_id(), DVec3::new(0.5, 64.0, 8.5));
        let holder = spawn_pig(world, next_entity_id(), DVec3::new(3.5, 64.0, 0.5));
        let holder_entity: SharedEntity = Arc::<PigEntity>::clone(&holder) as SharedEntity;
        assert!(
            lead.set_leashed_to(&holder_entity),
            "the caravan head must be leashed"
        );

        let mut joined = false;
        let mut distance_closed = false;
        for _ in 0..240 {
            tick_world(world, 1);
            joined |= follower.in_caravan();
            distance_closed |= follower.position().distance(lead.position()) < 4.0;
        }

        assert!(
            joined,
            "vanilla `LlamaFollowCaravanGoal` must make a loose llama join a leashed caravan"
        );
        assert!(
            lead.has_caravan_tail(),
            "vanilla `joinCaravan` must write the follower into the head's tail slot"
        );
        assert!(
            distance_closed,
            "the caravan follower must trail within two blocks of the head"
        );
    }

    #[test]
    fn trader_llama_defends_the_wandering_trader() {
        let fixture = llama_test_world("trader_llama_defends");
        let world = &fixture.world;

        let trader = Arc::new(WanderingTraderEntity::new(
            &vanilla_entities::WANDERING_TRADER,
            next_entity_id(),
            DVec3::new(0.5, 64.0, 0.5),
            Arc::downgrade(world),
        ));
        let trader_entity: SharedEntity = spawn(
            world,
            Arc::<WanderingTraderEntity>::clone(&trader) as SharedEntity,
        );
        trader.set_despawn_delay(47_999);

        let llama = spawn_trader_llama(world, next_entity_id(), DVec3::new(2.5, 64.0, 0.5));
        let llama_entity: SharedEntity = Arc::<TraderLlamaEntity>::clone(&llama) as SharedEntity;
        assert!(
            llama.set_leashed_to(&trader_entity),
            "the trader llama must leash to the wandering trader"
        );

        let zombie = spawn_zombie(world, next_entity_id(), DVec3::new(2.5, 64.0, 8.5));
        let attacker: SharedEntity = Arc::<ZombieEntity>::clone(&zombie) as SharedEntity;
        assert!(
            trader.hurt(
                world,
                &DamageSource::direct(&vanilla_damage_types::MOB_ATTACK, Arc::clone(&attacker)),
                1.0,
            ),
            "the trader must accept the zombie's hit"
        );

        let mut defended = false;
        let mut spit = false;
        for _ in 0..140 {
            tick_world(world, 1);
            defended |= llama
                .target()
                .is_some_and(|target| target.id() == attacker.id());
            spit |= has_llama_spit(world, llama_entity.position());
            if defended && spit {
                break;
            }
        }

        assert!(
            defended,
            "vanilla `TraderLlamaDefendWanderingTraderGoal` must target the trader's attacker"
        );
        assert!(
            spit,
            "the defending trader llama must spit at the attacker through `RangedAttackGoal`"
        );
    }
}
