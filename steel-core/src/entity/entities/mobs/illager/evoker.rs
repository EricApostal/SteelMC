//! Vanilla `Evoker` (vanilla `net.minecraft.world.entity.monster.illager.Evoker`).
//!
//! The evoker is a `SpellcasterIllager`: it keeps the spell state (casting tick count,
//! current spell) through the [`Spellcaster`] capability and casts through
//! [`SpellcasterUseSpellGoal`]. Its three spells are vanilla's inner goal classes:
//! [`EvokerAttackSpell`] (fangs), [`EvokerSummonSpell`] (three vexes) and
//! [`EvokerWololoSpell`] (blue sheep to red).
//!
//! Deliberate differences:
//! - vanilla renders the spell-color hand particles in `tick` on the client; Steel has no
//!   client, so only the server half runs.
//! - the summoned vex is not added to the evoker's scoreboard team: Steel's scoreboard teams
//!   are owned by the scoreboard lane and the evoker has no team accessor yet.

use std::iter;
use std::sync::{Arc, Weak};

use glam::DVec3;
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::Direction;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::EvokerEntityData;
use steel_registry::{
    DyeColor, sound_events, vanilla_attributes, vanilla_entities, vanilla_game_events,
    vanilla_game_rules,
};
use steel_utils::axis::Axis;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, Downcast as _, DowncastType, DowncastTypeKey};

use super::abstract_illager::{IllagerArmPose, IllagerSpell, RaiderKeys};
use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, HurtByTargetGoal, LookAtPlayerGoal, NearestAttackableTargetGoal,
    RandomStrollGoal, SpellcasterCastingSpellGoal, SpellcasterSpell, SpellcasterUseSpellGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::hostile::vex::VexEntity;
use crate::entity::entities::mobs::passive::SheepEntity;
use crate::entity::entities::objects::projectiles::EvokerFangsEntity;
use crate::entity::raid::RaidSnapshot;
use crate::entity::raid::raider::{Raider, RaiderBase};
use crate::entity::spawn::{add_spawned_entity, create_entity_instance};
use crate::entity::{
    ENTITIES, Entity, EntityBase, EntityBaseLoad, EntityPose, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, Monster, PathfinderMob, SharedEntity,
    next_entity_id,
};
use crate::physics::MoveResult;
use crate::world::LevelReader;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const DEFAULT_STEP_HEIGHT: f32 = 0.6;
/// Vanilla `Evoker.xpReward`.
const XP_REWARD: i32 = 10;

/// Vanilla `Evoker.EvokerAttackSpellGoal.getCastingTime`.
const ATTACK_CASTING_TIME: i32 = 40;
/// Vanilla `Evoker.EvokerAttackSpellGoal.getCastingInterval`.
const ATTACK_CASTING_INTERVAL: i32 = 100;
/// Vanilla `Evoker.EvokerSummonSpellGoal.getCastingTime`.
const SUMMON_CASTING_TIME: i32 = 100;
/// Vanilla `Evoker.EvokerSummonSpellGoal.getCastingInterval`.
const SUMMON_CASTING_INTERVAL: i32 = 340;
/// Vanilla `Evoker.EvokerWololoSpellGoal.getCastingTime`.
const WOLOLO_CASTING_TIME: i32 = 60;
/// Vanilla `Evoker.EvokerWololoSpellGoal.getCastingInterval`.
const WOLOLO_CASTING_INTERVAL: i32 = 140;
/// Vanilla `Evoker.EvokerWololoSpellGoal.getCastWarmupTime`.
const WOLOLO_CAST_WARMUP_TIME: i32 = 40;

/// Vanilla `Evoker.EvokerSummonSpellGoal`: three vexes per cast.
const VEX_SUMMON_COUNT: i32 = 3;
/// Vanilla `EvokerSummonSpellGoal.vexCountTargeting` range.
const VEX_COUNT_RADIUS: f64 = 16.0;
/// Vanilla `EvokerSummonSpellGoal.canUse`: `random.nextInt(8) + 1 > vexes`.
const VEX_LIMIT_RANDOM_BOUND: i32 = 8;
/// Vanilla `EvokerSummonSpellGoal`: `20 * (30 + random.nextInt(90))`.
const VEX_LIFE_BASE_TICKS: i32 = 30;
const VEX_LIFE_RANDOM_BOUND: i32 = 90;

/// Vanilla `Evoker.EvokerWololoSpellGoal.wololoTargeting` range.
const WOLOLO_RADIUS: f64 = 16.0;
/// Vanilla `EvokerWololoSpellGoal` vertical inflation around the evoker.
const WOLOLO_VERTICAL_RADIUS: f64 = 4.0;

/// Vanilla `Evoker.EvokerAttackSpellGoal.performSpellCasting`: `distanceToSqr(target) < 9.0`.
const FANGS_ARC_DISTANCE_SQR: f64 = 9.0;

/// Vanilla evoker entity.
#[entity_behavior(class = "Evoker")]
pub struct EvokerEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    raider_base: RaiderBase,
    entity_data: SyncMutex<EvokerEntityData>,
    /// Vanilla `SpellcasterIllager.spellCastingTickCount`.
    spell_casting_tick_count: SyncMutex<i32>,
    /// Vanilla `Evoker.wololoTarget`.
    wololo_target: SyncMutex<Option<SharedEntity>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `EvokerEntity`.
unsafe impl DowncastType for EvokerEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/evoker");
}

impl EvokerEntity {
    /// Creates a new evoker entity instance.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Loads a saved evoker entity.
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
        let mut entity_data = EvokerEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            let mut goals = mob_base.goal_selector().lock();
            goals.add_goal(0, FloatGoal::new(&mob_base));
            goals.add_goal(1, SpellcasterCastingSpellGoal);
            goals.add_goal(
                2,
                AvoidEntityGoal::with_selector(8.0, 0.6, 1.0, |target, _| {
                    target.entity_type() == &vanilla_entities::PLAYER
                }),
            );
            // Vanilla goal 3 avoids `Creaking`; Steel has no creaking factory yet.
            if ENTITIES.has_factory(&vanilla_entities::CREAKING) {
                goals.add_goal(
                    3,
                    AvoidEntityGoal::with_selector(8.0, 0.6, 1.0, |target, _| {
                        target.entity_type() == &vanilla_entities::CREAKING
                    }),
                );
            }
            goals.add_goal(4, SpellcasterUseSpellGoal::new(EvokerSummonSpell));
            goals.add_goal(5, SpellcasterUseSpellGoal::new(EvokerAttackSpell));
            goals.add_goal(
                6,
                SpellcasterUseSpellGoal::new(EvokerWololoSpell::default()),
            );
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
                NearestAttackableTargetGoal::new_for_players(true, |_, _| true)
                    .with_unseen_memory_ticks(300),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    false,
                    super::abstract_illager::can_attack_villager,
                )
                .with_unseen_memory_ticks(300),
            );
            targets.add_goal(
                3,
                NearestAttackableTargetGoal::new(
                    false,
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
            spell_casting_tick_count: SyncMutex::new(0),
            wololo_target: SyncMutex::new(None),
        }
    }

    /// Vanilla `SpellcasterIllager.isCastingSpell` (server side).
    #[must_use]
    pub fn is_casting_spell(&self) -> bool {
        self.spell_casting_tick_count() > 0
    }

    /// Vanilla `SpellcasterIllager.spellCastingTickCount`.
    #[must_use]
    pub fn spell_casting_tick_count(&self) -> i32 {
        *self.spell_casting_tick_count.lock()
    }

    /// Vanilla `SpellcasterIllager.spellCastingTickCount = ticks`.
    pub fn set_spell_casting_tick_count(&self, ticks: i32) {
        *self.spell_casting_tick_count.lock() = ticks;
    }

    /// Vanilla `SpellcasterIllager.setIsCastingSpell`.
    pub fn set_is_casting_spell(&self, spell: IllagerSpell) {
        self.entity_data
            .lock()
            .spellcaster_illager_mut()
            .spell_casting
            .set(spell.id() as i8);
    }

    /// Vanilla `SpellcasterIllager.getCastingSoundEvent`.
    #[must_use]
    pub fn casting_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_EVOKER_CAST_SPELL
    }

    /// Vanilla `Evoker.setWololoTarget`.
    pub fn set_wololo_target(&self, target: Option<SharedEntity>) {
        *self.wololo_target.lock() = target;
    }

    /// Vanilla `Evoker.getWololoTarget`.
    #[must_use]
    pub fn wololo_target(&self) -> Option<SharedEntity> {
        self.wololo_target.lock().clone()
    }

    /// Vanilla `Evoker.getArmPose` (via `SpellcasterIllager.getArmPose`).
    #[must_use]
    pub fn arm_pose(&self) -> IllagerArmPose {
        if self.is_casting_spell() {
            IllagerArmPose::Spellcasting
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

impl Entity for EvokerEntity {
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

    fn save_additional(&self, nbt: &mut simdnbt::owned::NbtCompound) {
        self.save_mob(nbt);
        self.raider_base.save(nbt);
        // Vanilla `SpellcasterIllager.addAdditionalSaveData`.
        nbt.insert("SpellTicks", self.spell_casting_tick_count());
    }

    fn load_additional(&self, nbt: simdnbt::borrow::NbtCompound<'_, '_>) {
        self.load_mob(nbt);
        if let Some(raid_id) = self.raider_base.load(&nbt) {
            self.rejoin_saved_raid(raid_id);
        }
        // Vanilla `SpellcasterIllager.readAdditionalSaveData`.
        self.set_spell_casting_tick_count(nbt.int("SpellTicks").unwrap_or(0));
        self.set_can_pick_up_loot(true);
    }
}

impl LivingEntity for EvokerEntity {
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
        Some(&sound_events::ENTITY_EVOKER_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_EVOKER_DEATH)
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
        // Vanilla `SpellcasterIllager.customServerAiStep`.
        if self.spell_casting_tick_count() > 0 {
            let remaining = self.spell_casting_tick_count() - 1;
            self.set_spell_casting_tick_count(remaining);
        }
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        Raider::raider_ai_step(self, entity);
        Monster::monster_ai_step(self, entity)
    }

    fn on_death(&self, source: &DamageSource) {
        Raider::raider_on_death(self, source);
    }
}

impl Mob for EvokerEntity {
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
        Some(&sound_events::ENTITY_EVOKER_AMBIENT)
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

impl Monster for EvokerEntity {}

impl PathfinderMob for EvokerEntity {}

impl Raider for EvokerEntity {
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

    /// Vanilla `Evoker.applyRaidBuffs` is empty: evokers never receive raid equipment.
    fn apply_raid_buffs(
        &self,
        _world: &Arc<World>,
        _raid: &RaidSnapshot,
        _wave: i32,
        _is_captain: bool,
    ) {
    }

    fn celebrate_sound(&self) -> SoundEventRef {
        &sound_events::ENTITY_EVOKER_CELEBRATE
    }
}

/// Vanilla `Evoker.EvokerAttackSpellGoal`: a line of fangs towards the target, or a ring of
/// fangs around the evoker when the target is closer than three blocks.
struct EvokerAttackSpell;

impl SpellcasterSpell for EvokerAttackSpell {
    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Fangs
    }

    fn casting_time(&self) -> i32 {
        ATTACK_CASTING_TIME
    }

    fn casting_interval(&self) -> i32 {
        ATTACK_CASTING_INTERVAL
    }

    fn spell_prepare_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_EVOKER_PREPARE_ATTACK)
    }

    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob) {
        let Some(world) = mob.level() else {
            return;
        };
        let Some(target) = mob.target() else {
            return;
        };

        let origin = mob.position();
        let target_pos = target.position();
        let min_y = target_pos.y.min(origin.y);
        let max_y = target_pos.y.max(origin.y) + 1.0;
        let angle = (target_pos.z - origin.z).atan2(target_pos.x - origin.x) as f32;
        let owner = world.get_entity_by_uuid(&mob.uuid());

        if mob.distance_to_sqr(target_pos) < FANGS_ARC_DISTANCE_SQR {
            for i in 0..5 {
                let fang_angle = angle + i as f32 * f32::consts::PI * 0.4;
                create_fang(
                    &world,
                    origin.x + f64::from(fang_angle.cos()) * 1.5,
                    origin.z + f64::from(fang_angle.sin()) * 1.5,
                    min_y,
                    max_y,
                    fang_angle,
                    0,
                    owner.as_ref(),
                );
            }

            for i in 0..8 {
                let fang_angle =
                    angle + i as f32 * f32::consts::PI * 2.0 / 8.0 + (f32::consts::PI * 2.0 / 5.0);
                create_fang(
                    &world,
                    origin.x + f64::from(fang_angle.cos()) * 2.5,
                    origin.z + f64::from(fang_angle.sin()) * 2.5,
                    min_y,
                    max_y,
                    fang_angle,
                    3,
                    owner.as_ref(),
                );
            }
        } else {
            for i in 0..16 {
                let reach = 1.25 * f64::from(i + 1);
                create_fang(
                    &world,
                    origin.x + f64::from(angle.cos()) * reach,
                    origin.z + f64::from(angle.sin()) * reach,
                    min_y,
                    max_y,
                    angle,
                    i,
                    owner.as_ref(),
                );
            }
        }
    }
}

/// Vanilla `Evoker.EvokerAttackSpellGoal.createSpellEntity`: places a fang on the first sturdy
/// floor between the maximum and minimum height, and only when one was found.
fn create_fang(
    world: &Arc<World>,
    x: f64,
    z: f64,
    min_y: f64,
    max_y: f64,
    angle: f32,
    delay_ticks: i32,
    owner: Option<&SharedEntity>,
) {
    let mut pos = BlockPos::new(x.floor() as i32, max_y.floor() as i32, z.floor() as i32);
    let mut success = false;
    let mut top_offset = 0.0;

    loop {
        let below = pos.below();
        let below_state = world.get_block_state(below);
        if world.is_face_sturdy(below_state, below, Direction::Up) {
            let state = world.get_block_state(pos);
            if !state.is_air() {
                let shape = state.get_static_collision_shape();
                if !shape.is_empty() {
                    top_offset = shape.max(Axis::Y);
                }
            }
            success = true;
            break;
        }

        pos = below;
        if pos.y() < min_y.floor() as i32 - 1 {
            break;
        }
    }

    if !success {
        return;
    }

    let y = f64::from(pos.y()) + top_offset;
    let fang = EvokerFangsEntity::new_spell(
        &vanilla_entities::EVOKER_FANGS,
        next_entity_id(),
        DVec3::new(x, y, z),
        angle,
        delay_ticks,
        owner,
        Arc::downgrade(world),
    );
    let _ = add_spawned_entity(world, Arc::new(fang));
    world.game_event(
        &vanilla_game_events::ENTITY_PLACE,
        BlockPos::new(x.floor() as i32, y.floor() as i32, z.floor() as i32),
        &GameEventContext::new(owner.map(|owner| owner.as_ref() as &dyn Entity), None),
    );
}

/// Vanilla `Evoker.EvokerSummonSpellGoal`: summons three vexes while fewer than
/// `random(8) + 1` vexes are nearby.
struct EvokerSummonSpell;

impl EvokerSummonSpell {
    /// Vanilla `EvokerSummonSpellGoal.vexCountTargeting` count.
    fn nearby_vex_count(world: &Arc<World>, mob: &dyn PathfinderMob) -> i32 {
        let box_ = mob.bounding_box().inflate(VEX_COUNT_RADIUS);
        i32::try_from(
            world
                .get_entities_in_aabb_matching(&box_, |entity| {
                    entity.entity_type() == &vanilla_entities::VEX
                })
                .len(),
        )
        .unwrap_or(i32::MAX)
    }
}

impl SpellcasterSpell for EvokerSummonSpell {
    fn spell(&self) -> IllagerSpell {
        IllagerSpell::SummonVex
    }

    fn casting_time(&self) -> i32 {
        SUMMON_CASTING_TIME
    }

    fn casting_interval(&self) -> i32 {
        SUMMON_CASTING_INTERVAL
    }

    fn spell_prepare_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_EVOKER_PREPARE_SUMMON)
    }

    fn can_use_extra(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        let vexes = Self::nearby_vex_count(&world, mob);
        rand::random_range(0..VEX_LIMIT_RANDOM_BOUND) + 1 > vexes
    }

    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob) {
        let Some(world) = mob.level() else {
            return;
        };
        let origin = mob.block_position();
        let owner = world.get_entity_by_uuid(&mob.uuid());

        for _ in 0..VEX_SUMMON_COUNT {
            let pos = origin.offset(
                -2 + rand::random_range(0..5),
                1,
                -2 + rand::random_range(0..5),
            );
            let Ok(vex) = create_entity_instance(
                &world,
                &vanilla_entities::VEX,
                DVec3::new(
                    f64::from(pos.x()) + 0.5,
                    f64::from(pos.y()),
                    f64::from(pos.z()) + 0.5,
                ),
            ) else {
                continue;
            };

            let Some(vex_entity) = vex.downcast_ref::<VexEntity>() else {
                continue;
            };
            vex_entity.set_owner(owner.as_ref());
            vex_entity.set_bound_origin(Some(pos));
            vex_entity.set_limited_life(
                20 * (VEX_LIFE_BASE_TICKS + rand::random_range(0..VEX_LIFE_RANDOM_BOUND)),
            );

            if let Some(vex_mob) = vex.as_mob() {
                vex_mob.finalize_spawn(&world, EntitySpawnReason::MobSummoned, None);
            }
            let _ = add_spawned_entity(&world, vex);
            world.game_event(
                &vanilla_game_events::ENTITY_PLACE,
                pos,
                &GameEventContext::new(owner.as_deref(), None),
            );
        }
    }
}

/// Vanilla `Evoker.EvokerWololoSpellGoal`: turns one nearby blue sheep red.
#[derive(Default)]
struct EvokerWololoSpell;

impl EvokerWololoSpell {
    /// Vanilla `EvokerWololoSpellGoal.canUse` target search: blue sheep within the inflated
    /// 16x4x16 box.
    fn find_target(world: &Arc<World>, mob: &dyn PathfinderMob) -> Option<SharedEntity> {
        let box_ =
            mob.bounding_box()
                .inflate_xyz(WOLOLO_RADIUS, WOLOLO_VERTICAL_RADIUS, WOLOLO_RADIUS);
        let sheep = world.get_entities_in_aabb_matching(&box_, |entity| {
            entity.entity_type() == &vanilla_entities::SHEEP
        });
        if sheep.is_empty() {
            return None;
        }

        let blue: Vec<SharedEntity> = sheep
            .into_iter()
            .filter(|entity| {
                entity
                    .downcast_ref::<SheepEntity>()
                    .is_some_and(|sheep| sheep.color() == DyeColor::Blue)
            })
            .collect();
        if blue.is_empty() {
            return None;
        }
        Some(blue[rand::random_range(0..blue.len())].clone())
    }
}

impl SpellcasterSpell for EvokerWololoSpell {
    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Wololo
    }

    fn casting_time(&self) -> i32 {
        WOLOLO_CASTING_TIME
    }

    fn casting_interval(&self) -> i32 {
        WOLOLO_CASTING_INTERVAL
    }

    fn cast_warmup_time(&self) -> i32 {
        WOLOLO_CAST_WARMUP_TIME
    }

    fn spell_prepare_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_EVOKER_PREPARE_WOLOLO)
    }

    /// Vanilla `EvokerWololoSpellGoal.canUse` replaces the base check entirely: it needs *no*
    /// combat target, the `MOB_GRIEFING` game rule and a blue sheep nearby.
    fn requires_target(&self) -> bool {
        false
    }

    fn can_continue_without_target(&self, mob: &dyn PathfinderMob) -> bool {
        mob.downcast_ref::<EvokerEntity>()
            .and_then(EvokerEntity::wololo_target)
            .and_then(|target| target.as_living_entity().map(LivingEntity::is_alive))
            .is_some_and(|alive| alive)
    }

    fn can_use_extra(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(world) = mob.level() else {
            return false;
        };
        if mob.target().is_some() {
            return false;
        }
        if !world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING) {
            return false;
        }
        let Some(target) = Self::find_target(&world, mob) else {
            return false;
        };
        let Some(evoker) = mob.downcast_ref::<EvokerEntity>() else {
            return false;
        };
        evoker.set_wololo_target(Some(target));
        true
    }

    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob) {
        let Some(evoker) = mob.downcast_ref::<EvokerEntity>() else {
            return;
        };
        let Some(target) = evoker.wololo_target() else {
            return;
        };
        if !target
            .as_living_entity()
            .is_some_and(LivingEntity::is_alive)
        {
            return;
        }
        if let Some(sheep) = target.downcast_ref::<SheepEntity>() {
            sheep.set_color(DyeColor::Red);
        }
    }

    fn on_stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(evoker) = mob.downcast_ref::<EvokerEntity>() {
            evoker.set_wololo_target(None);
        }
    }
}
