//! Vanilla `TraderLlama`: the wandering trader's leashed llama.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entity_data::TraderLlamaEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{REGISTRY, TaggedRegistryExt as _, vanilla_entities};
use steel_utils::locks::SyncMutex;
use steel_utils::{Downcast as _, DowncastType, DowncastTypeKey};

use super::LlamaEntity;
use crate::entity::ai::goal::{Goal, GoalControls, NearestAttackableTargetGoal, PanicGoal};
use crate::entity::entities::mobs::npc::wandering_trader::WanderingTraderEntity;
use crate::entity::leash::Leashable;
use crate::entity::{
    AgeableMob, Entity, EntityBase, EntityBaseLoad, EntitySpawnReason, EntitySyncedData,
    LivingEntity, LivingEntityBase, Mob, MobBase, PathfinderMob, SharedEntity, SpawnGroupData,
};
use crate::world::World;

/// Vanilla `TraderLlama.DEFAULT_DESPAWN_DELAY`.
pub(crate) const TRADER_LLAMA_DEFAULT_DESPAWN_DELAY: i32 = 47999;

#[entity_behavior(class = "TraderLlama")]
/// Vanilla trader llama: shares the llama body, adds the trader despawn clock.
pub struct TraderLlamaEntity {
    llama: LlamaEntity,
    entity_data: SyncMutex<TraderLlamaEntityData>,
    /// Vanilla `TraderLlama.despawnDelay`, initialized to 47999.
    despawn_delay: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `TraderLlamaEntity`.
unsafe impl DowncastType for TraderLlamaEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/trader_llama");
}

impl TraderLlamaEntity {
    /// Creates a new trader llama at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a trader llama from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let llama = LlamaEntity::new_with_base(base, entity_type);

        {
            // Vanilla `TraderLlama.registerGoals` additions, in order.
            let mut goal_selector = llama.mob_base().goal_selector().lock();
            goal_selector.add_goal(1, PanicGoal::new(2.0));
        }

        {
            // Vanilla target goals: defend the trader, then zombies (except
            // zombified piglins) and illagers.
            let mut target_selector = llama.mob_base().target_selector().lock();
            target_selector.add_goal(1, DefendWanderingTraderGoal::new());
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new(true, |target, _| {
                    REGISTRY
                        .entity_types
                        .is_in_tag(target.entity_type(), &EntityTypeTag::ZOMBIES)
                        && target.entity_type().key != vanilla_entities::ZOMBIFIED_PIGLIN.key
                }),
            );
            target_selector.add_goal(
                2,
                NearestAttackableTargetGoal::new(true, |target, _| {
                    REGISTRY
                        .entity_types
                        .is_in_tag(target.entity_type(), &EntityTypeTag::ILLAGER)
                }),
            );
        }

        Self {
            llama,
            entity_data: SyncMutex::new(TraderLlamaEntityData::new()),
            despawn_delay: SyncMutex::new(TRADER_LLAMA_DEFAULT_DESPAWN_DELAY),
        }
    }

    /// Vanilla `TraderLlama.setDespawnDelay`.
    pub fn set_despawn_delay(&self, despawn_delay: i32) {
        *self.despawn_delay.lock() = despawn_delay;
    }

    /// Vanilla `TraderLlama.despawnDelay` field.
    #[must_use]
    pub fn despawn_delay(&self) -> i32 {
        *self.despawn_delay.lock()
    }

    /// Vanilla `TraderLlama.isTraderLlama`.
    #[must_use]
    pub const fn is_trader_llama(&self) -> bool {
        true
    }

    fn leash_holder(&self) -> Option<SharedEntity> {
        Leashable::leash_holder(self)
    }

    /// Vanilla `TraderLlama.isLeashedToWanderingTrader`.
    fn is_leashed_to_wandering_trader(&self) -> Option<SharedEntity> {
        self.leash_holder()
            .filter(|holder| holder.downcast_ref::<WanderingTraderEntity>().is_some())
    }

    /// Vanilla `TraderLlama.canDespawn`.
    ///
    /// Vanilla also requires `!isTamed()`; Steel has no taming state yet, so that
    /// term is always true and is omitted rather than faked.
    fn can_despawn(&self) -> bool {
        let leashed_to_other =
            Leashable::is_leashed(self) && self.is_leashed_to_wandering_trader().is_none();

        !leashed_to_other
            && !self.has_exactly_one_player_passenger()
            && !AgeableMob::is_age_locked(self)
            && !Mob::is_persistence_required(self)
    }

    /// Vanilla `TraderLlama.maybeDespawn`: counts down, tracking the trader's clock.
    fn maybe_despawn(&self) {
        if !self.can_despawn() {
            return;
        }

        let next_delay = match self.is_leashed_to_wandering_trader() {
            Some(trader) => trader.downcast_ref::<WanderingTraderEntity>().map_or_else(
                || self.despawn_delay() - 1,
                |trader| trader.despawn_delay() - 1,
            ),
            None => self.despawn_delay() - 1,
        };
        *self.despawn_delay.lock() = next_delay;

        if next_delay <= 0 {
            self.remove_leash();
            self.set_removed(crate::entity::RemovalReason::Discarded);
        }
    }

    /// Vanilla `TraderLlama.doPlayerRide`: refuses to be ridden while the trader
    /// holds the leash. Steel has no riding entry point on this path yet.
    #[must_use]
    pub fn can_player_ride(&self) -> bool {
        self.is_leashed_to_wandering_trader().is_none()
    }
}

impl Entity for TraderLlamaEntity {
    fn base(&self) -> &EntityBase {
        self.llama.base()
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.llama.entity_type()
    }

    fn base_tick(&self) {
        self.llama.base_tick();
    }

    fn dimensions_for_pose(
        &self,
        pose: crate::entity::EntityPose,
    ) -> steel_registry::entity_type::EntityDimensions {
        self.llama.dimensions_for_pose(pose)
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        // The trader llama declares no synced data of its own; the llama layer carries it.
        Some(&self.entity_data)
    }

    fn update_data_before_sync(&self) {
        self.llama.update_data_before_sync();
    }

    fn max_up_step(&self) -> f32 {
        self.llama.max_up_step()
    }

    fn sound_source(&self) -> steel_protocol::packets::game::SoundSource {
        self.llama.sound_source()
    }

    fn play_step_sound(&self, pos: steel_utils::BlockPos, block_state: steel_utils::BlockStateId) {
        self.llama.play_step_sound(pos, block_state);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.llama.save_additional(nbt);
        nbt.insert("DespawnDelay", self.despawn_delay());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.llama.load_additional(nbt);
        self.set_despawn_delay(
            nbt.int("DespawnDelay")
                .unwrap_or(TRADER_LLAMA_DEFAULT_DESPAWN_DELAY),
        );
    }
}

impl LivingEntity for TraderLlamaEntity {
    fn living_base(&self) -> &LivingEntityBase {
        self.llama.living_base()
    }

    fn get_health(&self) -> f32 {
        self.llama.get_health()
    }

    fn set_health(&self, health: f32) {
        self.llama.set_health(health);
    }

    fn sound_volume(&self) -> f32 {
        self.llama.sound_volume()
    }

    fn hurt_sound(
        &self,
        source: &crate::entity::damage::DamageSource,
    ) -> Option<steel_registry::sound_event::SoundEventRef> {
        self.llama.hurt_sound(source)
    }

    fn death_sound(&self) -> Option<steel_registry::sound_event::SoundEventRef> {
        self.llama.death_sound()
    }

    fn server_ai_step(&self, entity: &SharedEntity) {
        self.llama.server_ai_step(entity);
        self.maybe_despawn();
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<crate::physics::MoveResult> {
        self.llama.ai_step(entity)
    }
}

impl AgeableMob for TraderLlamaEntity {
    fn ageable_base(&self) -> &crate::entity::AgeableMobBase {
        self.llama.ageable_base()
    }

    fn is_age_locked(&self) -> bool {
        self.llama.is_age_locked()
    }

    fn set_age_locked(&self, age_locked: bool) {
        self.llama.set_age_locked(age_locked);
    }

    fn set_synced_baby(&self, baby: bool) {
        self.llama.set_synced_baby(baby);
    }

    fn age_boundary_changed(&self, baby: bool) {
        self.llama.age_boundary_changed(baby);
    }
}

impl crate::entity::Animal for TraderLlamaEntity {
    fn animal_base(&self) -> &crate::entity::AnimalBase {
        self.llama.animal_base()
    }

    fn is_food(&self, item_stack: &steel_registry::item_stack::ItemStack) -> bool {
        crate::entity::Animal::is_food(&self.llama, item_stack)
    }
}

impl Mob for TraderLlamaEntity {
    fn mob_base(&self) -> &MobBase {
        self.llama.mob_base()
    }

    fn mob_flags(&self) -> i8 {
        self.llama.mob_flags()
    }

    fn set_mob_flags(&self, flags: i8) {
        self.llama.set_mob_flags(flags);
    }

    fn tick_goal_selectors(&self, entity: &SharedEntity) {
        self.llama.tick_goal_selectors(entity);
    }

    fn tick_path_navigation(&self) {
        self.llama.tick_path_navigation();
    }

    fn custom_server_ai_step(&self) {
        self.llama.custom_server_ai_step();
    }

    fn ambient_sound(&self) -> Option<steel_registry::sound_event::SoundEventRef> {
        self.llama.ambient_sound()
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `TraderLlama.finalizeSpawn`: spawner-spawned llamas are adults.
        if spawn_reason == EntitySpawnReason::Event {
            self.set_age(0);
        }
        Mob::finalize_spawn(&self.llama, world, spawn_reason, group_data)
    }

    fn mob_interact(
        &self,
        player: &crate::player::Player,
        hand: steel_utils::types::InteractionHand,
    ) -> crate::behavior::InteractionResult {
        Mob::mob_interact(&self.llama, player, hand)
    }
}

impl PathfinderMob for TraderLlamaEntity {}

/// Vanilla `TraderLlama.TraderLlamaDefendWanderingTraderGoal`.
///
/// Vanilla keeps the defended hit timestamp on this goal object (and passes
/// `alertOthers = false` to `TargetGoal`), so the state lives here too.
struct DefendWanderingTraderGoal {
    owner_last_hurt_by: Option<SharedEntity>,
    timestamp: i32,
}

impl DefendWanderingTraderGoal {
    const fn new() -> Self {
        Self {
            owner_last_hurt_by: None,
            timestamp: 0,
        }
    }

    /// Vanilla `canUse` owner lookup: leashed to a `WanderingTrader`.
    fn trader_holding_leash(mob: &dyn PathfinderMob) -> Option<SharedEntity> {
        let holder = mob.leash_holder()?;
        holder.downcast_ref::<WanderingTraderEntity>()?;
        Some(holder)
    }
}

impl Goal for DefendWanderingTraderGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::TARGET
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(trader) = Self::trader_holding_leash(mob) else {
            return false;
        };
        let Some(trader) = trader.downcast_ref::<WanderingTraderEntity>() else {
            return false;
        };

        self.owner_last_hurt_by = trader.last_hurt_by_mob();
        let timestamp = trader.last_hurt_by_mob_timestamp();
        self.owner_last_hurt_by.as_ref().is_some_and(|attacker| {
            timestamp != self.timestamp
                && attacker
                    .as_living_entity()
                    .is_some_and(|attacker| Mob::can_attack(mob, attacker))
        })
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        Mob::set_target(mob, self.owner_last_hurt_by.as_ref());
        if let Some(trader) = Self::trader_holding_leash(mob)
            && let Some(trader) = trader.downcast_ref::<WanderingTraderEntity>()
        {
            self.timestamp = trader.last_hurt_by_mob_timestamp();
        }
    }

    fn stop(&mut self, _mob: &dyn PathfinderMob) {
        self.owner_last_hurt_by = None;
    }
}
