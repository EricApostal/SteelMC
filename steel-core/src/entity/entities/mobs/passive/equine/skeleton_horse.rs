//! Vanilla `SkeletonHorse`: the undead trap horse.
//!
//! The trap goal (`SkeletonTrapGoal`) is registered permanently here and gated
//! on `is_trap`, instead of vanilla's dynamic add/remove in `setTrap`; the goal
//! selector has no goal-identity removal for a goal that is only conditionally
//! active, and the observable behaviour is identical.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::entity_type::{
    EntityAttachmentPoint, EntityAttachments, EntityDimensions, EntityTypeRef,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::{AbstractHorseEntityData, SkeletonHorseEntityData};
use steel_registry::{sound_events, vanilla_entities};
use steel_utils::locks::{Shared, SyncMutex};
use steel_utils::random::RandomSource;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use super::{
    AbstractHorseParts, Equine, FLAG_TAME, MAX_TEMPER, RandomStandGoal, RunAroundLikeCrazyGoal,
    abstract_horse_mob_interact, controlling_passenger, default_horse_group_data,
    generate_jump_strength, is_bred, is_eating, is_horse_food, is_immobile_from_state, is_tamed,
    load_legacy_equipment, make_mad, max_up_step, open_custom_inventory_screen, play_step_sound,
    ridden_input, ridden_speed, save_legacy_equipment, set_bred, set_eating, set_flag,
    stand_if_possible, tick_abstract_horse, tick_ridden,
};
use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    BreedGoal, FollowParentGoal, Goal, GoalControls, LookAtPlayerGoal, RandomLookAroundGoal,
    WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::PigEntity;
use crate::entity::npc::inventory_carrier::InventoryCarrier;
use crate::entity::{
    AgeableMob, AgeableMobBase, Animal, AnimalBase, ENTITIES, Entity, EntityBase, EntityBaseLoad,
    EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase,
    MoveResult, PathfinderMob, RemovalReason, SharedEntity, SpawnGroupData, next_entity_id,
};
use crate::inventory::equipment::EquipmentSlot;
use crate::player::Player;
use crate::world::World;

/// Vanilla `SkeletonHorse.TRAP_MAX_LIFE`.
const TRAP_MAX_LIFE: i32 = 18_000;
/// Vanilla `SkeletonTrapGoal` player detection radius.
const TRAP_PLAYER_RADIUS: f64 = 10.0;

const BABY_PASSENGER_ATTACHMENTS: [EntityAttachmentPoint; 1] =
    super::horse_baby_passenger_attachments(1.6);
const BABY_DIMENSIONS: EntityDimensions = EntityDimensions::new_with_attachments(
    1.396_484_4 * 0.7,
    1.6 * 0.7,
    1.52 * 0.7,
    EntityAttachments::new(&BABY_PASSENGER_ATTACHMENTS, &[], &[], &[]),
);

#[entity_behavior(class = "SkeletonHorse")]
/// Vanilla skeleton horse: undead horse that springs a skeleton ambush.
pub struct SkeletonHorseEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    animal_base: AnimalBase,
    parts: AbstractHorseParts,
    entity_data: SyncMutex<SkeletonHorseEntityData>,
    /// Vanilla `SkeletonHorse.isTrap`.
    is_trap: SyncMutex<bool>,
    /// Vanilla `SkeletonHorse.trapTime`.
    trap_time: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `SkeletonHorseEntity`.
unsafe impl DowncastType for SkeletonHorseEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/skeleton_horse");
}

impl SkeletonHorseEntity {
    /// Creates a new skeleton horse at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        let horse = Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        );
        horse.register_rearing_goal();
        horse
    }

    /// Reconstructs a skeleton horse from persisted base entity state.
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
        let mut entity_data = SkeletonHorseEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `AbstractHorse.registerGoals`; `SkeletonHorse.addBehaviourGoals`
            // is empty, so no `FloatGoal`, `MountPanicGoal` or `TemptGoal`.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, RunAroundLikeCrazyGoal::new(1.2));
            goal_selector.add_goal(1, SkeletonTrapGoal);
            goal_selector.add_goal(2, BreedGoal::new(1.0));
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
            is_trap: SyncMutex::new(false),
            trap_time: SyncMutex::new(0),
        }
    }

    /// Vanilla `AbstractHorse.canPerformRearing`.
    const fn can_perform_rearing(&self) -> bool {
        true
    }

    /// Vanilla `SkeletonHorse.isTrap`.
    #[must_use]
    pub fn is_trap(&self) -> bool {
        *self.is_trap.lock()
    }

    /// Vanilla `SkeletonHorse.setTrap`.
    ///
    /// The trap goal stays registered and checks [`Self::is_trap`] itself (see
    /// the module docs).
    pub fn set_trap(&self, trap: bool) {
        *self.is_trap.lock() = trap;
    }

    /// Vanilla `SkeletonHorse.trapTime`.
    #[must_use]
    pub fn trap_time(&self) -> i32 {
        *self.trap_time.lock()
    }

    /// Sets vanilla `SkeletonHorse.trapTime`.
    pub fn set_trap_time(&self, trap_time: i32) {
        *self.trap_time.lock() = trap_time;
    }

    /// Vanilla `SkeletonHorse.randomizeAttributes`: jump strength only.
    fn randomize_attributes(&self) {
        self.attributes().lock().set_base_value(
            steel_registry::vanilla_attributes::JUMP_STRENGTH,
            generate_jump_strength(),
        );
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

    /// Vanilla `AbstractHorse.inventory`.
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

    /// Vanilla `SkeletonHorse.mobInteract`: untamed skeleton horses ignore
    /// players; tamed ones fall through to `AbstractHorse.mobInteract`.
    ///
    /// Vanilla's skeleton horse has no food branch (`handleEating` is only
    /// reachable from `Horse`, `AbstractChestedHorse` and `ZombieHorse`).
    fn skeleton_horse_mob_interact(
        &self,
        player: &Player,
        hand: InteractionHand,
    ) -> InteractionResult {
        if !self.is_tamed() {
            return InteractionResult::Pass;
        }

        self.with_horse_data(|data| {
            abstract_horse_mob_interact(self, &self.parts, data, player, hand, 0)
        })
    }

    /// Vanilla `SkeletonHorse.checkSkeletonHorseSpawnRules`, the `SpawnPlacements`
    /// predicate for `EntityType.SKELETON_HORSE`.
    ///
    /// Non-spawner reasons fall back to the shared
    /// `Animal.checkAnimalSpawnRules`; spawner reasons skip the surface check
    /// and only require ignored light or sufficient brightness.
    #[must_use]
    pub fn check_skeleton_horse_spawn_rules(
        _entity_type: EntityTypeRef,
        level: &World,
        spawn_reason: EntitySpawnReason,
        pos: BlockPos,
        _random: &mut RandomSource,
    ) -> bool {
        if spawn_reason.is_spawner() {
            spawn_reason.ignores_light_requirements()
                || <Self as Animal>::is_bright_enough_to_spawn(level, pos)
        } else {
            <PigEntity as Animal>::check_animal_spawn_rules(level, spawn_reason, pos)
        }
    }
}

impl Entity for SkeletonHorseEntity {
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
        // Vanilla `SkeletonHorse.addAdditionalSaveData`.
        nbt.insert("SkeletonTrap", i8::from(self.is_trap()));
        nbt.insert("SkeletonTrapTime", self.trap_time());
        save_legacy_equipment(self, nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);
        self.load_animal(nbt);
        self.parts
            .load_abstract_horse(&mut self.entity_data.lock().abstract_horse, &nbt);
        // Vanilla `SkeletonHorse.readAdditionalSaveData`.
        self.set_trap(nbt.byte("SkeletonTrap").is_some_and(|value| value != 0));
        self.set_trap_time(nbt.int("SkeletonTrapTime").unwrap_or(0));
        load_legacy_equipment(self, &nbt);
    }

    /// Vanilla `SkeletonHorse.getSwimSound`.
    fn swim_sound(&self) -> SoundEventRef {
        if self.on_ground() {
            if !self.is_vehicle() {
                return &sound_events::ENTITY_SKELETON_HORSE_STEP_WATER;
            }

            let counter = self.parts.gallop_sound_counter() + 1;
            self.parts.set_gallop_sound_counter(counter);
            if counter > 5 && counter % 3 == 0 {
                return &sound_events::ENTITY_SKELETON_HORSE_GALLOP_WATER;
            }
            if counter <= 5 {
                return &sound_events::ENTITY_SKELETON_HORSE_STEP_WATER;
            }
        }

        &sound_events::ENTITY_SKELETON_HORSE_SWIM
    }

    /// Vanilla `SkeletonHorse.playSwimSound`: quieter splashes on the ground.
    fn play_swim_sound(&self, volume: f32) {
        let volume = if self.on_ground() {
            0.3
        } else {
            (volume * 25.0).min(0.1)
        };
        // Vanilla `Entity.playSwimSound` default body is inlined: a UFCS call
        // here would resolve back to this override and recurse.
        let pitch = 1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.4;
        self.play_sound(self.swim_sound(), volume, pitch);
    }
}

impl LivingEntity for SkeletonHorseEntity {
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
        Some(&sound_events::ENTITY_SKELETON_HORSE_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_HORSE_DEATH)
    }

    /// Vanilla `SkeletonHorse.canUseSlot`: every slot is usable.
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

    /// Vanilla `SkeletonHorse.hurtArmor`: body armor absorbs damage.
    fn hurt_armor(&self, source: &DamageSource, damage: f32) {
        self.do_hurt_equipment(source, damage, &[EquipmentSlot::Body]);
    }

    /// Vanilla `SkeletonHorse.getWaterSlowDown`.
    fn get_water_slow_down(&self) -> f32 {
        0.96
    }

    /// Vanilla `AbstractHorse.dropEquipment`: carried inventory stacks drop too.
    fn drop_custom_death_loot(&self, _source: &DamageSource, _killed_by_player: bool) {
        super::drop_carried_items(self, &self.parts);
    }

    /// Vanilla `AbstractHorse.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        6
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
        self.with_horse_data(|data| tick_abstract_horse(self, &self.parts, data, true));

        // Vanilla `SkeletonHorse.aiStep`: trap horses despawn after 18000 ticks
        // unless persistence was forced.
        if !self.is_persistence_required() && self.is_trap() {
            let mut trap_time = self.trap_time.lock();
            *trap_time += 1;
            if *trap_time >= TRAP_MAX_LIFE {
                drop(trap_time);
                self.set_removed(RemovalReason::Discarded);
                return None;
            }
        }

        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        Animal::tick_animal_love(self);
        result
    }
}

impl AgeableMob for SkeletonHorseEntity {
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

    /// Vanilla `SkeletonHorse.canAgeUp` is false.
    fn can_age_up(&self) -> bool {
        false
    }

    /// Vanilla `SkeletonHorse.getBreedOffspring`.
    fn get_breed_offspring(
        &self,
        world: &Arc<World>,
        _partner: &dyn AgeableMob,
    ) -> Option<SharedEntity> {
        ENTITIES.create(
            &vanilla_entities::SKELETON_HORSE,
            next_entity_id(),
            self.position(),
            Arc::downgrade(world),
        )
    }
}

impl Animal for SkeletonHorseEntity {
    fn animal_base(&self) -> &AnimalBase {
        &self.animal_base
    }

    fn is_food(&self, item_stack: &ItemStack) -> bool {
        SkeletonHorseEntity::is_food(item_stack)
    }

    /// Vanilla `AbstractHorse.canMate` is `false`; the skeleton horse never breeds.
    fn can_mate(&self, _partner: &dyn Animal) -> bool {
        false
    }

    /// Vanilla `AbstractHorse.getAmbientSoundInterval`.
    fn ambient_sound_interval_animal(&self) -> i32 {
        400
    }
}

impl Equine for SkeletonHorseEntity {
    fn is_tamed(&self) -> bool {
        SkeletonHorseEntity::is_tamed(self)
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
        // Vanilla `SkeletonHorse` has no angry sound (`getAngrySound` is null).
        let can_perform_rearing = self.can_perform_rearing();
        self.with_horse_data(|data| {
            make_mad(self, &self.parts, data, can_perform_rearing, None);
        });
    }

    fn stand_if_possible(&self) {
        let can_perform_rearing = self.can_perform_rearing();
        self.with_horse_data(|data| {
            stand_if_possible(self, &self.parts, data, can_perform_rearing);
        });
    }

    fn ambient_stand_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_SKELETON_HORSE_AMBIENT)
    }
}

impl Mob for SkeletonHorseEntity {
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
        // GAP: vanilla swaps to `SKELETON_HORSE_AMBIENT_WATER` while the eyes are
        // in water; Steel has no `isEyeInFluid` accessor on the shared path.
        Some(&sound_events::ENTITY_SKELETON_HORSE_AMBIENT)
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = default_horse_group_data(group_data);
        self.randomize_attributes();
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        self.skeleton_horse_mob_interact(player, hand)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data.lock().mob_mut().mob_flags.set(flags);
    }
}

impl PathfinderMob for SkeletonHorseEntity {}

/// Vanilla `SkeletonTrapGoal`: converts the trap horse into a tamed horse and
/// springs a skeleton ambush.
///
/// GAP: spawning the lightning bolt and the skeleton riders needs the
/// `LIGHTNING_BOLT`/`SKELETON` factories plus jockey equipment wiring, which are
/// owned by the HostileMobs/Projectiles lanes. Until then the goal performs the
/// state transition vanilla does (`setTrap(false)`, `setTamed(true)`,
/// `setAge(0)`) and logs the missing ambush.
struct SkeletonTrapGoal;

impl Goal for SkeletonTrapGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(horse) = mob.downcast_ref::<SkeletonHorseEntity>() else {
            return false;
        };
        if !horse.is_trap() {
            return false;
        }

        mob.level().is_some_and(|world| {
            world
                .nearest_player_distance_sqr(mob.position())
                .is_some_and(|dist_sqr| dist_sqr <= TRAP_PLAYER_RADIUS * TRAP_PLAYER_RADIUS)
        })
    }

    fn can_continue_to_use(&mut self, _mob: &dyn PathfinderMob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(horse) = mob.downcast_ref::<SkeletonHorseEntity>() else {
            return;
        };

        // Vanilla `SkeletonTrapGoal.tick` state transition.
        horse.set_trap(false);
        horse.set_tamed(true);
        horse.set_age(0);

        log::debug!(
            "skeleton trap sprung at {:?} (lightning + skeleton riders pending factory wiring)",
            mob.block_position()
        );
    }
}
