//! Vanilla advancement criterion triggers.
//!
//! Mirrors `net.minecraft.advancements.triggers`: one module per trigger class,
//! each exporting the trigger singletons registered in `CriteriaTriggers` (same
//! class, one singleton per id) plus the trigger's instance type.
//!
//! Vanilla passes each trigger its own event type. Steel funnels them through
//! [`TriggerEvent`] so a criterion instance can be tested without downcasting a
//! lifetime-carrying payload; an instance only accepts the variants of the
//! trigger it belongs to.

pub mod block;
pub mod entity;
pub mod inventory;
pub mod player;

pub use block::{
    ALLAY_DROP_ITEM_ON_BLOCK, ANY_BLOCK_USE, AnyBlockInteractionTrigger, BEE_NEST_DESTROYED,
    BeeNestDestroyedTrigger, BeeNestDestroyedTriggerInstance, BlockUseTriggerInstance,
    DEFAULT_BLOCK_USE, DefaultBlockInteractionTrigger, ENTER_BLOCK, EnterBlockTrigger,
    EnterBlockTriggerInstance, ITEM_USED_ON_BLOCK, ItemUsedOnBlockTrigger,
    ItemUsedOnBlockTriggerInstance, PLACED_BLOCK, SLIDE_DOWN_BLOCK, SlideDownBlockTrigger,
    SlideDownBlockTriggerInstance,
};
pub use entity::{
    BRED_ANIMALS, BREWED_POTION, BredAnimalsTrigger, BredAnimalsTriggerInstance,
    BrewedPotionTrigger, BrewedPotionTriggerInstance, CHANGED_DIMENSION, CHANNELED_LIGHTNING,
    CURED_ZOMBIE_VILLAGER, ChangedDimensionTrigger, ChangedDimensionTriggerInstance,
    ChanneledLightningTrigger, ChanneledLightningTriggerInstance, CuredZombieVillagerTrigger,
    CuredZombieVillagerTriggerInstance, EFFECTS_CHANGED, ENTITY_HURT_PLAYER, ENTITY_KILLED_PLAYER,
    EffectsChangedTrigger, EffectsChangedTriggerInstance, EntityConditionTrigger,
    EntityConditionTriggerInstance, EntityHurtPlayerTrigger, EntityHurtPlayerTriggerInstance,
    FISHING_ROD_HOOKED, FishingRodHookedTrigger, FishingRodHookedTriggerInstance,
    KILL_MOB_NEAR_SCULK_CATALYST, KILLED_BY_ARROW, KilledByArrowTrigger,
    KilledByArrowTriggerInstance, KilledTrigger, KilledTriggerInstance, LEVITATION,
    LIGHTNING_STRIKE, LevitationTrigger, LevitationTriggerInstance, LightningStrikeTrigger,
    LightningStrikeTriggerInstance, PLAYER_HURT_ENTITY, PLAYER_KILLED_ENTITY,
    PlayerHurtEntityTrigger, PlayerHurtEntityTriggerInstance, SUMMONED_ENTITY, TAME_ANIMAL,
    TARGET_HIT, TargetHitTrigger, TargetHitTriggerInstance,
};
pub use inventory::{
    CONSTRUCT_BEACON, CONSUME_ITEM, CRAFTER_RECIPE_CRAFTED, ConstructBeaconTrigger,
    ConstructBeaconTriggerInstance, DistanceTrigger, DistanceTriggerInstance, ENCHANTED_ITEM,
    EnchantedItemTrigger, EnchantedItemTriggerInstance, FALL_AFTER_EXPLOSION, FALL_FROM_HEIGHT,
    FILLED_BUCKET, FallAfterExplosionTrigger, FallAfterExplosionTriggerInstance, GENERATE_LOOT,
    INVENTORY_CHANGED, ITEM_DURABILITY_CHANGED, InventoryChangedSlots, InventoryChangedTrigger,
    InventoryChangedTriggerInstance, ItemDurabilityChangedTrigger,
    ItemDurabilityChangedTriggerInstance, ItemTrigger, ItemTriggerInstance, LootTableTrigger,
    LootTableTriggerInstance, NETHER_TRAVEL, PLAYER_INTERACTED_WITH_ENTITY,
    PLAYER_SHEARED_EQUIPMENT, PickedUpItemTrigger, PickedUpItemTriggerInstance,
    PlayerInteractedWithEntityTrigger, PlayerInteractedWithEntityTriggerInstance, RECIPE_CRAFTED,
    RECIPE_UNLOCKED, RIDE_ENTITY_IN_LAVA, RecipeCraftedTrigger, RecipeCraftedTriggerInstance,
    RecipeUnlockedTrigger, RecipeUnlockedTriggerInstance, SHOT_CROSSBOW, SPEAR_MOBS,
    SpearMobsTrigger, SpearMobsTriggerInstance, THROWN_ITEM_PICKED_UP_BY_ENTITY,
    THROWN_ITEM_PICKED_UP_BY_PLAYER, TRADE, TradeTrigger, TradeTriggerInstance, USED_ENDER_EYE,
    USED_TOTEM, USING_ITEM, UsedEnderEyeTrigger, UsedEnderEyeTriggerInstance,
};
pub use player::{
    AVOID_VIBRATION, LOCATION, PlayerTrigger, PlayerTriggerInstance, RAID_OMEN, RAID_WIN,
    SLEPT_IN_BED, START_RIDING, StartRidingTrigger, StartRidingTriggerInstance, TICK,
};

use crate::advancement::predicates::{
    AdvancementContext, Condition, DamageView, EffectInstanceView, EntityView, LevelView,
    player_condition_matches,
};
use crate::item_stack::ItemStack;
use steel_utils::{BlockPos, BlockStateId, Identifier};

/// The `SimpleCriterionTrigger.SimpleInstance.player` condition shared by every
/// non-impossible trigger.
#[derive(Debug, Default)]
pub struct PlayerCondition {
    /// Vanilla `SimpleInstance.player`.
    pub player: Option<Condition>,
}

impl PlayerCondition {
    /// Tests the shared `player` condition against the trigger context.
    #[must_use]
    pub fn matches(&self, ctx: &AdvancementContext<'_>) -> bool {
        player_condition_matches(self.player.as_ref(), ctx)
    }
}

/// The fields every trigger event carries.
#[derive(Clone, Copy)]
pub struct EventBase<'a> {
    /// The player the criterion is evaluated for.
    pub player: &'a EntityView<'a>,
    /// World facts at the event position.
    pub level: &'a dyn LevelView,
    /// Block position of the event.
    pub origin: BlockPos,
}

impl<'a> EventBase<'a> {
    /// The advancement context this event exposes to shared conditions.
    #[must_use]
    pub fn context(&self, tool: Option<&'a ItemStack>) -> AdvancementContext<'a> {
        AdvancementContext::for_player(self.player, self.level, self.origin, tool)
    }

    /// A context whose `this` entity is a non-player entity of the event.
    #[must_use]
    pub fn context_for(&self, entity: &'a EntityView<'a>) -> AdvancementContext<'a> {
        AdvancementContext::for_entity(entity, self.player, self.level, self.origin, None)
    }
}

/// Every advancement trigger event, tagged by the trigger that raised it.
#[derive(Clone, Copy)]
pub enum TriggerEvent<'a> {
    /// `location`, `slept_in_bed`, `tick`, `hero_of_the_village`, `voluntary_exile`, `avoid_vibration`.
    Player(EventBase<'a>),
    /// `started_riding`.
    StartRiding(EventBase<'a>),
    /// `inventory_changed`.
    InventoryChanged {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Every slot of the player inventory at trigger time.
        inventory: &'a [ItemStack],
        /// The item that changed.
        changed_item: &'a ItemStack,
        /// Number of slots holding a full stack.
        slots_full: i32,
        /// Number of empty slots.
        slots_empty: i32,
        /// Number of occupied slots.
        slots_occupied: i32,
    },
    /// `enter_block`.
    EnterBlock {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The block state entered.
        state: BlockStateId,
    },
    /// `slide_down_block`.
    SlideDownBlock {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The block state slid down.
        state: BlockStateId,
    },
    /// `item_durability_changed`.
    ItemDurabilityChanged {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The item whose durability changed.
        item: &'a ItemStack,
        /// The item's durability after the change.
        new_durability: i32,
    },
    /// `bee_nest_destroyed`.
    BeeNestDestroyed {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The destroyed nest state.
        state: BlockStateId,
        /// The tool used to destroy it.
        tool: &'a ItemStack,
        /// Bees inside when it was destroyed.
        bees_inside: i32,
    },
    /// `placed_block`.
    PlacedBlock {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The placed block's item. Vanilla passes it as the location context's
        /// `tool`, exactly like `item_used_on_block`.
        tool: &'a ItemStack,
    },
    /// `item_used_on_block`.
    ItemUsedOnBlock {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The used tool.
        tool: &'a ItemStack,
    },
    /// `allay_drop_item_on_block`.
    AllayDropItemOnBlock {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The dropped item.
        tool: &'a ItemStack,
    },
    /// `any_block_use`, `default_block_use`.
    BlockUse(EventBase<'a>),
    /// `fall_after_explosion`.
    FallAfterExplosion {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Where the fall started.
        start: (f64, f64, f64),
        /// The explosion's source entity, if any.
        cause: Option<&'a EntityView<'a>>,
    },
    /// `fall_from_height`, `nether_travel`, `ride_entity_in_lava`.
    Distance {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Where the traveled distance started.
        start: (f64, f64, f64),
    },
    /// `levitation`.
    Levitation {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Where the levitation started.
        start: (f64, f64, f64),
        /// Duration of the levitation in ticks.
        duration: i32,
    },
    /// `bred_animals`.
    BredAnimals {
        /// Shared event fields.
        base: EventBase<'a>,
        /// One parent.
        parent: &'a EntityView<'a>,
        /// The other parent.
        partner: &'a EntityView<'a>,
        /// The child, once it exists.
        child: Option<&'a EntityView<'a>>,
    },
    /// `brewed_potion`.
    BrewedPotion {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The brewed potion.
        potion: Option<&'a Identifier>,
    },
    /// `changed_dimension`.
    ChangedDimension {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The dimension left.
        from: &'a Identifier,
        /// The dimension entered.
        to: &'a Identifier,
    },
    /// `channeled_lightning`.
    ChanneledLightning {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Every entity struck.
        victims: &'a [&'a EntityView<'a>],
    },
    /// `construct_beacon`.
    ConstructBeacon {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The beacon level.
        levels: i32,
    },
    /// `consume_item`.
    ConsumeItem {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The consumed item.
        item: &'a ItemStack,
    },
    /// `cured_zombie_villager`.
    CuredZombieVillager {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The cured zombie villager.
        zombie: &'a EntityView<'a>,
        /// The villager it became.
        villager: &'a EntityView<'a>,
    },
    /// `effects_changed`.
    EffectsChanged {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The player's effects after the change.
        effects: &'a [EffectInstanceView<'a>],
        /// The effect source, if any.
        source: Option<&'a EntityView<'a>>,
    },
    /// `enchanted_item`.
    EnchantedItem {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The enchanted item.
        item: &'a ItemStack,
        /// The enchanting level spent.
        levels: i32,
    },
    /// `entity_hurt_player`.
    EntityHurtPlayer {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The damage taken.
        damage: DamageView<'a>,
    },
    /// `player_hurt_entity`.
    PlayerHurtEntity {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The damage dealt.
        damage: DamageView<'a>,
        /// The entity hurt.
        victim: &'a EntityView<'a>,
    },
    /// `filled_bucket`.
    FilledBucket {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The filled bucket.
        item: &'a ItemStack,
    },
    /// `fishing_rod_hooked`.
    FishingRodHooked {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The fishing rod.
        rod: &'a ItemStack,
        /// The hooked entity, if any.
        hooked: Option<&'a EntityView<'a>>,
        /// Items retrieved by the hook.
        items: &'a [ItemStack],
    },
    /// `player_killed_entity`.
    PlayerKilledEntity {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The killed entity.
        entity: &'a EntityView<'a>,
        /// The killing blow, if any.
        killing_blow: Option<DamageView<'a>>,
    },
    /// `entity_killed_player`.
    EntityKilledPlayer {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The killing entity.
        entity: &'a EntityView<'a>,
        /// The killing blow, if any.
        killing_blow: Option<DamageView<'a>>,
    },
    /// `kill_mob_near_sculk_catalyst`.
    KillMobNearSculkCatalyst {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The killed entity.
        entity: &'a EntityView<'a>,
        /// The killing blow, if any.
        killing_blow: Option<DamageView<'a>>,
    },
    /// `killed_by_arrow`.
    KilledByArrow {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Every entity killed by the arrow.
        victims: &'a [&'a EntityView<'a>],
        /// The bow/crossbow that fired it.
        weapon: Option<&'a ItemStack>,
        /// Number of distinct victim entity types.
        unique_entity_types: i32,
    },
    /// `lightning_strike`.
    LightningStrike {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The lightning bolt entity.
        lightning: &'a EntityView<'a>,
        /// Entities near the strike.
        bystanders: &'a [&'a EntityView<'a>],
    },
    /// `player_generates_container_loot`.
    LootTable {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The loot table that was rolled.
        loot_table: &'a Identifier,
    },
    /// `thrown_item_picked_up_by_entity`.
    ThrownItemPickedUpByEntity {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The picked-up item.
        item: &'a ItemStack,
        /// The picking entity.
        picker: Option<&'a EntityView<'a>>,
    },
    /// `thrown_item_picked_up_by_player`.
    ThrownItemPickedUpByPlayer {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The picked-up item.
        item: &'a ItemStack,
        /// The picking player.
        picker: Option<&'a EntityView<'a>>,
    },
    /// `player_interacted_with_entity`.
    PlayerInteractedWithEntity {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The item used.
        item: &'a ItemStack,
        /// The entity interacted with.
        entity: &'a EntityView<'a>,
    },
    /// `player_sheared_equipment`.
    PlayerShearedEquipment {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The shears used.
        item: &'a ItemStack,
        /// The sheared entity.
        entity: &'a EntityView<'a>,
    },
    /// `recipe_crafted`.
    RecipeCrafted {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The crafted recipe.
        recipe: &'a Identifier,
        /// The ingredients consumed.
        ingredients: &'a [ItemStack],
    },
    /// `crafter_recipe_crafted`.
    CrafterRecipeCrafted {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The crafted recipe.
        recipe: &'a Identifier,
        /// The ingredients consumed.
        ingredients: &'a [ItemStack],
    },
    /// `recipe_unlocked`.
    RecipeUnlocked {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The unlocked recipe.
        recipe: &'a Identifier,
    },
    /// `shot_crossbow`.
    ShotCrossbow {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The crossbow.
        item: &'a ItemStack,
    },
    /// `spear_mobs`.
    SpearMobs {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Number of mobs speared.
        count: i32,
    },
    /// `summoned_entity`.
    SummonedEntity {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The summoned entity.
        entity: &'a EntityView<'a>,
    },
    /// `tame_animal`.
    TameAnimal {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The tamed animal.
        entity: &'a EntityView<'a>,
    },
    /// `target_hit`.
    TargetHit {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The projectile that hit.
        projectile: &'a EntityView<'a>,
        /// The emitted redstone signal strength.
        signal_strength: i32,
    },
    /// `villager_trade`.
    VillagerTrade {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The trading villager.
        villager: &'a EntityView<'a>,
        /// The item received.
        item: &'a ItemStack,
    },
    /// `used_ender_eye`.
    UsedEnderEye {
        /// Shared event fields.
        base: EventBase<'a>,
        /// Squared horizontal distance to the stronghold.
        distance_sqr: f64,
    },
    /// `used_totem`.
    UsedTotem {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The totem used.
        item: &'a ItemStack,
    },
    /// `using_item`.
    UsingItem {
        /// Shared event fields.
        base: EventBase<'a>,
        /// The item being used.
        item: &'a ItemStack,
    },
}

impl<'a> TriggerEvent<'a> {
    /// The fields shared by every event.
    #[must_use]
    pub const fn base(&self) -> EventBase<'a> {
        match self {
            Self::Player(base) | Self::StartRiding(base) | Self::BlockUse(base) => *base,
            Self::PlacedBlock { base, .. }
            | Self::InventoryChanged { base, .. }
            | Self::EnterBlock { base, .. }
            | Self::SlideDownBlock { base, .. }
            | Self::BeeNestDestroyed { base, .. }
            | Self::ItemDurabilityChanged { base, .. }
            | Self::ItemUsedOnBlock { base, .. }
            | Self::AllayDropItemOnBlock { base, .. }
            | Self::FallAfterExplosion { base, .. }
            | Self::Distance { base, .. }
            | Self::Levitation { base, .. }
            | Self::BredAnimals { base, .. }
            | Self::BrewedPotion { base, .. }
            | Self::ChangedDimension { base, .. }
            | Self::ChanneledLightning { base, .. }
            | Self::ConstructBeacon { base, .. }
            | Self::ConsumeItem { base, .. }
            | Self::CuredZombieVillager { base, .. }
            | Self::EffectsChanged { base, .. }
            | Self::EnchantedItem { base, .. }
            | Self::EntityHurtPlayer { base, .. }
            | Self::PlayerHurtEntity { base, .. }
            | Self::FilledBucket { base, .. }
            | Self::FishingRodHooked { base, .. }
            | Self::PlayerKilledEntity { base, .. }
            | Self::EntityKilledPlayer { base, .. }
            | Self::KillMobNearSculkCatalyst { base, .. }
            | Self::KilledByArrow { base, .. }
            | Self::LightningStrike { base, .. }
            | Self::LootTable { base, .. }
            | Self::ThrownItemPickedUpByEntity { base, .. }
            | Self::ThrownItemPickedUpByPlayer { base, .. }
            | Self::PlayerInteractedWithEntity { base, .. }
            | Self::PlayerShearedEquipment { base, .. }
            | Self::RecipeCrafted { base, .. }
            | Self::CrafterRecipeCrafted { base, .. }
            | Self::RecipeUnlocked { base, .. }
            | Self::ShotCrossbow { base, .. }
            | Self::SpearMobs { base, .. }
            | Self::SummonedEntity { base, .. }
            | Self::TameAnimal { base, .. }
            | Self::TargetHit { base, .. }
            | Self::VillagerTrade { base, .. }
            | Self::UsedEnderEye { base, .. }
            | Self::UsedTotem { base, .. }
            | Self::UsingItem { base, .. } => *base,
        }
    }
}
