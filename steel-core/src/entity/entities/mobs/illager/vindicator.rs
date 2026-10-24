//! Vanilla `Vindicator` (vanilla `net.minecraft.world.entity.monster.illager.Vindicator`).

use std::iter;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::enchantment::table::RandRngAdapter;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::VindicatorEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{Downcast as _, DowncastType, DowncastTypeKey};
use text_components::TextComponent;

use super::abstract_illager::{IllagerArmPose, RaiderKeys};
use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, Goal, GoalControls, HurtByTargetGoal, LookAtPlayerGoal,
    MeleeAttackGoal, NearestAttackableTargetGoal, RaiderOpenDoorGoal, RandomStrollGoal,
    VindicatorBreakDoorGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::spawn_equipment;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::raid::vindicator_raid_enchant_provider;
use crate::entity::{
    ENTITIES, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;
/// Vanilla `Vindicator.TAG_JOHNNY`.
const TAG_JOHNNY: &str = "Johnny";

/// Vanilla vindicator entity.
#[entity_behavior(class = "Vindicator")]
pub struct VindicatorEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<VindicatorEntityData>,
    is_johnny: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `VindicatorEntity`.
unsafe impl DowncastType for VindicatorEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/vindicator");
}

impl VindicatorEntity {
    /// Creates a new vindicator entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved vindicator entity.
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
        mob_base.set_xp_reward(XP_REWARD);
        let mut entity_data = VindicatorEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            // Vanilla goal 1 avoids `Creaking`; Steel has no creaking factory yet, so the
            // avoidance is registered against the creaking entity type only if it exists.
            if ENTITIES.has_factory(&vanilla_entities::CREAKING) {
                goals.add_goal(
                    1,
                    AvoidEntityGoal::with_selector(8.0, 1.0, 1.2, |target, _| {
                        target.entity_type() == &vanilla_entities::CREAKING
                    }),
                );
            }
            // Vanilla goal 2 `VindicatorBreakDoorGoal` breaks doors while raiding on normal
            // or hard difficulty; goal 3 `RaiderOpenDoorGoal` opens doors while raiding.
            goals.add_goal(2, VindicatorBreakDoorGoal::new());
            goals.add_goal(3, RaiderOpenDoorGoal::new());
            goals.add_goal(4, super::raider_goals::hold_ground_attack(10.0));
            goals.add_goal(5, MeleeAttackGoal::new(1.0, false));
            goals.add_goal(8, RandomStrollGoal::new(0.6));
            goals.add_goal(9, LookAtPlayerGoal::new(3.0));
            goals.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(8.0, 1.0, |_, _| true),
            );

            let mut targets = mob_base.target_selector().lock();
            targets.add_goal(
                1,
                HurtByTargetGoal::new()
                    .with_ignored_damage_types(RaiderKeys::ALL)
                    .set_alert_others(iter::empty()),
            );
            targets.add_goal(
                2,
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(true, super::abstract_illager::is_target_villager),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    true,
                    super::abstract_illager::is_target_iron_golem,
                ),
            );
            targets.add_goal(4, VindicatorJohnnyAttackGoal::new());
        }

        super::raider_goals::register_raider_goals(&mob_base);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            raider_base: RaiderBase::new(),
            entity_data: SyncMutex::new(entity_data),
            is_johnny: SyncMutex::new(false),
        }
    }

    /// Vanilla `Vindicator.isJohnny`.
    #[must_use]
    pub fn is_johnny(&self) -> bool {
        *self.is_johnny.lock()
    }

    /// Vanilla `Vindicator.setCustomName`: a name of "Johnny" turns the vindicator hostile to
    /// everything.
    pub fn set_custom_name_johnny(&self, name: Option<&TextComponent>) {
        if self.is_johnny() {
            return;
        }
        // Vanilla compares the rendered name (`Component.getString()`); Steel's
        // `TextComponent` renders through `Display`.
        let is_johnny = name.is_some_and(|name| name.to_string() == TAG_JOHNNY);
        if is_johnny {
            *self.is_johnny.lock() = true;
        }
    }

    /// Vanilla `Vindicator.getArmPose`.
    #[must_use]
    pub fn arm_pose(&self) -> IllagerArmPose {
        if self.is_aggressive() {
            IllagerArmPose::Attacking
        } else if <Self as Raider>::is_celebrating(self) {
            IllagerArmPose::Celebrating
        } else {
            IllagerArmPose::Crossed
        }
    }

    fn update_dirty_mob_effect_entity_data(&self) {
        if !self.living_base.take_effects_dirty() {
            return;
        }
        let display = self.living_base.mob_effect_display_state();
        {
            let mut entity_data = self.entity_data.lock();
            let living = entity_data.raider_mut().mob_mut().living_entity_mut();
            living.effect_particles.set(display.particles);
            living.effect_ambience.set(display.ambient);
        }
        self.entity_data.set_base_invisible_flag(display.invisible);
    }
}

impl Entity for VindicatorEntity {
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.raider_base.save(nbt);
        if self.is_johnny() {
            nbt.insert("Johnny", true);
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
        *self.is_johnny.lock() = nbt.byte("Johnny").is_some_and(|value| value != 0);
    }
}

impl LivingEntity for VindicatorEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn get_health(&self) -> f32 {
        *self
            .entity_data
            .lock()
            .raider()
            .mob()
            .living_entity()
            .health
            .get()
    }

    fn set_health(&self, health: f32) {
        let clamped = health.clamp(0.0, self.get_max_health());
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .living_entity_mut()
            .health
            .set(clamped);
    }

    fn sound_volume(&self) -> f32 {
        0.4
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VINDICATOR_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VINDICATOR_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Raider::raider_ai_step(self, entity);
        Monster::monster_ai_step(self, entity)
    }

    fn on_death(&self, source: &DamageSource) {
        Raider::raider_on_death(self, source);
    }
}

impl Mob for VindicatorEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_VINDICATOR_AMBIENT)
    }

    fn mob_flags(&self) -> i8 {
        *self.entity_data.lock().raider().mob().mob_flags.get()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.entity_data
            .lock()
            .raider_mut()
            .mob_mut()
            .mob_flags
            .set(flags);
    }

    /// Vanilla `Vindicator.customServerAiStep`: opening doors only while the position is
    /// raided.
    fn custom_server_ai_step(&self) {
        let Some(world) = self.level() else {
            return;
        };
        if self.is_no_ai() {
            return;
        }
        let can_open_doors = world.is_raided(self.block_position());
        self.mob_base()
            .navigation()
            .lock()
            .set_can_open_doors(can_open_doors);
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let group_data = self.finalize_spawn_mob_base(world, spawn_reason, group_data);
        self.mob_base().navigation().lock().set_can_open_doors(true);
        let mut random = LegacyRandom::from_seed(rand::random());
        let difficulty_modifier = world
            .current_difficulty_at(self.block_position())
            .special_multiplier();
        // Vanilla only arms a vindicator that is not already in a raid; raid buffs equip the
        // axe later.
        if self.current_raid(world).is_none() {
            self.living_base.equipment().lock().set(
                EquipmentSlot::MainHand,
                ItemStack::new(&vanilla_items::IRON_AXE),
            );
        }
        spawn_equipment::populate_default_equipment_enchantments(
            self,
            &mut random,
            difficulty_modifier,
        );
        group_data
    }

    fn requires_custom_persistence(&self) -> bool {
        Raider::raider_requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        Raider::raider_remove_when_far_away(self, dist_sqr)
    }
}

impl Monster for VindicatorEntity {}

impl PathfinderMob for VindicatorEntity {}

impl Raider for VindicatorEntity {
    fn raider_base(&self) -> &RaiderBase {
        &self.raider_base
    }

    fn is_celebrating(&self) -> bool {
        *self.entity_data.lock().raider().is_celebrating.get()
    }

    fn set_celebrating(&self, celebrating: bool) {
        self.entity_data
            .lock()
            .raider_mut()
            .is_celebrating
            .set(celebrating);
    }

    fn apply_raid_buffs(
        &self,
        world: &Arc<World>,
        raid: &RaidSnapshot,
        wave: i32,
        _is_captain: bool,
    ) {
        // Vanilla `Vindicator.applyRaidBuffs`: always equip an iron axe, and enchant it from
        // `RAID_VINDICATOR_POST_WAVE_5` (past the normal wave gate) or `RAID_VINDICATOR` when
        // the raid enchant roll succeeds.
        let mut axe = ItemStack::new(&vanilla_items::IRON_AXE);
        if rand::random::<f32>() <= raid.enchant_odds {
            let provider = vindicator_raid_enchant_provider(wave);
            provider.enchant(
                &mut axe,
                world
                    .current_difficulty_at(self.block_position())
                    .special_multiplier(),
                &mut RandRngAdapter(&mut rand::rng()),
            );
        }
        self.living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, axe);
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_VINDICATOR_CELEBRATE
    }
}

/// Vanilla `Vindicator.VindicatorJohnnyAttackGoal`: a "Johnny" vindicator attacks every
/// living entity it can reach.
struct VindicatorJohnnyAttackGoal {
    inner: NearestAttackableTargetGoal,
}

impl VindicatorJohnnyAttackGoal {
    fn new() -> Self {
        Self {
            inner: NearestAttackableTargetGoal::new_with_interval(0, true, true, |target, _| {
                !target.is_spectator()
            }),
        }
    }
}

impl Goal for VindicatorJohnnyAttackGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let is_johnny = mob
            .downcast_ref::<VindicatorEntity>()
            .is_some_and(VindicatorEntity::is_johnny);
        is_johnny && self.inner.can_use(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        self.inner.can_continue_to_use(mob)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        self.inner.start(mob);
        mob.set_no_action_time(0);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.inner.stop(mob);
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, entity: &SharedEntity) {
        self.inner.tick(mob, entity);
    }
}
