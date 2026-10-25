//! Inventory, recipe and item-use criteria.
//!
//! Covers `InventoryChangeTrigger`, `RecipeUnlockedTrigger`, `RecipeCraftedTrigger`,
//! `ItemDurabilityTrigger`, `LootTableTrigger`, `PickedUpItemTrigger`,
//! `PlayerInteractTrigger`, `ConsumeItemTrigger`, `FilledBucketTrigger`,
//! `UsedTotemTrigger`, `UsingItemTrigger`, `ShotCrossbowTrigger`, `TradeTrigger`,
//! `SpearMobsTrigger`, `UsedEnderEyeTrigger`, `DistanceTrigger`,
//! `FallAfterExplosionTrigger` and `ConstructBeaconTrigger`.

use crate::advancement::criterion::{CriterionTrigger, CriterionTriggerInstance};
use crate::advancement::predicates::{
    AdvancementContext, Condition, DistancePredicate, HolderSet, IntBounds, ItemPredicate,
    LocationPredicate, PlayerView, item_predicate_matches,
};
use crate::advancement::triggers::{EventBase, PlayerCondition, TriggerEvent};
use crate::item_stack::ItemStack;
use steel_utils::Identifier;

/// Vanilla `InventoryChangeTrigger.TriggerInstance.Slots`.
#[derive(Debug, Clone, Default)]
pub struct InventoryChangedSlots {
    /// Bounds on the number of occupied slots.
    pub occupied: IntBounds,
    /// Bounds on the number of completely full slots.
    pub full: IntBounds,
    /// Bounds on the number of empty slots.
    pub empty: IntBounds,
}

impl InventoryChangedSlots {
    /// Vanilla `Slots.matches(slotsFull, slotsEmpty, slotsOccupied)`.
    #[must_use]
    pub fn matches(&self, slots_full: i32, slots_empty: i32, slots_occupied: i32) -> bool {
        self.full.matches(slots_full)
            && self.empty.matches(slots_empty)
            && self.occupied.matches(slots_occupied)
    }
}

/// Instance of an `inventory_changed` criterion (`InventoryChangeTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct InventoryChangedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `slots`.
    pub slots: InventoryChangedSlots,
    /// Vanilla `items`: each predicate must match a distinct non-empty slot.
    pub items: &'static [ItemPredicate],
}

impl InventoryChangedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(inventory, changedItem, slotsFull, slotsEmpty, slotsOccupied)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::InventoryChanged {
            base,
            inventory,
            changed_item,
            slots_full,
            slots_empty,
            slots_occupied,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if !self.slots.matches(slots_full, slots_empty, slots_occupied) {
            return false;
        }
        if self.items.is_empty() {
            return true;
        }
        if self.items.len() == 1 {
            return !changed_item.is_empty()
                && item_predicate_matches(&self.items[0], changed_item);
        }
        let mut remaining: Vec<usize> = (0..self.items.len()).collect();
        for stack in inventory {
            if remaining.is_empty() {
                return true;
            }
            if stack.is_empty() {
                continue;
            }
            remaining.retain(|&index| !item_predicate_matches(&self.items[index], stack));
        }
        remaining.is_empty()
    }
}

impl CriterionTriggerInstance for InventoryChangedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.INVENTORY_CHANGED`.
#[derive(Debug)]
pub struct InventoryChangedTrigger;

impl CriterionTrigger for InventoryChangedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("inventory_changed")
    }
}

/// The static `minecraft:inventory_changed` trigger.
pub static INVENTORY_CHANGED: InventoryChangedTrigger = InventoryChangedTrigger;

/// Instance of a `recipe_unlocked` criterion (`RecipeUnlockedTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct RecipeUnlockedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `recipes`: required recipe holder set.
    pub recipes: HolderSet,
}

impl RecipeUnlockedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(recipe)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::RecipeUnlocked { base, recipe } = *event else {
            return false;
        };
        self.player.matches(&base.context(None)) && self.recipes.contains_recipe(recipe)
    }
}

impl CriterionTriggerInstance for RecipeUnlockedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.RECIPE_UNLOCKED`.
#[derive(Debug)]
pub struct RecipeUnlockedTrigger;

impl CriterionTrigger for RecipeUnlockedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("recipe_unlocked")
    }
}

/// The static `minecraft:recipe_unlocked` trigger.
pub static RECIPE_UNLOCKED: RecipeUnlockedTrigger = RecipeUnlockedTrigger;

/// Instance of a `recipe_crafted`/`crafter_recipe_crafted` criterion.
#[derive(Debug, Default)]
pub struct RecipeCraftedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `recipes`: required recipe holder set.
    pub recipes: HolderSet,
    /// Vanilla `ingredients`: each predicate must consume a distinct used stack.
    pub ingredients: &'static [ItemPredicate],
}

impl RecipeCraftedTriggerInstance {
    fn matches_inner(
        &self,
        base: &EventBase<'_>,
        recipe: &Identifier,
        ingredients: &[ItemStack],
    ) -> bool {
        if !self.player.matches(&base.context(None)) || !self.recipes.contains_recipe(recipe) {
            return false;
        }
        let mut remaining: Vec<usize> = (0..self.ingredients.len()).collect();
        for stack in ingredients {
            if remaining.is_empty() {
                return true;
            }
            remaining.retain(|&index| !item_predicate_matches(&self.ingredients[index], stack));
        }
        remaining.is_empty()
    }

    /// Vanilla `RecipeCraftedTrigger.TriggerInstance.matches(recipe, items)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        match *event {
            TriggerEvent::RecipeCrafted {
                ref base,
                recipe,
                ingredients,
            }
            | TriggerEvent::CrafterRecipeCrafted {
                ref base,
                recipe,
                ingredients,
            } => self.matches_inner(base, recipe, ingredients),
            _ => false,
        }
    }
}

impl CriterionTriggerInstance for RecipeCraftedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.RECIPE_CRAFTED`.
#[derive(Debug)]
pub struct RecipeCraftedTrigger {
    id: &'static str,
}

impl RecipeCraftedTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for RecipeCraftedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:recipe_crafted` trigger.
pub static RECIPE_CRAFTED: RecipeCraftedTrigger = RecipeCraftedTrigger::new("recipe_crafted");
/// The static `minecraft:crafter_recipe_crafted` trigger.
pub static CRAFTER_RECIPE_CRAFTED: RecipeCraftedTrigger =
    RecipeCraftedTrigger::new("crafter_recipe_crafted");

/// Instance of an `item_durability_changed` criterion.
#[derive(Debug, Default)]
pub struct ItemDurabilityChangedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `item`.
    pub item: Option<ItemPredicate>,
    /// Vanilla `durability`: bounds on `maxDamage - newDurability`.
    pub durability: IntBounds,
    /// Vanilla `delta`: bounds on `damage - newDurability`.
    pub delta: IntBounds,
}

impl ItemDurabilityChangedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(itemStack, newDurability)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::ItemDurabilityChanged {
            base,
            item,
            new_durability,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if self
            .item
            .as_ref()
            .is_some_and(|predicate| !item_predicate_matches(predicate, item))
        {
            return false;
        }
        self.durability
            .matches(item.get_max_damage() - new_durability)
            && self.delta.matches(item.get_damage_value() - new_durability)
    }
}

impl CriterionTriggerInstance for ItemDurabilityChangedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.ITEM_DURABILITY_CHANGED`.
#[derive(Debug)]
pub struct ItemDurabilityChangedTrigger;

impl CriterionTrigger for ItemDurabilityChangedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("item_durability_changed")
    }
}

/// The static `minecraft:item_durability_changed` trigger.
pub static ITEM_DURABILITY_CHANGED: ItemDurabilityChangedTrigger = ItemDurabilityChangedTrigger;

/// Instance of a `player_generates_container_loot` criterion.
#[derive(Debug, Default)]
pub struct LootTableTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `loot_tables`: required loot-table holder set.
    pub loot_tables: HolderSet,
}

impl LootTableTriggerInstance {
    /// Vanilla `TriggerInstance.matches(lootTable)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::LootTable { base, loot_table } = *event else {
            return false;
        };
        self.player.matches(&base.context(None)) && self.loot_tables.contains_loot_table(loot_table)
    }
}

impl CriterionTriggerInstance for LootTableTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.GENERATE_LOOT`.
#[derive(Debug)]
pub struct LootTableTrigger;

impl CriterionTrigger for LootTableTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("player_generates_container_loot")
    }
}

/// The static `minecraft:player_generates_container_loot` trigger.
pub static GENERATE_LOOT: LootTableTrigger = LootTableTrigger;

/// Instance of a `thrown_item_picked_up_by_entity`/`_by_player` criterion.
#[derive(Debug, Default)]
pub struct PickedUpItemTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `item`.
    pub item: Option<ItemPredicate>,
    /// Vanilla `entity`: the picker.
    pub entity: Option<Condition>,
}

impl PickedUpItemTriggerInstance {
    /// Vanilla `PickedUpItemTrigger.TriggerInstance.matches(stack, pickedBy)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let (TriggerEvent::ThrownItemPickedUpByEntity {
            ref base,
            item,
            picker,
        }
        | TriggerEvent::ThrownItemPickedUpByPlayer {
            ref base,
            item,
            picker,
        }) = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if self
            .item
            .as_ref()
            .is_some_and(|predicate| !item_predicate_matches(predicate, item))
        {
            return false;
        }
        self.entity.as_ref().is_none_or(|condition| {
            picker.is_some_and(|picker| {
                condition.test(&AdvancementContext::for_entity(
                    picker,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        })
    }
}

impl CriterionTriggerInstance for PickedUpItemTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.THROWN_ITEM_PICKED_UP_BY_ENTITY`/`_BY_PLAYER`.
#[derive(Debug)]
pub struct PickedUpItemTrigger {
    id: &'static str,
}

impl PickedUpItemTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for PickedUpItemTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:thrown_item_picked_up_by_entity` trigger.
pub static THROWN_ITEM_PICKED_UP_BY_ENTITY: PickedUpItemTrigger =
    PickedUpItemTrigger::new("thrown_item_picked_up_by_entity");
/// The static `minecraft:thrown_item_picked_up_by_player` trigger.
pub static THROWN_ITEM_PICKED_UP_BY_PLAYER: PickedUpItemTrigger =
    PickedUpItemTrigger::new("thrown_item_picked_up_by_player");

/// Instance of a `player_interacted_with_entity`/`player_sheared_equipment` criterion.
#[derive(Debug, Default)]
pub struct PlayerInteractedWithEntityTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `item`.
    pub item: Option<ItemPredicate>,
    /// Vanilla `entity`: the interacted-with entity.
    pub entity: Option<Condition>,
}

impl PlayerInteractedWithEntityTriggerInstance {
    /// Vanilla `PlayerInteractTrigger.TriggerInstance.matches(stack, entity)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let (TriggerEvent::PlayerInteractedWithEntity {
            ref base,
            item,
            entity,
        }
        | TriggerEvent::PlayerShearedEquipment {
            ref base,
            item,
            entity,
        }) = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if self
            .item
            .as_ref()
            .is_some_and(|predicate| !item_predicate_matches(predicate, item))
        {
            return false;
        }
        self.entity.as_ref().is_none_or(|condition| {
            condition.test(&AdvancementContext::for_entity(
                entity,
                base.player,
                base.level,
                base.origin,
                None,
            ))
        })
    }
}

impl CriterionTriggerInstance for PlayerInteractedWithEntityTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.PLAYER_INTERACTED_WITH_ENTITY`/`PLAYER_SHEARED_EQUIPMENT`.
#[derive(Debug)]
pub struct PlayerInteractedWithEntityTrigger {
    id: &'static str,
}

impl PlayerInteractedWithEntityTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for PlayerInteractedWithEntityTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:player_interacted_with_entity` trigger.
pub static PLAYER_INTERACTED_WITH_ENTITY: PlayerInteractedWithEntityTrigger =
    PlayerInteractedWithEntityTrigger::new("player_interacted_with_entity");
/// The static `minecraft:player_sheared_equipment` trigger.
pub static PLAYER_SHEARED_EQUIPMENT: PlayerInteractedWithEntityTrigger =
    PlayerInteractedWithEntityTrigger::new("player_sheared_equipment");

/// Instance of an item-only criterion (`consume_item`, `filled_bucket`,
/// `used_totem`, `using_item`, `shot_crossbow`).
#[derive(Debug, Default)]
pub struct ItemTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `item`.
    pub item: Option<ItemPredicate>,
}

impl ItemTriggerInstance {
    /// Vanilla `matches(stack)` for the item-only triggers.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let (TriggerEvent::ConsumeItem { ref base, item }
        | TriggerEvent::FilledBucket { ref base, item }
        | TriggerEvent::UsedTotem { ref base, item }
        | TriggerEvent::UsingItem { ref base, item }
        | TriggerEvent::ShotCrossbow { ref base, item }) = *event
        else {
            return false;
        };
        self.player.matches(&base.context(Some(item)))
            && self
                .item
                .as_ref()
                .is_none_or(|predicate| item_predicate_matches(predicate, item))
    }
}

impl CriterionTriggerInstance for ItemTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// An item-only trigger singleton; one per `CriteriaTriggers` id.
#[derive(Debug)]
pub struct ItemTrigger {
    id: &'static str,
}

impl ItemTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for ItemTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// `CriteriaTriggers.CONSUME_ITEM`.
pub static CONSUME_ITEM: ItemTrigger = ItemTrigger::new("consume_item");
/// `CriteriaTriggers.FILLED_BUCKET`.
pub static FILLED_BUCKET: ItemTrigger = ItemTrigger::new("filled_bucket");
/// `CriteriaTriggers.USED_TOTEM`.
pub static USED_TOTEM: ItemTrigger = ItemTrigger::new("used_totem");
/// `CriteriaTriggers.USING_ITEM`.
pub static USING_ITEM: ItemTrigger = ItemTrigger::new("using_item");
/// `CriteriaTriggers.SHOT_CROSSBOW`.
pub static SHOT_CROSSBOW: ItemTrigger = ItemTrigger::new("shot_crossbow");

/// Instance of a `villager_trade` criterion (`TradeTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct TradeTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `villager`.
    pub villager: Option<Condition>,
    /// Vanilla `item`.
    pub item: Option<ItemPredicate>,
}

impl TradeTriggerInstance {
    /// Vanilla `TradeTrigger.TriggerInstance.matches(villager, stack)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::VillagerTrade {
            ref base,
            villager,
            item,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(Some(item))) {
            return false;
        }
        if let Some(condition) = &self.villager
            && !condition.test(&AdvancementContext::for_entity(
                villager,
                base.player,
                base.level,
                base.origin,
                Some(item),
            ))
        {
            return false;
        }
        self.item
            .as_ref()
            .is_none_or(|predicate| item_predicate_matches(predicate, item))
    }
}

impl CriterionTriggerInstance for TradeTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.TRADE`.
#[derive(Debug)]
pub struct TradeTrigger;

impl CriterionTrigger for TradeTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("villager_trade")
    }
}

/// The static `minecraft:villager_trade` trigger.
pub static TRADE: TradeTrigger = TradeTrigger;

/// Instance of a `spear_mobs` criterion (`SpearMobsTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct SpearMobsTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `count`: minimum number of speared mobs.
    pub count: Option<i32>,
}

impl SpearMobsTriggerInstance {
    /// Vanilla `TriggerInstance.matches(number)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::SpearMobs { base, count } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self.count.is_none_or(|required| count >= required)
    }
}

impl CriterionTriggerInstance for SpearMobsTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.SPEAR_MOBS_TRIGGER`.
#[derive(Debug)]
pub struct SpearMobsTrigger;

impl CriterionTrigger for SpearMobsTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("spear_mobs")
    }
}

/// The static `minecraft:spear_mobs` trigger.
pub static SPEAR_MOBS: SpearMobsTrigger = SpearMobsTrigger;

/// Instance of a `used_ender_eye` criterion (`UsedEnderEyeTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct UsedEnderEyeTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `distance`: bounds on the squared horizontal distance.
    pub distance: crate::item_predicate::DoubleBounds,
}

impl UsedEnderEyeTriggerInstance {
    /// Vanilla `TriggerInstance.matches(sqrDistance)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::UsedEnderEye { base, distance_sqr } = *event else {
            return false;
        };
        self.player.matches(&base.context(None)) && self.distance.matches_sqr(distance_sqr)
    }
}

impl CriterionTriggerInstance for UsedEnderEyeTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.USED_ENDER_EYE`.
#[derive(Debug)]
pub struct UsedEnderEyeTrigger;

impl CriterionTrigger for UsedEnderEyeTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("used_ender_eye")
    }
}

/// The static `minecraft:used_ender_eye` trigger.
pub static USED_ENDER_EYE: UsedEnderEyeTrigger = UsedEnderEyeTrigger;

/// Instance of a `fall_from_height`/`nether_travel`/`ride_entity_in_lava` criterion.
#[derive(Debug, Default)]
pub struct DistanceTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `start_position`.
    pub start_position: Option<LocationPredicate>,
    /// Vanilla `distance`.
    pub distance: Option<DistancePredicate>,
}

impl DistanceTriggerInstance {
    /// Vanilla `TriggerInstance.matches(level, enteredPosition, playerPosition)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::Distance { base, start } = *event else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if let Some(start_position) = &self.start_position
            && !start_position.matches_at(base.level, start)
        {
            return false;
        }
        self.distance
            .as_ref()
            .is_none_or(|distance| distance.matches(start, base.level.position()))
    }
}

impl CriterionTriggerInstance for DistanceTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.FALL_FROM_HEIGHT`/`NETHER_TRAVEL`/`RIDE_ENTITY_IN_LAVA_TRIGGER`.
#[derive(Debug)]
pub struct DistanceTrigger {
    id: &'static str,
}

impl DistanceTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for DistanceTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:fall_from_height` trigger.
pub static FALL_FROM_HEIGHT: DistanceTrigger = DistanceTrigger::new("fall_from_height");
/// The static `minecraft:nether_travel` trigger.
pub static NETHER_TRAVEL: DistanceTrigger = DistanceTrigger::new("nether_travel");
/// The static `minecraft:ride_entity_in_lava` trigger.
pub static RIDE_ENTITY_IN_LAVA: DistanceTrigger = DistanceTrigger::new("ride_entity_in_lava");

/// Instance of a `fall_after_explosion` criterion.
#[derive(Debug, Default)]
pub struct FallAfterExplosionTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `start_position`.
    pub start_position: Option<LocationPredicate>,
    /// Vanilla `distance`.
    pub distance: Option<DistancePredicate>,
    /// Vanilla `cause`.
    pub cause: Option<Condition>,
}

impl FallAfterExplosionTriggerInstance {
    /// Vanilla `TriggerInstance.matches(level, start, end, cause)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::FallAfterExplosion { base, start, cause } = *event else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if let Some(start_position) = &self.start_position
            && !start_position.matches_at(base.level, start)
        {
            return false;
        }
        if let Some(distance) = &self.distance
            && !distance.matches(start, base.level.position())
        {
            return false;
        }
        self.cause.as_ref().is_none_or(|condition| {
            cause.is_some_and(|cause| {
                condition.test(&AdvancementContext::for_entity(
                    cause,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        })
    }
}

impl CriterionTriggerInstance for FallAfterExplosionTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.FALL_AFTER_EXPLOSION`.
#[derive(Debug)]
pub struct FallAfterExplosionTrigger;

impl CriterionTrigger for FallAfterExplosionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("fall_after_explosion")
    }
}

/// The static `minecraft:fall_after_explosion` trigger.
pub static FALL_AFTER_EXPLOSION: FallAfterExplosionTrigger = FallAfterExplosionTrigger;

/// Instance of a `construct_beacon` criterion.
#[derive(Debug, Default)]
pub struct ConstructBeaconTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `level`: bounds on the beacon level.
    pub level: IntBounds,
}

impl ConstructBeaconTriggerInstance {
    /// Vanilla `TriggerInstance.matches(levels)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::ConstructBeacon { base, levels } = *event else {
            return false;
        };
        self.player.matches(&base.context(None)) && self.level.matches(levels)
    }
}

impl CriterionTriggerInstance for ConstructBeaconTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.CONSTRUCT_BEACON`.
#[derive(Debug)]
pub struct ConstructBeaconTrigger;

impl CriterionTrigger for ConstructBeaconTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("construct_beacon")
    }
}

/// The static `minecraft:construct_beacon` trigger.
pub static CONSTRUCT_BEACON: ConstructBeaconTrigger = ConstructBeaconTrigger;

/// Instance of an `enchanted_item` criterion (`EnchantedItemTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct EnchantedItemTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `item`: the enchanted stack.
    pub item: Option<ItemPredicate>,
    /// Vanilla `levels`: bounds on the enchanting level spent.
    pub levels: IntBounds,
}

impl EnchantedItemTriggerInstance {
    /// Vanilla `TriggerInstance.matches(stack, levels)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::EnchantedItem { base, item, levels } = *event else {
            return false;
        };
        self.player.matches(&base.context(Some(item)))
            && self
                .item
                .as_ref()
                .is_none_or(|predicate| item_predicate_matches(predicate, item))
            && self.levels.matches(levels)
    }
}

impl CriterionTriggerInstance for EnchantedItemTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.ENCHANTED_ITEM`.
#[derive(Debug)]
pub struct EnchantedItemTrigger;

impl CriterionTrigger for EnchantedItemTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("enchanted_item")
    }
}

/// The static `minecraft:enchanted_item` trigger.
pub static ENCHANTED_ITEM: EnchantedItemTrigger = EnchantedItemTrigger;

/// The player view used by `type_specific/player` predicates.
pub type TriggerPlayerView<'a> = PlayerView<'a>;
