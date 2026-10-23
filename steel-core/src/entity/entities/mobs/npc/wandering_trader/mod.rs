//! Vanilla `WanderingTrader`: the data-driven merchant that spawns every so often
//! near players, wanders towards its spawn point and despawns after two in-game days.

mod goals;
mod merchant;
mod spawner;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components;
use steel_registry::entity_type::{EntityDimensions, EntityType, EntityTypeRef};
use steel_registry::item_stack::ItemStack;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_custom_stats;
use steel_registry::vanilla_entity_data::WanderingTraderEntityData;
use steel_registry::{
    RegistryReference, sound_events, vanilla_entities, vanilla_items, vanilla_potions,
};
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, DowncastType, DowncastTypeKey};

use self::goals::{
    LOOK_AT_PROBABILITY, LookAtTradingPlayerGoal, TradeWithPlayerGoal, WanderToPositionGoal,
    is_mob_look_target,
};
use self::merchant::TraderState;
use crate::behavior::InteractionResult;
use crate::entity::ai::goal::{
    AvoidEntityGoal, FloatGoal, InteractGoal, LookAtPlayerGoal, MoveTowardsRestrictionGoal,
    PanicGoal, UseItemGoal, WaterAvoidingRandomStrollGoal,
};
use crate::entity::damage::DamageSource;
use crate::entity::npc::Npc;
use crate::entity::npc::trading::{Merchant, MerchantOffers};
use crate::entity::{
    AgeableMob, AgeableMobBase, AgeableMobGroupData, Entity, EntityBase, EntityBaseLoad,
    EntityEventSource, EntityPose, EntitySpawnReason, EntitySyncedData, LivingEntity,
    LivingEntityBase, LivingEntitySyncedData, Mob, MobBase, PathfinderMob, RemovalReason,
    SharedEntity, SpawnGroupData,
};
use crate::physics::MoveResult;
use crate::player::Player;
use crate::world::World;

pub use spawner::{WanderingTraderData, WanderingTraderSpawner};

/// Vanilla `AgeableMob` baby scale.
const BABY_AGE_SCALE: f32 = 0.5;
/// Vanilla wandering trader interaction level (`openTradingScreen(..., 1)`).
const TRADER_LEVEL: i32 = 1;
/// Vanilla `AbstractVillager.stillValid` interaction range.
pub(super) const TRADING_INTERACTION_RANGE: f64 = 4.0;

#[entity_behavior(class = "WanderingTrader")]
/// Vanilla wandering trader with its trade-set offers, despawn timer and wander target.
pub struct WanderingTraderEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    mob_base: MobBase,
    ageable_base: AgeableMobBase,
    entity_data: SyncMutex<WanderingTraderEntityData>,
    state: Arc<TraderState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `WanderingTraderEntity`.
unsafe impl DowncastType for WanderingTraderEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/wandering_trader");
}

impl WanderingTraderEntity {
    /// Creates a new wandering trader at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a wandering trader from persisted base entity state.
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
        let ageable_base = AgeableMobBase::new();
        let mut entity_data = WanderingTraderEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        let state = Arc::new(TraderState::new());

        {
            // Keep vanilla `WanderingTrader.registerGoals` priorities and speeds in order.
            //
            let mut goal_selector = mob_base.goal_selector().lock();
            goal_selector.add_goal(0, FloatGoal::new(&mob_base));
            goal_selector.add_goal(
                0,
                UseItemGoal::new(
                    invisibility_potion(),
                    Some(&sound_events::ENTITY_WANDERING_TRADER_DISAPPEARED),
                    |mob| {
                        mob.level().is_some_and(|level| level.is_dark_outside())
                            && !mob.is_invisible()
                    },
                ),
            );
            goal_selector.add_goal(
                0,
                UseItemGoal::new(
                    ItemStack::new(&vanilla_items::MILK_BUCKET),
                    Some(&sound_events::ENTITY_WANDERING_TRADER_REAPPEARED),
                    |mob| {
                        mob.level().is_some_and(|level| level.is_bright_outside())
                            && mob.is_invisible()
                    },
                ),
            );
            goal_selector.add_goal(1, TradeWithPlayerGoal::new(Arc::clone(&state)));
            goal_selector.add_goal(1, avoid_goal(8.0, 0.5, 0.5, is_zombie));
            goal_selector.add_goal(
                1,
                avoid_goal(12.0, 0.5, 0.5, is_type(&vanilla_entities::EVOKER)),
            );
            goal_selector.add_goal(
                1,
                avoid_goal(8.0, 0.5, 0.5, is_type(&vanilla_entities::VINDICATOR)),
            );
            goal_selector.add_goal(
                1,
                avoid_goal(8.0, 0.5, 0.5, is_type(&vanilla_entities::VEX)),
            );
            goal_selector.add_goal(
                1,
                avoid_goal(15.0, 0.5, 0.5, is_type(&vanilla_entities::PILLAGER)),
            );
            goal_selector.add_goal(
                1,
                avoid_goal(12.0, 0.5, 0.5, is_type(&vanilla_entities::ILLUSIONER)),
            );
            goal_selector.add_goal(
                1,
                avoid_goal(10.0, 0.5, 0.5, is_type(&vanilla_entities::ZOGLIN)),
            );
            goal_selector.add_goal(1, PanicGoal::new(0.5));
            goal_selector.add_goal(1, LookAtTradingPlayerGoal::new(Arc::clone(&state)));
            goal_selector.add_goal(2, WanderToPositionGoal::new(Arc::clone(&state), 2.0, 0.35));
            goal_selector.add_goal(4, MoveTowardsRestrictionGoal::new(0.35));
            goal_selector.add_goal(8, WaterAvoidingRandomStrollGoal::new(0.35));
            goal_selector.add_goal(9, InteractGoal::new_player(3.0, 1.0));
            goal_selector.add_goal(
                10,
                LookAtPlayerGoal::new_for_living_entities(8.0, LOOK_AT_PROBABILITY, |living, _| {
                    is_mob_look_target(living)
                }),
            );
        }

        Self {
            base,
            entity_type,
            living_base,
            mob_base,
            ageable_base,
            entity_data: SyncMutex::new(entity_data),
            state,
        }
    }

    /// Vanilla `WanderingTrader.getDespawnDelay`.
    #[must_use]
    pub fn despawn_delay(&self) -> i32 {
        self.state.despawn_delay()
    }

    /// Vanilla `WanderingTrader.setDespawnDelay`.
    pub fn set_despawn_delay(&self, despawn_delay: i32) {
        self.state.set_despawn_delay(despawn_delay);
    }

    /// Vanilla `WanderingTrader.setWanderTarget`.
    pub fn set_wander_target(&self, pos: Option<BlockPos>) {
        self.state.set_wander_target(pos);
    }

    /// Vanilla `WanderingTrader.getWanderTarget`.
    #[must_use]
    pub fn wander_target(&self) -> Option<BlockPos> {
        self.state.wander_target()
    }

    /// Vanilla `AbstractVillager.isTrading`.
    #[must_use]
    pub fn is_trading(&self) -> bool {
        self.state.is_trading()
    }

    /// Vanilla `AbstractVillager.getUnhappyCounter`.
    #[must_use]
    pub fn unhappy_counter(&self) -> i32 {
        *self
            .entity_data
            .lock()
            .abstract_villager
            .unhappy_counter
            .get()
    }

    /// Vanilla `AbstractVillager.setUnhappyCounter`.
    pub fn set_unhappy_counter(&self, value: i32) {
        self.entity_data
            .lock()
            .abstract_villager
            .unhappy_counter
            .set(value);
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

        // Sync base entity flags from resolved effect display state in one place.
        self.entity_data.set_base_invisible_flag(display.invisible);
        self.entity_data
            .set_base_glowing_flag(self.has_glowing_tag() || display.glowing);
    }

    /// Vanilla `WanderingTrader.mobInteract`.
    fn interact_trade(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        if hand == InteractionHand::MainHand {
            player.award_custom_stat(&vanilla_custom_stats::TALKED_TO_VILLAGER);
        }

        let entity = EntityEventSource::as_entity_event_source(self);
        let offers_ready = self.state.ensure_offers(entity);
        if !offers_ready {
            return InteractionResult::Consume;
        }
        if self.state.offers().is_empty() {
            return InteractionResult::Consume;
        }

        let Some(shared_player) = self
            .level()
            .and_then(|world| world.players.get_by_entity_id(player.id()))
        else {
            return InteractionResult::Consume;
        };

        let merchant = Arc::clone(&self.state) as Arc<dyn Merchant>;
        merchant.set_trading_player(Some(shared_player));
        player.open_trading_screen(merchant, self.display_name(), TRADER_LEVEL);
        InteractionResult::SuccessServer
    }
}

impl Entity for WanderingTraderEntity {
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
            self.entity_type.dimensions.scale(scale * BABY_AGE_SCALE)
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

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_mob(nbt);
        self.save_ageable_mob(nbt);

        if self.state.offers_initialized() {
            let offers = self.state.offers();
            nbt.insert("Offers", offers.to_nbt());
        }
        self.state.inventory().lock().write_to_tag(nbt);

        nbt.insert("DespawnDelay", self.state.despawn_delay());
        if let Some(wander_target) = self.state.wander_target() {
            nbt.insert(
                "wander_target",
                NbtTag::IntArray(vec![
                    wander_target.x(),
                    wander_target.y(),
                    wander_target.z(),
                ]),
            );
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_mob(nbt);
        self.load_ageable_mob(nbt);

        if let Some(offers) = nbt
            .compound("Offers")
            .and_then(|compound| MerchantOffers::from_nbt(&compound))
        {
            self.state.set_offers(offers);
        }
        self.state.inventory().lock().read_from_tag(&nbt);

        self.state
            .set_despawn_delay(nbt.int("DespawnDelay").unwrap_or(0));
        self.state
            .set_wander_target(nbt.int_array("wander_target").and_then(|array| {
                (array.len() == 3).then(|| BlockPos::new(array[0], array[1], array[2]))
            }));
        self.set_age(AgeableMob::get_age(self).max(0));
    }
}

impl LivingEntity for WanderingTraderEntity {
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

    fn server_ai_step(&self, entity: &SharedEntity) {
        self.state.bind_entity(entity);
        Mob::mob_server_ai_step(self, entity);

        // Vanilla `WanderingTrader.aiStep` → `maybeDespawn`.
        if self.state.tick_despawn_delay() {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    fn ai_step(&self, entity: &SharedEntity) -> Option<MoveResult> {
        let result = Mob::mob_ai_step(self, entity);
        AgeableMob::tick_ageable_mob(self);
        result
    }

    /// Vanilla `WanderingTrader.getHurtSound`.
    fn hurt_sound(&self, _source: &DamageSource) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WANDERING_TRADER_HURT)
    }

    /// Vanilla `WanderingTrader.getDeathSound`.
    fn death_sound(&self) -> Option<SoundEventRef> {
        Some(&sound_events::ENTITY_WANDERING_TRADER_DEATH)
    }

    /// Needed for the using-item animation bits (`UseItemGoal` drinks) to sync.
    ///
    /// `WanderingTraderEntityData` already carries the living layer via the base
    /// initialization; without this the `DATA_LIVING_ENTITY_FLAGS` bits never sync.
    fn living_synced_data(&self) -> Option<&dyn LivingEntitySyncedData> {
        Some(&self.entity_data)
    }

    /// Vanilla `WanderingTrader implements Consumable.OverrideConsumeSound`.
    fn get_consume_sound(&self, stack: &ItemStack) -> Option<SoundEventRef> {
        Some(if stack.is(&vanilla_items::MILK_BUCKET) {
            &sound_events::ENTITY_WANDERING_TRADER_DRINK_MILK
        } else {
            &sound_events::ENTITY_WANDERING_TRADER_DRINK_POTION
        })
    }
}

impl AgeableMob for WanderingTraderEntity {
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

impl Mob for WanderingTraderEntity {
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

    /// Vanilla `AbstractVillager` is never leashed.
    fn mob_can_be_leashed(&self) -> bool {
        false
    }

    /// Vanilla `WanderingTrader.removeWhenFarAway` is `false`.
    fn remove_when_far_away(&self, _dist_sqr: f64) -> bool {
        false
    }

    fn ambient_sound(&self) -> Option<SoundEventRef> {
        Some(if self.state.is_trading() {
            &sound_events::ENTITY_WANDERING_TRADER_TRADE
        } else {
            &sound_events::ENTITY_WANDERING_TRADER_AMBIENT
        })
    }

    fn finalize_spawn(
        &self,
        world: &Arc<World>,
        spawn_reason: EntitySpawnReason,
        group_data: Option<SpawnGroupData>,
    ) -> Option<SpawnGroupData> {
        // Vanilla `AbstractVillager.finalizeSpawn` never spawns babies.
        let group_data = group_data.or_else(|| {
            Some(SpawnGroupData::AgeableMob(
                AgeableMobGroupData::with_should_spawn_baby(false),
            ))
        });
        self.finalize_spawn_ageable_mob(world, spawn_reason, group_data)
    }

    fn mob_interact(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let is_spawn_egg = player
            .inventory
            .lock()
            .get_item_in_hand(hand)
            .is(&vanilla_items::VILLAGER_SPAWN_EGG);

        if is_spawn_egg
            || !Entity::is_alive(self)
            || self.state.is_trading()
            || AgeableMob::is_baby(self)
        {
            return InteractionResult::Pass;
        }

        self.interact_trade(player, hand)
    }
}

impl PathfinderMob for WanderingTraderEntity {}

impl Npc for WanderingTraderEntity {}

/// Vanilla `PotionContents.createItemStack(Items.POTION, Potions.INVISIBILITY)`.
fn invisibility_potion() -> ItemStack {
    let mut potion = ItemStack::new(&vanilla_items::POTION);
    potion.set(
        vanilla_components::POTION_CONTENTS,
        PotionContents::empty().with_potion(RegistryReference::new(&vanilla_potions::INVISIBILITY)),
    );
    potion
}

/// Vanilla `new AvoidEntityGoal<>(trader, <class>, maxDist, 0.5, 0.5)`.
fn avoid_goal(
    max_dist: f32,
    walk_speed_modifier: f64,
    sprint_speed_modifier: f64,
    selector: impl Fn(&dyn LivingEntity, &World) -> bool + Send + Sync + 'static,
) -> AvoidEntityGoal {
    AvoidEntityGoal::with_selector(
        max_dist,
        walk_speed_modifier,
        sprint_speed_modifier,
        selector,
    )
}

/// Selector matching one entity type by key.
fn is_type(entity_type: &'static EntityType) -> impl Fn(&dyn LivingEntity, &World) -> bool {
    move |target, _| target.entity_type().key == entity_type.key
}

/// Vanilla `AvoidEntityGoal<Zombie>`: the `Zombie` class and its subclasses
/// (`zombie`, `husk`, `drowned`, `zombie_villager`, `zombified_piglin`).
fn is_zombie(target: &dyn LivingEntity, _world: &World) -> bool {
    const ZOMBIE_TYPES: [&EntityType; 5] = [
        &vanilla_entities::ZOMBIE,
        &vanilla_entities::HUSK,
        &vanilla_entities::DROWNED,
        &vanilla_entities::ZOMBIE_VILLAGER,
        &vanilla_entities::ZOMBIFIED_PIGLIN,
    ];

    let key = &target.entity_type().key;
    ZOMBIE_TYPES
        .iter()
        .any(|entity_type| key == &entity_type.key)
}
