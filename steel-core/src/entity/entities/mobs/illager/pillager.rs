//! Vanilla `Pillager` (vanilla `net.minecraft.world.entity.monster.illager.Pillager`).

use std::iter;
use std::ptr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::enchantment::provider::enchant_item_from_provider;
use steel_registry::enchantment::table::RandRngAdapter;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::PillagerEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::random::Random as _;
use steel_utils::random::legacy_random::LegacyRandom;
use steel_utils::{DowncastType, DowncastTypeKey, Identifier};

use super::abstract_illager::{IllagerArmPose, RaiderKeys};
use crate::behavior::items::{MOB_ARROW_POWER, perform_mob_crossbow_attack};
use crate::entity::ai::goal::{
    FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal, RandomStrollGoal,
    RangedAttackMob,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::spawn_equipment;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::pillager_raid_enchant_provider;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::world::World;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
const XP_REWARD: i32 = 5;

/// Vanilla pillager entity.
#[entity_behavior(class = "Pillager")]
pub struct PillagerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<PillagerEntityData>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PillagerEntity`.
unsafe impl DowncastType for PillagerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/pillager");
}

impl PillagerEntity {
    /// Creates a new pillager entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved pillager entity.
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
        let mut entity_data = PillagerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            goals.add_goal(2, super::raider_goals::hold_ground_attack(10.0));
            goals.add_goal(3, super::raider_goals::crossbow_attack(1.0, 8.0));
            goals.add_goal(8, RandomStrollGoal::new(0.6));
            goals.add_goal(9, LookAtPlayerGoal::new(15.0));
            goals.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(15.0, 1.0, |_, _| true),
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
                NearestAttackableTargetGoal::new(
                    false,
                    super::abstract_illager::is_target_villager,
                ),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    true,
                    super::abstract_illager::is_target_iron_golem,
                ),
            );
        }

        super::raider_goals::register_raider_goals(&mob_base);

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            raider_base: RaiderBase::new(),
            entity_data: SyncMutex::new(entity_data),
        }
    }

    /// Vanilla `Pillager.isChargingCrossbow`.
    #[must_use]
    pub fn is_charging_crossbow(&self) -> bool {
        *self.entity_data.lock().is_charging_crossbow.get()
    }

    /// Vanilla `Pillager.setChargingCrossbow`.
    pub fn set_charging_crossbow(&self, charging: bool) {
        self.entity_data.lock().is_charging_crossbow.set(charging);
    }

    /// Vanilla `Pillager.getArmPose`.
    #[must_use]
    pub fn arm_pose(&self) -> IllagerArmPose {
        if self.is_charging_crossbow() {
            IllagerArmPose::CrossbowCharge
        } else if self.is_holding(&mut |stack| {
            ptr::eq(stack.item(), ptr::from_ref(&*vanilla_items::CROSSBOW))
        }) {
            IllagerArmPose::CrossbowHold
        } else if self.is_aggressive() {
            IllagerArmPose::Attacking
        } else {
            IllagerArmPose::Neutral
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

impl Entity for PillagerEntity {
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
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
        self.set_can_pick_up_loot(true);
    }
}

impl LivingEntity for PillagerEntity {
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
        Some(&sound_events::ENTITY_PILLAGER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PILLAGER_DEATH)
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

impl Mob for PillagerEntity {
    fn mob_base(&self) -> &MobBase {
        &self.mob_base
    }

    /// Vanilla `Pillager.getMaxSpawnClusterSize`.
    fn max_spawn_cluster_size(&self) -> i32 {
        1
    }

    /// Vanilla `Pillager.finalizeSpawn`: crossbow in the main hand, then the
    /// shared weapon/armor enchantment pass, then `Mob`'s follow-range bonus.
    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        let mut random = LegacyRandom::from_seed(rand::random());
        let difficulty_modifier = world
            .current_difficulty_at(self.block_position())
            .special_multiplier();

        self.living_base.equipment().lock().set(
            EquipmentSlot::MainHand,
            ItemStack::new(&vanilla_items::CROSSBOW),
        );
        spawn_equipment::enchant_spawned_weapon(self, &mut random, difficulty_modifier);
        // Vanilla `Pillager.enchantSpawnedWeapon` adds the raid crossbow roll after
        // `Mob.enchantSpawnedWeapon`.
        if random.next_i32_bounded(300) == 0
            && self
                .equipment_in_slot(EquipmentSlot::MainHand)
                .is(&vanilla_items::CROSSBOW)
        {
            let mut crossbow = self.equipment_in_slot(EquipmentSlot::MainHand);
            enchant_item_from_provider(
                &mut crossbow,
                &Identifier::vanilla_static("pillager_spawn_crossbow"),
                difficulty_modifier,
                &mut random,
            );
            self.living_base
                .equipment()
                .lock()
                .set(EquipmentSlot::MainHand, crossbow);
        }
        spawn_equipment::enchant_spawned_armor(self, &mut random, difficulty_modifier);

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        PathfinderMob::tick_pathfinder_goal_selectors(self, entity);
    }

    fn tick_path_navigation(&self) {
        PathfinderMob::tick_pathfinder_path_navigation(self);
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_PILLAGER_AMBIENT)
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

    fn requires_custom_persistence(&self) -> bool {
        Raider::raider_requires_custom_persistence(self)
    }

    fn remove_when_far_away(&self, dist_sqr: f64) -> bool {
        Raider::raider_remove_when_far_away(self, dist_sqr)
    }
}

impl Monster for PillagerEntity {}

impl PathfinderMob for PillagerEntity {}

/// Vanilla `Pillager` is a `CrossbowAttackMob`: the raid goal releases its bolt
/// through `CrossbowAttackMob.performCrossbowAttack`, which is what
/// `Pillager.performRangedAttack` wraps.
impl RangedAttackMob for PillagerEntity {
    /// Vanilla `Pillager.performRangedAttack`: `performCrossbowAttack(this, 1.6F)`
    /// aimed at `target`.
    ///
    /// Vanilla's `1.0F` argument only satisfies the `RangedAttackMob` signature;
    /// the pillager discards it and hands over `CrossbowItem.MOB_ARROW_POWER`.
    /// `onCrossbowAttackPerformed` then clears the no-action time, whether or not
    /// the pillager actually held a crossbow.
    fn perform_ranged_attack(&self, target: &SharedEntity, _power: f32) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        let Some(shooter) = world.get_entity_by_id(self.id()) else {
            return false;
        };

        let released = perform_mob_crossbow_attack(
            &world,
            &shooter,
            target,
            MOB_ARROW_POWER,
            self.ranged_attack_uncertainty(&world),
        );
        self.set_no_action_time(0);
        released
    }
}

impl Raider for PillagerEntity {
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
        // Vanilla `Pillager.applyRaidBuffs`: roll the raid enchant odds, then enchant a fresh
        // crossbow from `RAID_PILLAGER_POST_WAVE_3`/`RAID_PILLAGER_POST_WAVE_5` and equip it.
        // Below both wave gates vanilla leaves the crossbow alone.
        if rand::random::<f32>() > raid.enchant_odds {
            return;
        }
        let Some(provider) = pillager_raid_enchant_provider(wave) else {
            return;
        };

        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        provider.enchant(
            &mut crossbow,
            world
                .current_difficulty_at(self.block_position())
                .special_multiplier(),
            &mut RandRngAdapter(&mut rand::rng()),
        );
        self.living_base()
            .equipment()
            .lock()
            .set(EquipmentSlot::MainHand, crossbow);
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_PILLAGER_CELEBRATE
    }
}

/// Vanilla pillager spawn equipment/sounds are registered through the entity factory.
const _: () = {
    assert!(XP_REWARD == 5);
};

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::data_components::components::ChargedProjectiles;
    use steel_registry::data_components::vanilla_components::CHARGED_PROJECTILES;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::item_stack_template::ItemStackTemplate;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};
    use steel_utils::types::InteractionHand;
    use steel_utils::{ChunkPos, WorldAabb};

    use super::PillagerEntity;
    use crate::behavior::init_behaviors;
    use crate::behavior::items::{MOB_ARROW_POWER, is_charged};
    use crate::entity::ai::goal::RangedAttackMob;
    use crate::entity::{Entity as _, LivingEntity, SharedEntity};
    use crate::player::Player;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };

    const PILLAGER_ID: i32 = 5_151;
    const TARGET_ID: i32 = 5_150;

    /// A loaded pillager standing next to a player it shoots at.
    fn fixture(
        name: &'static str,
        crossbow: ItemStack,
    ) -> (TestWorld, Arc<PillagerEntity>, SharedEntity, SharedEntity) {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world(name);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let target = TestPlayerBuilder::new(Arc::clone(world), "PillagerTarget", TARGET_ID).build();
        target
            .try_set_position(DVec3::new(4.5, 65.0, 0.5))
            .expect("the target starts inside the loaded chunk");
        let target_body: SharedEntity = Arc::<Player>::clone(&target);
        world
            .try_add_entity(Arc::clone(&target_body))
            .expect("the target joins the world");

        let pillager = Arc::new(PillagerEntity::new(
            &vanilla_entities::PILLAGER,
            PILLAGER_ID,
            DVec3::new(0.5, 65.0, 0.5),
            Arc::downgrade(world),
        ));
        pillager.set_item_in_hand(InteractionHand::MainHand, crossbow);
        let body: SharedEntity = Arc::<PillagerEntity>::clone(&pillager);
        world
            .try_add_entity(Arc::clone(&body))
            .expect("the pillager joins the world");
        (world_fixture, pillager, body, target_body)
    }

    /// The state a completed charge leaves in the pillager's hand.
    fn charged_crossbow() -> ItemStack {
        let mut crossbow = ItemStack::new(&vanilla_items::CROSSBOW);
        let arrow = ItemStackTemplate::from_stack(&ItemStack::new(&vanilla_items::ARROW))
            .expect("an arrow is a valid projectile template");
        crossbow.set(
            CHARGED_PROJECTILES,
            ChargedProjectiles::new(vec![arrow]).expect("one projectile fits"),
        );
        crossbow
    }

    fn arrows(world_fixture: &TestWorld) -> Vec<SharedEntity> {
        world_fixture.world.get_entities_in_aabb_matching(
            &WorldAabb::new(-16.0, 55.0, -16.0, 16.0, 75.0, 16.0),
            |entity| entity.entity_type() == &vanilla_entities::ARROW,
        )
    }

    #[test]
    fn pillager_is_a_ranged_attack_mob() {
        init_vanilla_registry();
        let (_fixture, _pillager, body, _target) =
            fixture("pillager_ranged_attack_capability", charged_crossbow());
        assert!(
            body.as_ranged_attack_mob().is_some(),
            "the raid goal's `ReadyToAttack` has nothing to release without this capability"
        );
    }

    /// Vanilla `Pillager.performRangedAttack` is `performCrossbowAttack(this,
    /// 1.6F)`: the pillager discards the `1.0F` the goal passes and its bolt
    /// flies at `CrossbowItem.MOB_ARROW_POWER`.
    #[test]
    fn pillager_fires_its_charged_crossbow_at_mob_arrow_power() {
        init_vanilla_registry();
        let (world_fixture, pillager, _body, target) =
            fixture("pillager_ranged_attack_fire", charged_crossbow());
        pillager.set_no_action_time(42);

        assert!(RangedAttackMob::perform_ranged_attack(
            pillager.as_ref(),
            &target,
            1.0
        ));

        let fired = pillager.get_item_in_hand(InteractionHand::MainHand);
        assert!(
            !is_charged(&fired),
            "the shot clears the loaded projectiles"
        );
        assert_eq!(
            fired.get_damage_value(),
            1,
            "firing an arrow costs one durability"
        );
        assert_eq!(
            pillager.no_action_time(),
            0,
            "`onCrossbowAttackPerformed` clears the no-action time"
        );

        let arrows = arrows(&world_fixture);
        assert_eq!(arrows.len(), 1);
        let velocity = arrows[0].velocity();
        assert!(
            (velocity.length() - f64::from(MOB_ARROW_POWER)).abs() < 0.2,
            "the bolt flies at `MOB_ARROW_POWER` (1.6): {velocity:?}"
        );
    }

    #[test]
    fn uncharged_pillager_releases_nothing() {
        init_vanilla_registry();
        let (world_fixture, pillager, _body, target) = fixture(
            "pillager_ranged_attack_uncharged",
            ItemStack::new(&vanilla_items::CROSSBOW),
        );

        assert!(
            !RangedAttackMob::perform_ranged_attack(pillager.as_ref(), &target, 1.0),
            "an unloaded crossbow releases no bolt"
        );
        assert!(arrows(&world_fixture).is_empty());
        assert_eq!(
            pillager.no_action_time(),
            0,
            "vanilla still performs `onCrossbowAttackPerformed`"
        );
    }
}
