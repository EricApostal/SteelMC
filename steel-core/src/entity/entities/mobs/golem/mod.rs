//! Vanilla `IronGolem` (`world/entity/animal/golem/IronGolem.java`).
//!
//! Parent `AbstractGolem` returns no ambient/hurt/death sound of its own and never despawns; the
//! iron golem adds crackiness, the poppy offer, the player-created flag and persistent anger.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::{EntityDimensions, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::IronGolemEntityData;
use steel_registry::{sound_events, vanilla_attributes, vanilla_entities, vanilla_items};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, Downcast as _, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    AngryAtPlayerTargetGoal, DefendVillageTargetGoal, GolemRandomStrollInVillageGoal,
    HurtByTargetGoal, LookAtPlayerGoal, MeleeAttackGoal, MoveBackToVillageGoal,
    MoveTowardsTargetGoal, NearestAttackableTargetGoal, OfferFlowerGoal, RandomLookAroundGoal,
    ResetUniversalAngerTargetGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::neutral_mob::{NeutralMob, PersistentAnger};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityEventSource, EntityPose, EntitySpawnReason,
    EntitySyncedData, LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity,
    SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

/// Vanilla `IronGolem.IRON_INGOT_HEAL_AMOUNT`.
const IRON_INGOT_HEAL_AMOUNT: f32 = 25.0;
/// Vanilla `IronGolem.PERSISTENT_ANGER_TIME = TimeUtil.rangeOfSeconds(20, 39)`.
const PERSISTENT_ANGER_TIME_MIN: i64 = 20 * 20;
const PERSISTENT_ANGER_TIME_MAX: i64 = 39 * 20;
const ATTACK_ANIMATION_TICKS: i32 = 10;
const OFFER_FLOWER_TICKS: i32 = 400;
const FLAG_PLAYER_CREATED: i8 = 1;

#[entity_behavior(class = "IronGolem")]
/// Vanilla iron golem: a neutral mob that defends villagers and offers poppies.
pub struct IronGolemEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    entity_data: SyncMutex<IronGolemEntityData>,
    persistent_anger: PersistentAnger,
    attack_animation_tick: SyncMutex<i32>,
    offer_flower_tick: SyncMutex<i32>,
    last_crackiness: SyncMutex<Option<CrackinessLevel>>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `IronGolemEntity`.
unsafe impl DowncastType for IronGolemEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/iron_golem");
}

impl IronGolemEntity {
    /// Creates a new iron golem at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs an iron golem from persisted base entity state.
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
        let mut entity_data = IronGolemEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);

        {
            // Vanilla `IronGolem.registerGoals` order and priorities.
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(1, MeleeAttackGoal::new(1.0, true));
            goal_selector.add_goal(2, MoveTowardsTargetGoal::new(0.9, 32.0));
            goal_selector.add_goal(2, MoveBackToVillageGoal::new(0.6, false));
            goal_selector.add_goal(4, GolemRandomStrollInVillageGoal::new(0.6));
            goal_selector.add_goal(5, OfferFlowerGoal::new());
            goal_selector.add_goal(7, LookAtPlayerGoal::new(6.0));
            goal_selector.add_goal(8, RandomLookAroundGoal::new());
        }

        {
            let mut target_selector = mob_base.target_selector().lock();
            target_selector.add_goal(1, DefendVillageTargetGoal::new(villager_player_reputation));
            target_selector.add_goal(2, HurtByTargetGoal::new());
            // Vanilla `NearestAttackableTargetGoal<>(this, Player.class, 10, true, false, this::isAngryAt)`.
            target_selector.add_goal(3, AngryAtPlayerTargetGoal::new());
            target_selector.add_goal(
                3,
                NearestAttackableTargetGoal::new_with_interval(
                    5,
                    false,
                    false,
                    |target, _world| {
                        // Vanilla: `target instanceof Enemy && !(target instanceof Creeper)`.
                        target.as_enemy().is_some()
                            && target.entity_type() != &vanilla_entities::CREEPER
                    },
                ),
            );
            target_selector.add_goal(4, ResetUniversalAngerTargetGoal::new(false));
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            entity_data: SyncMutex::new(entity_data),
            persistent_anger: PersistentAnger::new(),
            attack_animation_tick: SyncMutex::new(0),
            offer_flower_tick: SyncMutex::new(0),
            last_crackiness: SyncMutex::new(None),
        }
    }

    /// Vanilla `IronGolem.isPlayerCreated`.
    #[must_use]
    pub fn is_player_created(&self) -> bool {
        self.entity_data.lock().flags.get() & FLAG_PLAYER_CREATED != 0
    }

    /// Vanilla `IronGolem.setPlayerCreated`.
    pub fn set_player_created(&self, value: bool) {
        let new_flags = {
            let entity_data = self.entity_data.lock();
            let current = entity_data.flags.get();
            if value {
                current | FLAG_PLAYER_CREATED
            } else {
                current & !FLAG_PLAYER_CREATED
            }
        };
        self.entity_data.lock().flags.set(new_flags);
    }

    /// Vanilla `IronGolem.getAttackAnimationTick`.
    #[must_use]
    pub fn attack_animation_tick(&self) -> i32 {
        *self.attack_animation_tick.lock()
    }

    /// Vanilla `IronGolem.getOfferFlowerTick`.
    #[must_use]
    pub fn offer_flower_tick(&self) -> i32 {
        *self.offer_flower_tick.lock()
    }

    /// Vanilla `IronGolem.offerFlower`.
    pub fn offer_flower(&self, offer: bool) {
        if offer {
            *self.offer_flower_tick.lock() = OFFER_FLOWER_TICKS;
            self.broadcast_entity_event(EntityStatus::OfferFlower);
        } else {
            *self.offer_flower_tick.lock() = 0;
            self.broadcast_entity_event(EntityStatus::StopOfferFlower);
        }
    }

    /// Vanilla `IronGolem.getCrackiness` over `Crackiness.GOLEM` (0.75 / 0.5 / 0.25).
    #[must_use]
    pub fn crackiness(&self) -> CrackinessLevel {
        let fraction = self.get_health() / self.get_max_health();
        if fraction < 0.25 {
            CrackinessLevel::High
        } else if fraction < 0.5 {
            CrackinessLevel::Medium
        } else if fraction < 0.75 {
            CrackinessLevel::Low
        } else {
            CrackinessLevel::None
        }
    }

    /// Vanilla `IronGolem.canAttack`: player-created golems spare players, nobody hits creepers.
    #[must_use]
    fn allows_attacking(&self, target_is_player: bool, target_is_creeper: bool) -> bool {
        if target_is_player && self.is_player_created() {
            return false;
        }

        !target_is_creeper
    }

    /// Vanilla `IronGolem.doHurtTarget` damage roll: `base/2 + random.nextInt(base)`.
    fn roll_attack_damage(attack_damage: f32) -> f32 {
        let base = attack_damage as i32;
        if base > 0 {
            attack_damage / 2.0 + rand::random_range(0..base) as f32
        } else {
            attack_damage
        }
    }

    /// Vanilla plays `IRON_GOLEM_DAMAGE` from `hurtServer` when the crackiness level changed.
    ///
    /// Steel's `LivingEntity::hurt_server` default cannot be wrapped from an override, so the
    /// transition is detected on the next AI tick instead.
    fn tick_crackiness_sound(&self) {
        let current = self.crackiness();
        let mut last = self.last_crackiness.lock();
        if last.is_some_and(|previous| current.severity() > previous.severity()) {
            drop(last);
            self.play_sound(&sound_events::ENTITY_IRON_GOLEM_DAMAGE, 1.0, 1.0);
            *self.last_crackiness.lock() = Some(current);
            return;
        }

        *last = Some(current);
    }
}

/// Vanilla `Crackiness.Level`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrackinessLevel {
    /// Undamaged.
    None,
    /// Below `0.75` health fraction.
    Low,
    /// Below `0.5` health fraction.
    Medium,
    /// Below `0.25` health fraction.
    High,
}

impl CrackinessLevel {
    /// Vanilla enum ordinal; the damage sound plays when the value grows.
    #[must_use]
    const fn severity(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }
}

/// Vanilla `Villager.getPlayerReputation` for `DefendVillageTargetGoal`.
fn villager_player_reputation(entity: &dyn Entity, player: &uuid::Uuid) -> Option<i32> {
    let villager = entity.downcast_ref::<VillagerEntity>()?;
    Some(villager.player_reputation(*player))
}

impl Entity for IronGolemEntity {
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

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn play_step_sound(&self, _pos: BlockPos, _block_state: BlockStateId) {
        self.play_sound(&sound_events::ENTITY_IRON_GOLEM_STEP, 1.0, 1.0);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        nbt.insert("PlayerCreated", self.is_player_created());
        self.add_persistent_anger_save_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.set_player_created(
            nbt.byte("PlayerCreated")
                .is_some_and(|value| value & FLAG_PLAYER_CREATED != 0),
        );
        self.read_persistent_anger_save_data(&nbt);
    }
}

impl LivingEntity for IronGolemEntity {
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

    fn decrease_air_supply(&self, current_supply: i32) -> i32 {
        // Vanilla `IronGolem.decreaseAirSupply`: golems never drown.
        current_supply
    }

    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_IRON_GOLEM_HURT)
    }

    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_IRON_GOLEM_DEATH)
    }

    fn do_push(&self, entity: &SharedEntity) {
        // Vanilla `IronGolem.doPush`: a pushed hostile sometimes becomes the golem's target.
        if entity.as_enemy().is_some()
            && entity.entity_type() != &vanilla_entities::CREEPER
            && rand::random_range(0..20) == 0
        {
            let _ = self.set_target(Some(entity));
        }

        entity.push_entity(EntityEventSource::as_entity_event_source(self));
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        Mob::mob_server_ai_step(self, entity);
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);

        {
            let mut attack_animation = self.attack_animation_tick.lock();
            if *attack_animation > 0 {
                *attack_animation -= 1;
            }
        }
        {
            let mut offer_flower = self.offer_flower_tick.lock();
            if *offer_flower > 0 {
                *offer_flower -= 1;
            }
        }
        // Vanilla `aiStep` runs `updatePersistentAnger(level, true)` on the server.
        self.update_persistent_anger(true);
        self.tick_crackiness_sound();

        result
    }
}

impl Mob for IronGolemEntity {
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

    fn ambient_sound_interval(&self) -> i32 {
        // Vanilla `AbstractGolem.getAmbientSoundInterval`.
        120
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        // Vanilla `AbstractGolem.getAmbientSound` returns null.
        None
    }

    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        // Vanilla `AbstractGolem.removeWhenFarAway` is always false.
        false
    }

    fn can_attack(&self, target: &dyn LivingEntity) -> bool {
        if !self.allows_attacking(
            target.entity_type() == &vanilla_entities::PLAYER,
            target.entity_type() == &vanilla_entities::CREEPER,
        ) {
            return false;
        }

        // Vanilla `Mob.canAttack`: ghasts are never targeted directly.
        target.entity_type() != &vanilla_entities::GHAST && LivingEntity::can_attack(self, target)
    }

    fn do_hurt_target(&self, entity: &SharedEntity, world: &World, target: &SharedEntity) -> bool {
        *self.attack_animation_tick.lock() = ATTACK_ANIMATION_TICKS;
        self.broadcast_entity_event(EntityStatus::StartAttacking);

        let attack_damage = self
            .attributes()
            .lock()
            .required_value(vanilla_attributes::ATTACK_DAMAGE) as f32;
        let damage = Self::roll_attack_damage(attack_damage);
        let source = self.mob_attack_damage_source(&ItemStack::empty(), entity);
        let hurt = target.hurt(world, &source, damage);
        if hurt {
            let knockback_resistance = target
                .as_living_entity()
                .map_or(0.0, LivingEntity::knockback_resistance);
            let scale = (1.0 - knockback_resistance).max(0.0);
            target.set_velocity(target.velocity() + DVec3::new(0.0, 0.4 * scale, 0.0));
        }

        self.play_sound(&sound_events::ENTITY_IRON_GOLEM_ATTACK, 1.0, 1.0);
        hurt
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let is_iron_ingot = {
            let inventory = player.inventory.lock();
            inventory
                .get_item_in_hand(hand)
                .is(&vanilla_items::IRON_INGOT)
        };
        if !is_iron_ingot {
            return InteractionResult::Pass;
        }

        let health_before = self.get_health();
        self.heal(IRON_INGOT_HEAL_AMOUNT);
        // Vanilla compares the health exactly.
        #[expect(clippy::float_cmp, reason = "vanilla compares exactly")]
        if self.get_health() == health_before {
            return InteractionResult::Pass;
        }

        let pitch = 1.0 + (rand::random::<f32>() - rand::random::<f32>()) * 0.2;
        self.play_sound(&sound_events::ENTITY_IRON_GOLEM_REPAIR, 1.0, pitch);
        let has_infinite_materials = player.has_infinite_materials();
        player
            .inventory
            .lock()
            .mutate_item_in_hand(hand, |stack| stack.consume(1, has_infinite_materials));
        InteractionResult::Success
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `CarvedPumpkinBlock.trySpawnGolem` marks the golem it builds as player-created;
        // `EntitySpawnReason::Triggered` is that build path (villager summons use `MobSummoned`).
        if spawn_reason == EntitySpawnReason::Triggered {
            self.set_player_created(true);
        }

        self.finalize_spawn_mob_base(world, spawn_reason, group_data)
    }

    fn check_spawn_obstruction(&self, world: &Arc<World>) -> bool {
        // Vanilla `IronGolem.checkSpawnObstruction`: the block below must be standable and the
        // golem's own column plus two blocks above it must be unobstructed. Steel has no
        // `NaturalSpawner.isValidEmptySpawnBlock`, so the column test uses the solid predicate.
        let pos = self.block_position();
        if !world.get_block_state(pos.below()).is_solid() {
            return false;
        }

        for offset in 1..3 {
            if world.get_block_state(pos.above_n(offset)).is_solid() {
                return false;
            }
        }

        !world.get_block_state(pos).is_solid()
            && !world.has_entity_in_aabb_matching(&self.bounding_box(), |entity| {
                entity.id() != self.id() && entity.blocks_building() && !entity.is_spectator()
            })
    }
}

impl PathfinderMob for IronGolemEntity {}

impl NeutralMob for IronGolemEntity {
    fn persistent_anger(&self) -> &PersistentAnger {
        &self.persistent_anger
    }

    fn start_persistent_anger_timer(&self) {
        self.set_time_to_remain_angry(rand::random_range(
            PERSISTENT_ANGER_TIME_MIN..=PERSISTENT_ANGER_TIME_MAX,
        ));
    }
}

pub mod copper_golem;
pub mod snow_golem;

pub use copper_golem::CopperGolemEntity;
pub use snow_golem::SnowGolemEntity;

#[cfg(test)]
mod tests;
