//! Vanilla `TropicalFish` (`net.minecraft.world.entity.animal.fish.TropicalFish`).
//!
//! A schooling `AbstractFish` whose synced variant packs a pattern with two dye
//! colors. The shared water behaviour lives in [`super::abstract_fish`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::{
    TROPICAL_FISH_BASE_COLOR, TROPICAL_FISH_PATTERN, TROPICAL_FISH_PATTERN_COLOR,
};
use steel_registry::dye_color::DyeColor;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::fluid::FluidStateExt as _;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_biome_tags::BiomeTag;
use steel_registry::vanilla_entity_data::TropicalFishEntityData;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, sound_events, vanilla_attributes, vanilla_blocks, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::SchoolingFish;
use crate::entity::damage::DamageSource;
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

use super::abstract_fish::{self, SchoolingState};

/// Vanilla `TropicalFish.Base`, shared with the pattern item component.
///
/// Deviation: Steel keeps the closed enum in `steel_registry` so the synced value and
/// the bucket components use one representation.
pub use steel_registry::entity_variant::TropicalFishBase as Base;
/// Vanilla `TropicalFish.Pattern`, shared with the pattern item component.
///
/// Deviation: Steel keeps the closed enum in `steel_registry` so the synced value and
/// the bucket components use one representation; `Pattern::id()` is vanilla
/// `Pattern.getPackedId()`.
pub use steel_registry::entity_variant::TropicalFishPattern as Pattern;

/// Vanilla `TropicalFish.getMaxSchoolSize` (`AbstractFish.getMaxSpawnClusterSize`).
pub(super) const MAX_SCHOOL_SIZE: i32 = 8;
/// Vanilla `TropicalFish.TAG_VARIANT`.
const TAG_VARIANT: &str = "Variant";
/// Vanilla `TropicalFish.finalizeSpawn` common-variant chance.
const COMMON_VARIANT_CHANCE: f32 = 0.9;
/// Vanilla `Mob` default step height used when the attribute is missing.
const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `TropicalFish.packVariant` bit layout.
const BASE_COLOR_SHIFT: i32 = 16;
const PATTERN_COLOR_SHIFT: i32 = 24;
const COLOR_MASK: i32 = 0xFF;
const PATTERN_MASK: i32 = 0xFFFF;

/// Vanilla `TropicalFish.Variant`: a pattern plus its base and pattern colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// Vanilla `Variant.pattern`.
    pub pattern: Pattern,
    /// Vanilla `Variant.baseColor`.
    pub base_color: DyeColor,
    /// Vanilla `Variant.patternColor`.
    pub pattern_color: DyeColor,
}

impl Variant {
    /// Vanilla `new TropicalFish.Variant(pattern, baseColor, patternColor)`.
    #[must_use]
    pub const fn new(pattern: Pattern, base_color: DyeColor, pattern_color: DyeColor) -> Self {
        Self {
            pattern,
            base_color,
            pattern_color,
        }
    }

    /// Vanilla `new TropicalFish.Variant(int packedId)`.
    #[must_use]
    pub const fn from_packed_id(packed_id: i32) -> Self {
        Self {
            pattern: Self::pattern_of(packed_id),
            base_color: Self::base_color_of(packed_id),
            pattern_color: Self::pattern_color_of(packed_id),
        }
    }

    /// Vanilla `TropicalFish.packVariant`.
    #[must_use]
    pub const fn packed_id(self) -> i32 {
        self.pattern.id() & PATTERN_MASK
            | (self.base_color.id() & COLOR_MASK) << BASE_COLOR_SHIFT
            | (self.pattern_color.id() & COLOR_MASK) << PATTERN_COLOR_SHIFT
    }

    /// Vanilla `TropicalFish.getPattern(int)`.
    #[must_use]
    pub const fn pattern_of(packed_id: i32) -> Pattern {
        Pattern::by_id(packed_id & PATTERN_MASK)
    }

    /// Vanilla `TropicalFish.getBaseColor(int)`.
    #[must_use]
    pub const fn base_color_of(packed_id: i32) -> DyeColor {
        DyeColor::by_id(packed_id >> BASE_COLOR_SHIFT & COLOR_MASK)
    }

    /// Vanilla `TropicalFish.getPatternColor(int)`.
    #[must_use]
    pub const fn pattern_color_of(packed_id: i32) -> DyeColor {
        DyeColor::by_id(packed_id >> PATTERN_COLOR_SHIFT & COLOR_MASK)
    }
}

/// Vanilla `TropicalFish.DEFAULT_VARIANT`.
pub const DEFAULT_VARIANT: Variant = Variant::new(Pattern::Kob, DyeColor::White, DyeColor::White);

/// Vanilla `TropicalFish.COMMON_VARIANTS`, in vanilla order.
pub const COMMON_VARIANTS: [Variant; 22] = [
    Variant::new(Pattern::Stripey, DyeColor::Orange, DyeColor::Gray),
    Variant::new(Pattern::Flopper, DyeColor::Gray, DyeColor::Gray),
    Variant::new(Pattern::Flopper, DyeColor::Gray, DyeColor::Blue),
    Variant::new(Pattern::Clayfish, DyeColor::White, DyeColor::Gray),
    Variant::new(Pattern::Sunstreak, DyeColor::Blue, DyeColor::Gray),
    Variant::new(Pattern::Kob, DyeColor::Orange, DyeColor::White),
    Variant::new(Pattern::Spotty, DyeColor::Pink, DyeColor::LightBlue),
    Variant::new(Pattern::Blockfish, DyeColor::Purple, DyeColor::Yellow),
    Variant::new(Pattern::Clayfish, DyeColor::White, DyeColor::Red),
    Variant::new(Pattern::Spotty, DyeColor::White, DyeColor::Yellow),
    Variant::new(Pattern::Glitter, DyeColor::White, DyeColor::Gray),
    Variant::new(Pattern::Clayfish, DyeColor::White, DyeColor::Orange),
    Variant::new(Pattern::Dasher, DyeColor::Cyan, DyeColor::Pink),
    Variant::new(Pattern::Brinely, DyeColor::Lime, DyeColor::LightBlue),
    Variant::new(Pattern::Betty, DyeColor::Red, DyeColor::White),
    Variant::new(Pattern::Snooper, DyeColor::Gray, DyeColor::Red),
    Variant::new(Pattern::Blockfish, DyeColor::Red, DyeColor::White),
    Variant::new(Pattern::Flopper, DyeColor::White, DyeColor::Yellow),
    Variant::new(Pattern::Kob, DyeColor::Red, DyeColor::White),
    Variant::new(Pattern::Sunstreak, DyeColor::Gray, DyeColor::White),
    Variant::new(Pattern::Dasher, DyeColor::Cyan, DyeColor::Yellow),
    Variant::new(Pattern::Flopper, DyeColor::Yellow, DyeColor::Yellow),
];

#[entity_behavior(class = "TropicalFish")]
/// Vanilla tropical fish: a schooling bucketable fish with a packed pattern variant.
pub struct TropicalFishEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<TropicalFishEntityData>,
    schooling: SyncMutex<SchoolingState>,
    /// Vanilla `TropicalFish.isSchool`, defaulting to `true`.
    is_school: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TropicalFishEntity`.
unsafe impl DowncastType for TropicalFishEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/tropical_fish");
}

impl TropicalFishEntity {
    /// Creates a new tropical fish at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a tropical fish from persisted base entity state.
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
        abstract_fish::initialize_water_pathfinding_malus(&mob_base);
        let mut entity_data = TropicalFishEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data.id_type_variant.set(DEFAULT_VARIANT.packed_id());
        abstract_fish::register_abstract_fish_goals(&mob_base);
        abstract_fish::register_schooling_goals(&mob_base);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            schooling: SyncMutex::new(SchoolingState::new()),
            is_school: SyncMutex::new(true),
        }
    }

    /// Vanilla `TropicalFish.getPackedVariant`.
    #[must_use]
    pub fn packed_variant(&self) -> i32 {
        *self.entity_data.lock().id_type_variant.get()
    }

    /// Vanilla `TropicalFish.setPackedVariant`.
    fn set_packed_variant(&self, packed_variant: i32) {
        self.entity_data.lock().id_type_variant.set(packed_variant);
    }

    /// Vanilla `TropicalFish.getVariant` (`new Variant(getPackedVariant())`).
    #[must_use]
    pub fn variant(&self) -> Variant {
        Variant::from_packed_id(self.packed_variant())
    }

    /// Vanilla `TropicalFish.getPattern`.
    #[must_use]
    pub fn pattern(&self) -> Pattern {
        Variant::pattern_of(self.packed_variant())
    }

    /// Vanilla `TropicalFish.getBaseColor`.
    #[must_use]
    pub fn base_color(&self) -> DyeColor {
        Variant::base_color_of(self.packed_variant())
    }

    /// Vanilla `TropicalFish.getPatternColor`.
    #[must_use]
    pub fn pattern_color(&self) -> DyeColor {
        Variant::pattern_color_of(self.packed_variant())
    }

    /// Vanilla `TropicalFish.setPattern`.
    fn set_pattern(&self, pattern: Pattern) {
        let packed = self.packed_variant();
        self.set_packed_variant(
            Variant::new(
                pattern,
                Variant::base_color_of(packed),
                Variant::pattern_color_of(packed),
            )
            .packed_id(),
        );
    }

    /// Vanilla `TropicalFish.setBaseColor`.
    fn set_base_color(&self, base_color: DyeColor) {
        let packed = self.packed_variant();
        self.set_packed_variant(
            Variant::new(
                Variant::pattern_of(packed),
                base_color,
                Variant::pattern_color_of(packed),
            )
            .packed_id(),
        );
    }

    /// Vanilla `TropicalFish.setPatternColor`.
    fn set_pattern_color(&self, pattern_color: DyeColor) {
        let packed = self.packed_variant();
        self.set_packed_variant(
            Variant::new(
                Variant::pattern_of(packed),
                Variant::base_color_of(packed),
                pattern_color,
            )
            .packed_id(),
        );
    }

    /// Vanilla `TropicalFish.isSchool`.
    #[must_use]
    pub fn is_school(&self) -> bool {
        *self.is_school.lock()
    }

    fn set_is_school(&self, is_school: bool) {
        *self.is_school.lock() = is_school;
    }

    /// Vanilla `AbstractFish.fromBucket`.
    #[must_use]
    #[expect(
        clippy::wrong_self_convention,
        reason = "vanilla `AbstractFish.fromBucket` getter; the name is shared with the other bucketable fish entities"
    )]
    pub fn from_bucket(&self) -> bool {
        *self.entity_data.lock().abstract_fish.from_bucket.get()
    }

    /// Vanilla `AbstractFish.setFromBucket`.
    pub fn set_from_bucket(&self, from_bucket: bool) {
        self.entity_data
            .lock()
            .abstract_fish
            .from_bucket
            .set(from_bucket);
    }

    /// Vanilla `AbstractSchoolingFish` flock state.
    pub(super) const fn schooling_state(&self) -> &SyncMutex<SchoolingState> {
        &self.schooling
    }

    /// Vanilla `TropicalFish.getBucketItemStack`.
    #[must_use]
    pub fn bucket_item_stack() -> ItemStack {
        ItemStack::new(&vanilla_items::TROPICAL_FISH_BUCKET)
    }

    /// Vanilla `AbstractFish.getPickupSound`.
    #[must_use]
    pub fn pickup_sound() -> SoundEventRef {
        &sound_events::ITEM_BUCKET_FILL_FISH
    }

    /// Vanilla `TropicalFish.saveToBucketTag`: the default bucket data plus the three
    /// tropical-fish components.
    fn create_bucket_stack(&self) -> ItemStack {
        let mut stack = Self::bucket_item_stack();
        abstract_fish::save_default_bucket_data(self, &mut stack);
        stack.set(TROPICAL_FISH_PATTERN, self.pattern());
        stack.set(TROPICAL_FISH_BASE_COLOR, self.base_color());
        stack.set(TROPICAL_FISH_PATTERN_COLOR, self.pattern_color());
        stack
    }

    /// Vanilla `TropicalFish.loadFromBucketTag`
    /// (`Bucketable.loadDefaultDataFromBucketTag`).
    pub fn load_from_bucket_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        abstract_fish::load_default_bucket_data(self, nbt);
    }

    /// Vanilla `AbstractFish.mobInteract`'s bucket half (`Bucketable.bucketMobPickup`).
    fn try_bucket_pickup(&self, player: &Player, hand: InteractionHand) -> bool {
        abstract_fish::try_bucket_pickup(self, player, hand, Self::pickup_sound(), || {
            self.create_bucket_stack()
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

impl Entity for TropicalFishEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    /// Vanilla `WaterAnimal.baseTick`: the generic base tick, then the water air supply.
    fn base_tick(&self) {
        let air_supply = self.air_supply();
        Mob::base_tick_mob(self);
        abstract_fish::handle_air_supply(self, air_supply);
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
        // Vanilla `AbstractSchoolingFish.tick` after `super.tick()`.
        abstract_fish::tick_schooling(entity.as_ref());
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

    /// Vanilla `AbstractFish.playStepSound` is empty.
    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {}

    fn swim_sound(&self) -> SoundEventRef {
        abstract_fish::swim_sound()
    }

    /// Vanilla `WaterAnimal.isPushedByFluid`.
    fn is_pushed_by_fluid(&self) -> bool {
        false
    }

    fn apply_implicit_item_components(&self, item_stack: &ItemStack) {
        // Vanilla `TropicalFish.applyImplicitComponent` for the three components.
        if let Some(pattern) = item_stack.get(TROPICAL_FISH_PATTERN) {
            self.set_pattern(*pattern);
        }
        if let Some(base_color) = item_stack.get(TROPICAL_FISH_BASE_COLOR) {
            self.set_base_color(*base_color);
        }
        if let Some(pattern_color) = item_stack.get(TROPICAL_FISH_PATTERN_COLOR) {
            self.set_pattern_color(*pattern_color);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("FromBucket", i8::from(self.from_bucket()));
        // Vanilla `TropicalFish.addAdditionalSaveData`: `Variant.CODEC` is the packed int.
        nbt.insert(TAG_VARIANT, self.packed_variant());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_from_bucket(nbt.byte("FromBucket").is_some_and(|value| value != 0));
        self.set_packed_variant(nbt.int(TAG_VARIANT).unwrap_or(DEFAULT_VARIANT.packed_id()));
    }

    /// Vanilla `TropicalFish.loadFromBucketTag`, restored by `MobBucketItem.spawn`
    /// on release.
    fn load_from_bucket_tag(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_from_bucket_data(nbt);
    }

    /// Vanilla `Bucketable.setFromBucket`, flipped for every released mob.
    fn set_from_bucket(&self, from_bucket: bool) {
        TropicalFishEntity::set_from_bucket(self, from_bucket);
    }

    fn as_schooling_fish(&self) -> Option<&dyn SchoolingFish> {
        Some(self)
    }
}

impl LivingEntity for TropicalFishEntity {
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

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_TROPICAL_FISH_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_TROPICAL_FISH_DEATH)
    }

    fn travel_in_water(
        &self,
        entity: &SharedEntity,
        input: DVec3,
        _base_gravity: f64,
        _is_falling: bool,
        _old_y: f64,
    ) -> Option<MoveResult> {
        abstract_fish::travel_in_water(entity, input)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        abstract_fish::tick_flop(self, &sound_events::ENTITY_TROPICAL_FISH_FLOP);
        Mob::mob_ai_step(self, entity)
    }
}

impl TropicalFishEntity {
    /// Vanilla `TropicalFish.checkTropicalFishSpawnRules`, the `SpawnPlacements`
    /// predicate for `EntityType.TROPICAL_FISH`.
    ///
    /// The position needs water below and a full water block above, and either the
    /// `#minecraft:allows_tropical_fish_spawns_at_any_height` biome tag or the shared
    /// surface water-creature rule.
    #[must_use]
    pub fn check_tropical_fish_spawn_rules(
        entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        random: &mut RandomSource,
    ) -> bool {
        level
            .get_block_state(pos.below())
            .get_fluid_state()
            .is_water()
            && level.get_block_state(pos.above()).get_block() == &vanilla_blocks::WATER
            && (level.biome_at(pos).is_some_and(|biome| {
                REGISTRY
                    .biomes
                    .is_in_tag(biome, &BiomeTag::ALLOWS_TROPICAL_FISH_SPAWNS_AT_ANY_HEIGHT)
            }) || abstract_fish::check_surface_water_animal_spawn_rules(
                entity_type,
                level,
                spawn_reason,
                pos,
                random,
            ))
    }
}

impl Mob for TropicalFishEntity {
    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        // Vanilla `AbstractFish.mobInteract`: bucket pickup, else `super.mobInteract`
        // (`Mob` default is `PASS`).
        if self.try_bucket_pickup(player, hand) {
            return InteractionResult::Success;
        }
        InteractionResult::Pass
    }
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    /// Vanilla `AbstractSchoolingFish.getMaxSpawnClusterSize` = `getMaxSchoolSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        MAX_SCHOOL_SIZE
    }

    /// Vanilla `TropicalFish.isMaxGroupSizeReached`.
    fn is_max_group_size_reached(&self, _group_size: i32) -> bool {
        !self.is_school()
    }

    /// Vanilla `TropicalFish.finalizeSpawn`: 90% of fish take a common variant and form a
    /// school, the rest roll a fully random variant and spawn alone.
    ///
    /// Deviation: `SpawnGroupData` carries no variant slot, so each fish rolls its own
    /// variant instead of sharing `TropicalFishGroupData.variant`.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_mob_base(world, spawn_reason, group_data);

        if rand::random::<f32>() < COMMON_VARIANT_CHANCE {
            let index = rand::random_range(0..COMMON_VARIANTS.len());
            self.set_packed_variant(COMMON_VARIANTS[index].packed_id());
        } else {
            self.set_is_school(false);
            let pattern = Pattern::VALUES[rand::random_range(0..Pattern::VALUES.len())];
            let base_color = DyeColor::VALUES[rand::random_range(0..DyeColor::VALUES.len())];
            let pattern_color = DyeColor::VALUES[rand::random_range(0..DyeColor::VALUES.len())];
            self.set_packed_variant(Variant::new(pattern, base_color, pattern_color).packed_id());
        }

        group_data
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_TROPICAL_FISH_AMBIENT)
    }

    fn ambient_sound_interval(&self) -> i32 {
        abstract_fish::AMBIENT_SOUND_INTERVAL
    }

    /// Vanilla `WaterAnimal.canBeLeashed`.
    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    /// Vanilla `AbstractFish.requiresCustomPersistence`.
    fn requires_custom_persistence(&self) -> bool {
        abstract_fish::requires_custom_persistence(self, self.from_bucket())
    }

    /// Vanilla `AbstractFish.removeWhenFarAway`.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        abstract_fish::remove_when_far_away(self, self.from_bucket())
    }

    /// Vanilla `WaterAnimal.checkSpawnObstruction` = `level.isUnobstructed(this)`.
    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        abstract_fish::check_water_animal_spawn_obstruction(world, self)
    }

    /// Vanilla `WaterAnimal.getBaseExperienceReward`.
    fn base_experience_reward_mob(&self) -> i32 {
        abstract_fish::base_experience_reward()
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl SchoolingFish for TropicalFishEntity {
    fn has_followers(&self) -> bool {
        self.schooling.lock().has_followers()
    }

    fn is_follower(&self) -> bool {
        self.schooling.lock().is_follower()
    }

    fn can_be_followed(&self) -> bool {
        self.schooling.lock().can_be_followed(MAX_SCHOOL_SIZE)
    }

    fn add_followers(&self, followers: Vec<SharedEntity>) {
        abstract_fish::add_followers(&self.schooling, self, followers, MAX_SCHOOL_SIZE);
    }

    fn in_range_of_leader(&self) -> bool {
        abstract_fish::in_range_of_leader(&self.schooling, self)
    }

    fn path_to_leader(&self, mob: &SharedEntity) {
        abstract_fish::path_to_leader(&self.schooling, mob);
    }

    fn stop_following(&self) {
        abstract_fish::stop_following(&self.schooling);
    }
}

impl PathfinderMob for TropicalFishEntity {}
