//! Vanilla llama family: `Llama` (chested storage) and `TraderLlama` (the
//! wandering trader's leashed llama with its own despawn clock).
//!
//! Scope: this wave implements the parts `TraderLlama` and the wandering-trader
//! spawner depend on — chested storage (`ChestedHorse` + `Items` NBT, strength-sized
//! inventory), strength/variant synced data, llama sounds, chest interaction and
//! trader despawn/leash behaviour. The remaining vanilla llama behaviour
//! (caravan following, spit ranged attack, taming/temper, breeding offspring,
//! carpet decor, riding) is **not** implemented here and is listed as a gap in the
//! wave report.

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
use steel_registry::vanilla_entity_data::LlamaEntityData;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, LookAtPlayerGoal, PanicGoal, RandomLookAroundGoal,
    TemptGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, Entity, EntityBase, EntityBaseLoad, EntityPose,
    EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Llama.MAX_STRENGTH`.
const MAX_STRENGTH: i32 = 5;
/// Vanilla `AbstractMountInventoryMenu.getInventorySize`: columns × 3 rows.
const INVENTORY_ROWS: usize = 3;
/// Vanilla `AbstractChestedHorse` baby passenger attachment/scale.
const LLAMA_BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    [EntityAttachmentPoint::new(0.0, 1.87 - 0.25, -0.3)];

/// Vanilla `Llama.createAttributes` movement speed (`AbstractChestedHorse`).
const DEFAULT_STEP_HEIGHT: f32 = 0.6;

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
    /// Vanilla `AbstractHorse.inventory`; empty while the llama has no chest.
    inventory: SyncMutex<InventoryCarrier>,
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
            // Vanilla `Llama.registerGoals`; caravan follow, ranged spit and
            // `RunAroundLikeCrazyGoal` are the documented gaps (see module docs).
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
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

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            entity_data: SyncMutex::new(entity_data),
            inventory: SyncMutex::new(InventoryCarrier::new(0)),
        }
    }

    /// Vanilla `Llama.isTraderLlama`; overridden by [`TraderLlamaEntity`].
    #[must_use]
    pub const fn is_trader_llama(&self) -> bool {
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
        usize::try_from(self.inventory_columns()).unwrap_or(0) * INVENTORY_ROWS
    }

    /// Vanilla `AbstractHorse.createInventory`: rebuilds the storage, copying slots.
    fn create_inventory(&self) {
        let size = self.inventory_size();
        let mut inventory = self.inventory.lock();
        let mut rebuilt = InventoryCarrier::new(size);
        let copy_count = inventory.size().min(size);
        for slot in 0..copy_count {
            rebuilt.set_item(slot, inventory.get_item(slot).clone());
        }
        *inventory = rebuilt;
    }

    /// Returns the carried items (vanilla `AbstractHorse` inventory).
    #[must_use]
    pub fn carried_items(&self) -> &SyncMutex<InventoryCarrier> {
        &self.inventory
    }

    /// Vanilla `AbstractChestedHorse.equipChest`.
    fn equip_chest(&self, player: &Player, hand: InteractionHand) {
        self.set_chest(true);
        self.play_sound(
            &sound_events::ENTITY_LLAMA_CHEST,
            1.0,
            (rand::random::<f32>() - rand::random::<f32>()) * 0.2 + 1.0,
        );
        let mut inventory = player.inventory.lock();
        inventory
            .get_item_in_hand_mut(hand)
            .consume(1, player.has_infinite_materials());
        drop(inventory);
        self.create_inventory();
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

        // Vanilla `Llama` + `AbstractChestedHorse` keys.
        // Vanilla `Llama.Variant.LEGACY_CODEC` stores the variant by name.
        nbt.insert("Variant", self.variant().serialized_name());
        nbt.insert("Strength", self.strength());
        nbt.insert("ChestedHorse", i8::from(self.has_chest()));
        if self.has_chest() {
            self.inventory.lock().write_to_tag(nbt);
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);

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
        if nbt.byte("ChestedHorse").is_some_and(|value| value != 0) {
            self.set_chest(true);
            self.create_inventory();
            self.inventory.lock().read_from_tag(&nbt);
        }
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
        // Vanilla `AbstractChestedHorse.mobInteract`: a chest equips storage once tamed.
        let holding_chest = player
            .inventory
            .lock()
            .get_item_in_hand(hand)
            .is(&vanilla_items::CHEST);

        if holding_chest && !self.has_chest() && !AgeableMob::is_baby(self) {
            self.equip_chest(player, hand);
            return InteractionResult::Success;
        }

        InteractionResult::Pass
    }
}

impl PathfinderMob for LlamaEntity {}
