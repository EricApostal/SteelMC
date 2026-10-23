//! Vanilla `Horse` entity: type variant + markings, riding, taming and
//! donkey-cross breeding.
//!
//! # Known gaps
//!
//! * Vanilla `Horse.playGallopSound` adds `HORSE_BREATHE` on one in ten gallops;
//!   the shared `AbstractHorse.playStepSound` helper owns the gallop branch and
//!   has no per-variant hook, so the breathe sound is not played.
//! * The mount inventory GUI opens through `ClientboundMountScreenOpenPacket`
//!   and `mount_menu`; the equipment slots edit one-slot mirrors that are
//!   published back to the entity after each click (see `mount_menu`).

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::entity_variant::HorseVariant;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::{AbstractHorseEntityData, HorseEntityData};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, sound_events, vanilla_attributes, vanilla_entities,
    vanilla_items,
};
use steel_utils::locks::{Shared, SyncMutex};
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use super::{
    AbstractHorseParts, Equine, FLAG_TAME, MAX_TEMPER, MountPanicGoal, RandomStandGoal,
    RunAroundLikeCrazyGoal, abstract_horse_mob_interact, can_mate_horse_or_donkey, can_parent,
    controlling_passenger, default_horse_group_data, fed_food, generate_jump_strength,
    generate_max_health, generate_speed, is_bred, is_eating, is_horse_food, is_immobile_from_state,
    is_tamed, load_legacy_equipment, make_mad, max_up_step, open_custom_inventory_screen,
    play_step_sound, ridden_input, ridden_speed, save_legacy_equipment, set_bred, set_eating,
    set_flag, set_offspring_attributes, stand_if_possible, tick_abstract_horse, tick_ridden,
};
use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BreedGoal, FloatGoal, FollowParentGoal, LookAtPlayerGoal, RandomLookAroundGoal, TemptGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, ENTITIES, Entity, EntityBase, EntityBaseLoad,
    EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    MoveResult, PathfinderMob, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;

/// Vanilla `Horse.BABY_DIMENSIONS` passenger attachment, `scale(0.7)` applied.
const BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    super::horse_baby_passenger_attachments(1.6);
const BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    1.396_484_4 * 0.7,
    1.6 * 0.7,
    1.52 * 0.7,
    EntityAttachments::new(&BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

/// Vanilla `Markings`: the horse's white-pattern overlay.
///
/// Vanilla keeps this enum next to `Horse`/`Variant`; the Steel registry only
/// generates `HorseVariant`, so the markings ids live here.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HorseMarkings {
    /// Vanilla `Markings.NONE`.
    #[default]
    None,
    /// Vanilla `Markings.WHITE`.
    White,
    /// Vanilla `Markings.WHITE_FIELD`.
    WhiteField,
    /// Vanilla `Markings.WHITE_DOTS`.
    WhiteDots,
    /// Vanilla `Markings.BLACK_DOTS`.
    BlackDots,
}

impl HorseMarkings {
    /// Vanilla `Markings.values()`.
    pub const VALUES: [Self; 5] = [
        Self::None,
        Self::White,
        Self::WhiteField,
        Self::WhiteDots,
        Self::BlackDots,
    ];

    /// Vanilla `Markings.getId`.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::None => 0,
            Self::White => 1,
            Self::WhiteField => 2,
            Self::WhiteDots => 3,
            Self::BlackDots => 4,
        }
    }

    /// Vanilla `Markings.byId`, wrapping out-of-range ids like `ByIdMap.continuous`.
    #[must_use]
    pub const fn by_id(id: i32) -> Self {
        match id.rem_euclid(5) {
            0 => Self::None,
            1 => Self::White,
            2 => Self::WhiteField,
            3 => Self::WhiteDots,
            _ => Self::BlackDots,
        }
    }
}

#[entity_behavior(class = "Horse")]
/// Vanilla horse with taming, variant + markings, riding and donkey breeding.
pub struct HorseEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    parts: AbstractHorseParts,
    entity_data: SyncMutex<HorseEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `HorseEntity`.
unsafe impl DowncastType for HorseEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/horse");
}

impl HorseEntity {
    /// Creates a new horse at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        let horse = Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        );
        horse.register_rearing_goal();
        horse
    }

    /// Reconstructs a horse from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        let horse = Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        );
        horse.register_rearing_goal();
        horse
    }

    /// Vanilla `AbstractHorse.registerGoals` priority-9 `RandomStandGoal`.
    ///
    /// Vanilla passes the horse to the goal constructor; Steel builds the goal
    /// selector before `&self` exists, so it is registered right afterwards.
    fn register_rearing_goal(&self) {
        self.mob_base
            .goal_selector()
            .lock()
            .add_goal(9, RandomStandGoal::new(self.ambient_stand_interval()));
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mob_base = MobBase::new();
        let ageable_base = AgeableMobBase::new();
        let animal_base = AnimalBase::new();
        AnimalBase::initialize_pathfinding_malus(&mob_base);
        let mut entity_data = HorseEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `AbstractHorse.registerGoals` + `addBehaviourGoals`, in
            // priority order. `RandomStandGoal` runs because `Horse` can rear.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(1, RunAroundLikeCrazyGoal::new(1.2));
            goal_selector.add_goal(1, MountPanicGoal::new(1.2));
            goal_selector.add_goal(2, BreedGoal::new(1.0));
            goal_selector.add_goal(3, TemptGoal::new(1.25, is_horse_tempt, false));
            goal_selector.add_goal(4, FollowParentGoal::new(1.0));
            goal_selector.add_goal(6, WaterAvoidingRandomStrollGoal::new(0.7));
            goal_selector.add_goal(7, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            animal_base,
            parts: AbstractHorseParts::new(true),
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Vanilla `AbstractHorse.canPerformRearing` (never disabled for `Horse`).
    const fn can_perform_rearing(&self) -> bool {
        true
    }

    /// Vanilla `Horse.getVariant`.
    #[must_use]
    pub fn variant(&self) -> HorseVariant {
        HorseVariant::by_id(*self.entity_data.lock().id_type_variant.get() & 0xFF)
    }

    /// Vanilla `Horse.getMarkings`.
    #[must_use]
    pub fn markings(&self) -> HorseMarkings {
        HorseMarkings::by_id((*self.entity_data.lock().id_type_variant.get() & 0xFF00) >> 8)
    }

    /// Vanilla `Horse.getTypeVariant`.
    #[must_use]
    pub fn type_variant(&self) -> i32 {
        *self.entity_data.lock().id_type_variant.get()
    }

    /// Vanilla `Horse.setTypeVariant`.
    pub fn set_type_variant(&self, variant: i32) {
        self.entity_data.lock().id_type_variant.set(variant);
    }

    /// Vanilla `Horse.setVariantAndMarkings`.
    pub fn set_variant_and_markings(&self, variant: HorseVariant, markings: HorseMarkings) {
        self.set_type_variant(variant.id() & 0xFF | (markings.id() << 8 & 0xFF00));
    }

    /// Vanilla `Horse.setVariant`.
    pub fn set_variant(&self, variant: HorseVariant) {
        self.set_type_variant(variant.id() & 0xFF | self.type_variant() & !0xFF);
    }

    /// Sets markings while keeping the variant (`Horse.setVariantAndMarkings` half).
    pub fn set_markings(&self, markings: HorseMarkings) {
        self.set_type_variant((markings.id() << 8 & 0xFF00) | (self.type_variant() & 0xFF));
    }

    /// Vanilla `Horse.randomizeAttributes`.
    fn randomize_attributes(&self) {
        let mut attributes = self.attributes().lock();
        attributes.set_base_value(
            vanilla_attributes::MAX_HEALTH,
            f64::from(generate_max_health()),
        );
        attributes.set_base_value(vanilla_attributes::MOVEMENT_SPEED, generate_speed());
        attributes.set_base_value(vanilla_attributes::JUMP_STRENGTH, generate_jump_strength());
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

    /// Runs a shared `equine` helper against a detached copy of the synchronized
    /// `AbstractHorse` layer.
    ///
    /// Every shared helper that takes `&mut AbstractHorseEntityData` also reads
    /// health, age and level back through the entity (`getHealth`, `isBaby`,
    /// `level`, ...), which re-enters this entity's synced-data mutex. Those
    /// helpers only ever write the `id_flags` byte, so the flags are detached,
    /// mutated and merged back instead of handing out the live guard.
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

    /// Returns whether the stack is vanilla horse food.
    #[must_use]
    pub fn is_food(item_stack: &ItemStack) -> bool {
        is_horse_food(item_stack)
    }

    /// Vanilla `Horse.canMate`: `Horse` or `Donkey` partners only.
    #[must_use]
    pub fn can_mate_horse(&self, partner: &dyn Animal) -> bool {
        self.with_horse_data(|data| can_mate_horse_or_donkey(self, partner, data))
    }

    /// Vanilla `AbstractHorse.canParent`.
    #[must_use]
    pub fn can_parent(&self) -> bool {
        self.with_horse_data(|data| can_parent(self, data))
    }

    /// Vanilla `AbstractHorse.isTamed`.
    #[must_use]
    pub fn is_tamed(&self) -> bool {
        is_tamed(&self.entity_data.lock().abstract_horse)
    }

    /// Vanilla `AbstractHorse.setTamed`.
    pub fn set_tamed(&self, tamed: bool) {
        set_flag(
            &mut self.entity_data.lock().abstract_horse,
            FLAG_TAME,
            tamed,
        );
    }

    /// Vanilla `AbstractHorse.isEating`.
    #[must_use]
    pub fn is_eating(&self) -> bool {
        is_eating(&self.entity_data.lock().abstract_horse)
    }

    /// Vanilla `AbstractHorse.setEating`.
    pub fn set_eating(&self, eating: bool) {
        set_eating(&mut self.entity_data.lock().abstract_horse, eating);
    }

    /// Vanilla `AbstractHorse.isBred`.
    #[must_use]
    pub fn is_bred(&self) -> bool {
        is_bred(&self.entity_data.lock().abstract_horse)
    }

    /// Vanilla `AbstractHorse.setBred`.
    pub fn set_bred(&self, bred: bool) {
        set_bred(&mut self.entity_data.lock().abstract_horse, bred);
    }

    /// Vanilla `AbstractHorse.getTemper`.
    #[must_use]
    pub fn temper(&self) -> i32 {
        self.parts.temper()
    }

    /// Vanilla `AbstractHorse.setTemper`.
    pub fn set_temper(&self, temper: i32) {
        self.parts.set_temper(temper);
    }

    /// Vanilla `AbstractHorse.getOwnerReference`, reduced to the owner UUID.
    #[must_use]
    pub fn owner(&self) -> Option<uuid::Uuid> {
        self.parts.owner()
    }

    /// Vanilla `AbstractHorse.setOwner`.
    pub fn set_owner(&self, owner: Option<uuid::Uuid>) {
        self.parts.set_owner(owner);
    }

    /// Vanilla `AbstractHorse.hasInventoryChanged` successor: the mount inventory.
    #[must_use]
    pub fn carried_items(&self) -> &Shared<InventoryCarrier> {
        self.parts.inventory()
    }

    /// Vanilla `AbstractHorse.openCustomInventoryScreen`.
    pub fn open_custom_inventory_screen(&self, player: &Player) {
        self.with_horse_data(|data| {
            open_custom_inventory_screen(self, &self.parts, data, player, 0)
        });
    }

    /// Feeds the horse (`AbstractHorse.fedFood`) with its per-age eat sound.
    fn feed(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let eating_sound = if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_EAT
        } else {
            &sound_events::ENTITY_HORSE_EAT
        };
        self.with_horse_data(|data| {
            fed_food(self, &self.parts, data, player, hand, Some(eating_sound))
        })
    }

    /// Vanilla `Horse.mobInteract`.
    fn horse_mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
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
                if HorseEntity::is_food(&item_stack) {
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
            }
        }

        self.abstract_horse_interact(player, hand)
    }

    /// Vanilla `AbstractHorse.mobInteract` tail.
    fn abstract_horse_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.with_horse_data(|data| {
            abstract_horse_mob_interact(self, &self.parts, data, player, hand, 0)
        })
    }
}

impl Entity for HorseEntity {
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
            BABY_DIMENSIONS.scale(scale)
        } else if self.entity_type.fixed {
            self.entity_type.dimensions
        } else {
            self.entity_type.dimensions.scale(scale)
        }
    }

    fn controlling_passenger(&self) -> Option<SharedEntity> {
        controlling_passenger(self)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.update_dirty_mob_effect_entity_data();
    }

    fn max_up_step(&self) -> f32 {
        max_up_step(self)
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        play_step_sound(
            self,
            &self.parts,
            &sound_events::ENTITY_BABY_HORSE_STEP,
            &sound_events::ENTITY_HORSE_STEP,
            &sound_events::ENTITY_HORSE_GALLOP,
        );
    }

    /// Vanilla `AbstractHorse.isPushable`: only riderless horses get pushed.
    fn is_pushable(&self) -> bool {
        !self.is_vehicle()
    }

    /// Vanilla `AbstractHorse.causeFallDamage`: the landing sound, then the
    /// shared living fall-damage path.
    fn cause_fall_damage(
        self: Arc<Self>,
        fall_distance: f64,
        damage_modifier: f32,
        source: &DamageSource,
    ) -> bool {
        if fall_distance > 1.0 {
            self.play_sound(
                if AgeableMob::is_baby(self.as_ref()) {
                    &sound_events::ENTITY_BABY_HORSE_LAND
                } else {
                    &sound_events::ENTITY_HORSE_LAND
                },
                0.4,
                1.0,
            );
        }

        LivingEntity::cause_living_fall_damage(
            self.as_ref(),
            fall_distance,
            damage_modifier,
            source,
        )
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);
        self.save_animal(nbt);
        self.parts
            .save_abstract_horse(&self.entity_data.lock().abstract_horse, nbt);
        // Vanilla `Horse.addAdditionalSaveData`.
        nbt.insert("Variant", self.type_variant());
        save_legacy_equipment(self, nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.parts
            .load_abstract_horse(&mut self.entity_data.lock().abstract_horse, &nbt);
        // Vanilla `Horse.readAdditionalSaveData`.
        self.set_type_variant(nbt.int("Variant").unwrap_or(0));
        load_legacy_equipment(self, &nbt);
    }
}

impl LivingEntity for HorseEntity {
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
        0.8
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_HURT
        } else {
            &sound_events::ENTITY_HORSE_HURT
        })
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_DEATH
        } else {
            &sound_events::ENTITY_HORSE_DEATH
        })
    }

    /// Vanilla `Horse.canUseSlot`: horses accept every equipment slot.
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
        super::equip_sound(slot).or_else(|| LivingEntity::equip_sound(self, slot, stack))
    }

    /// Vanilla `Horse.hurtArmor`: horse armor absorbs damage in the body slot.
    fn hurt_armor(&self, source: &DamageSource, damage: f32) {
        self.do_hurt_equipment(source, damage, &[EquipmentSlot::Body]);
    }

    /// Vanilla `AbstractHorse.dropEquipment`: carried inventory stacks drop too.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        super::drop_carried_items(self, &self.parts);
    }

    fn is_immobile(&self) -> bool {
        self.default_is_immobile() && self.is_vehicle() && self.is_saddled()
            || is_immobile_from_state(&self.entity_data.lock().abstract_horse)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
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

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        // Vanilla `AbstractHorse.aiStep` bookkeeping runs before movement.
        self.with_horse_data(|data| tick_abstract_horse(self, &self.parts, data, true));

        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for HorseEntity {
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

    /// Vanilla `Horse.getBreedOffspring`: a donkey cross yields a mule.
    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        if partner.downcast_ref::<super::DonkeyEntity>().is_some() {
            let baby = ENTITIES.create(
                &vanilla_entities::MULE,
                next_entity_id(),
                self.position(),
                Arc::downgrade(world),
            )?;
            set_offspring_attributes(self, partner, baby.as_animal()?);
            return Some(baby);
        }

        let baby = ENTITIES.create(
            &vanilla_entities::HORSE,
            next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        )?;
        let baby_horse = baby.downcast_ref::<HorseEntity>()?;
        let partner_horse = partner.downcast_ref::<HorseEntity>();

        // Vanilla `Horse.getBreedOffspring`: 4/9 self, 4/9 partner, 1/9 random.
        let variant = match rand::random_range(0..9) {
            0..=3 => self.variant(),
            4..=7 => partner_horse.map_or(self.variant(), HorseEntity::variant),
            _ => HorseVariant::VALUES[rand::random_range(0..HorseVariant::VALUES.len())],
        };
        let markings = match rand::random_range(0..5) {
            0..=1 => self.markings(),
            2..=3 => partner_horse.map_or(self.markings(), HorseEntity::markings),
            _ => HorseMarkings::VALUES[rand::random_range(0..HorseMarkings::VALUES.len())],
        };
        baby_horse.set_variant_and_markings(variant, markings);
        set_offspring_attributes(self, partner, baby.as_animal()?);
        Some(baby)
    }
}

impl Animal for HorseEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        HorseEntity::is_food(item_stack)
    }

    fn can_mate(&self, partner: &dyn Animal) -> bool {
        self.can_mate_horse(partner)
    }

    /// Vanilla `AbstractHorse.getAmbientSoundInterval`.
    fn ambient_sound_interval_animal(&self) -> i32 {
        400
    }

    fn play_eating_sound(&self) {
        let sound = if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_EAT
        } else {
            &sound_events::ENTITY_HORSE_EAT
        };
        self.play_sound(sound, 1.0, 1.0);
    }
}

impl Equine for HorseEntity {
    fn is_tamed(&self) -> bool {
        HorseEntity::is_tamed(self)
    }

    fn temper(&self) -> i32 {
        self.parts.temper()
    }

    fn max_temper(&self) -> i32 {
        MAX_TEMPER
    }

    fn modify_temper(&self, amount: i32) -> i32 {
        self.parts.modify_temper(amount)
    }

    fn tame_with_name(&self, player: &SharedEntity) {
        self.with_horse_data(|data| super::tame_with_name(self, &self.parts, data, player));
    }

    fn make_mad(&self, _mob: &SharedEntity) {
        let angry_sound = if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_ANGRY
        } else {
            &sound_events::ENTITY_HORSE_ANGRY
        };
        self.with_horse_data(|data| {
            make_mad(
                self,
                &self.parts,
                data,
                self.can_perform_rearing(),
                Some(angry_sound),
            );
        });
    }

    fn stand_if_possible(&self) {
        let can_perform_rearing = self.can_perform_rearing();
        self.with_horse_data(|data| {
            stand_if_possible(self, &self.parts, data, can_perform_rearing);
        });
    }

    fn ambient_stand_sound(&self) -> Option<SoundEventRef> {
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_AMBIENT
        } else {
            &sound_events::ENTITY_HORSE_AMBIENT
        })
    }
}

impl Mob for HorseEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
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
        Some(if AgeableMob::is_baby(self) {
            &sound_events::ENTITY_BABY_HORSE_AMBIENT
        } else {
            &sound_events::ENTITY_HORSE_AMBIENT
        })
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = default_horse_group_data(group_data);

        // Vanilla `Horse.finalizeSpawn` picks a group variant; Steel's
        // `SpawnGroupData` has no horse-variant slot, so each horse rolls its own.
        let variant = HorseVariant::VALUES[rand::random_range(0..HorseVariant::VALUES.len())];
        let markings = HorseMarkings::VALUES[rand::random_range(0..HorseMarkings::VALUES.len())];
        self.set_variant_and_markings(variant, markings);

        self.randomize_attributes();
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.horse_mob_interact(player, hand)
    }

    /// Vanilla `AbstractHorse.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        6
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for HorseEntity {}

/// Vanilla `Horse` tempt items: `ItemTags.HORSE_TEMPT_ITEMS`.
fn is_horse_tempt(item_stack: &ItemStack) -> bool {
    REGISTRY
        .items
        .is_in_tag(item_stack.item(), &ItemTag::HORSE_TEMPT_ITEMS)
}
