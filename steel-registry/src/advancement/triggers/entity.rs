//! Entity-related criteria.
//!
//! Covers `KilledTrigger`, `KilledByArrowTrigger`, `PlayerHurtEntityTrigger`,
//! `EntityHurtPlayerTrigger`, `BredAnimalsTrigger`, `CuredZombieVillagerTrigger`,
//! `TameAnimalTrigger`, `SummonedEntityTrigger`, `EffectsChangedTrigger`,
//! `ChanneledLightningTrigger`, `LightningStrikeTrigger`, `FishingRodHookedTrigger`,
//! `LevitationTrigger`, `ChangeDimensionTrigger` and `BrewedPotionTrigger`.

use crate::advancement::criterion::{CriterionTrigger, CriterionTriggerInstance};
use crate::advancement::predicates::{
    AdvancementContext, Condition, DamagePredicate, DamageSourcePredicate, DamageView, EntityView,
    IntBounds, ItemPredicate, MobEffectsPredicate, item_predicate_matches,
};
use crate::advancement::triggers::{EventBase, PlayerCondition, TriggerEvent};
use crate::item_stack::ItemStack;
use steel_utils::Identifier;

/// Testes every entry of `victims` against a distinct entity of `entities`
/// (vanilla's order-free multiset match).
fn matches_victims(
    conditions: &[Condition],
    entities: &[&EntityView<'_>],
    base: &EventBase<'_>,
) -> bool {
    let mut used = vec![false; entities.len()];
    for condition in conditions {
        let mut matched = false;
        for (index, entity) in entities.iter().enumerate() {
            if used[index] {
                continue;
            }
            if condition.test(&AdvancementContext::for_entity(
                entity,
                base.player,
                base.level,
                base.origin,
                None,
            )) {
                used[index] = true;
                matched = true;
                break;
            }
        }
        if !matched {
            return false;
        }
    }
    true
}

/// Instance of a `player_killed_entity`/`entity_killed_player`/
/// `kill_mob_near_sculk_catalyst` criterion (`KilledTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct KilledTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `entity`.
    pub entity: Option<Condition>,
    /// Vanilla `killing_blow`.
    pub killing_blow: Option<DamageSourcePredicate>,
}

impl KilledTriggerInstance {
    fn matches_inner(
        &self,
        base: &EventBase<'_>,
        entity: &EntityView<'_>,
        killing_blow: Option<DamageView<'_>>,
    ) -> bool {
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        let no_damage = empty_damage();
        let blow_view = killing_blow.unwrap_or(no_damage);
        if let Some(blow) = &self.killing_blow
            && !blow.matches(&blow_view)
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

    /// Vanilla `TriggerInstance.matches`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        match *event {
            TriggerEvent::PlayerKilledEntity {
                ref base,
                entity,
                killing_blow,
            }
            | TriggerEvent::EntityKilledPlayer {
                ref base,
                entity,
                killing_blow,
            }
            | TriggerEvent::KillMobNearSculkCatalyst {
                ref base,
                entity,
                killing_blow,
            } => self.matches_inner(base, entity, killing_blow),
            _ => false,
        }
    }
}

/// A damage view carrying no damage source, used when vanilla passes no damage.
fn empty_damage() -> DamageView<'static> {
    DamageView {
        original: 0.0,
        actual: 0.0,
        blocked: false,
        source_entity: None,
        direct_entity: None,
        type_key: None,
        is_direct: false,
    }
}

impl CriterionTriggerInstance for KilledTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.PLAYER_KILLED_ENTITY`/`ENTITY_KILLED_PLAYER`/`KILL_MOB_NEAR_SCULK_CATALYST`.
#[derive(Debug)]
pub struct KilledTrigger {
    id: &'static str,
}

impl KilledTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for KilledTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:player_killed_entity` trigger.
pub static PLAYER_KILLED_ENTITY: KilledTrigger = KilledTrigger::new("player_killed_entity");
/// The static `minecraft:entity_killed_player` trigger.
pub static ENTITY_KILLED_PLAYER: KilledTrigger = KilledTrigger::new("entity_killed_player");
/// The static `minecraft:kill_mob_near_sculk_catalyst` trigger.
pub static KILL_MOB_NEAR_SCULK_CATALYST: KilledTrigger =
    KilledTrigger::new("kill_mob_near_sculk_catalyst");

/// Instance of a `killed_by_arrow` criterion (`KilledByArrowTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct KilledByArrowTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `victims`.
    pub victims: &'static [Condition],
    /// Vanilla `unique_entity_types`: bounds on the distinct victim types.
    pub unique_entity_types: IntBounds,
    /// Vanilla `fired_from_weapon`.
    pub fired_from_weapon: Option<ItemPredicate>,
}

impl KilledByArrowTriggerInstance {
    /// Vanilla `TriggerInstance.matches(victims, weapon, uniqueEntityTypes)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::KilledByArrow {
            base,
            victims,
            weapon,
            unique_entity_types,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(weapon)) {
            return false;
        }
        if let Some(predicate) = &self.fired_from_weapon
            && !weapon.is_some_and(|weapon| item_predicate_matches(predicate, weapon))
        {
            return false;
        }
        if !self.unique_entity_types.matches(unique_entity_types) {
            return false;
        }
        matches_victims(self.victims, victims, &base)
    }
}

impl CriterionTriggerInstance for KilledByArrowTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.KILLED_BY_ARROW`.
#[derive(Debug)]
pub struct KilledByArrowTrigger;

impl CriterionTrigger for KilledByArrowTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("killed_by_arrow")
    }
}

/// The static `minecraft:killed_by_arrow` trigger.
pub static KILLED_BY_ARROW: KilledByArrowTrigger = KilledByArrowTrigger;

/// Instance of a `player_hurt_entity` criterion (`PlayerHurtEntityTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct PlayerHurtEntityTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `damage`.
    pub damage: Option<DamagePredicate>,
    /// Vanilla `entity`: the victim.
    pub entity: Option<Condition>,
}

impl PlayerHurtEntityTriggerInstance {
    /// Vanilla `TriggerInstance.matches(player, victim, source, original, actual, blocked)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::PlayerHurtEntity {
            base,
            damage,
            victim,
        } = *event
        else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self
                .damage
                .as_ref()
                .is_none_or(|predicate| predicate.matches(&damage))
            && self.entity.as_ref().is_none_or(|condition| {
                condition.test(&AdvancementContext::for_entity(
                    victim,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
    }
}

impl CriterionTriggerInstance for PlayerHurtEntityTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.PLAYER_HURT_ENTITY`.
#[derive(Debug)]
pub struct PlayerHurtEntityTrigger;

impl CriterionTrigger for PlayerHurtEntityTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("player_hurt_entity")
    }
}

/// The static `minecraft:player_hurt_entity` trigger.
pub static PLAYER_HURT_ENTITY: PlayerHurtEntityTrigger = PlayerHurtEntityTrigger;

/// Instance of an `entity_hurt_player` criterion (`EntityHurtPlayerTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct EntityHurtPlayerTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `damage`.
    pub damage: Option<DamagePredicate>,
}

impl EntityHurtPlayerTriggerInstance {
    /// Vanilla `TriggerInstance.matches(player, source, original, actual, blocked)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::EntityHurtPlayer { base, damage } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self
                .damage
                .as_ref()
                .is_none_or(|predicate| predicate.matches(&damage))
    }
}

impl CriterionTriggerInstance for EntityHurtPlayerTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.ENTITY_HURT_PLAYER`.
#[derive(Debug)]
pub struct EntityHurtPlayerTrigger;

impl CriterionTrigger for EntityHurtPlayerTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("entity_hurt_player")
    }
}

/// The static `minecraft:entity_hurt_player` trigger.
pub static ENTITY_HURT_PLAYER: EntityHurtPlayerTrigger = EntityHurtPlayerTrigger;

/// Instance of a `bred_animals` criterion (`BredAnimalsTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct BredAnimalsTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `parent`.
    pub parent: Option<Condition>,
    /// Vanilla `partner`.
    pub partner: Option<Condition>,
    /// Vanilla `child`.
    pub child: Option<Condition>,
}

impl BredAnimalsTriggerInstance {
    /// Vanilla `TriggerInstance.matches(parent, partner, child)`: the parent and
    /// partner conditions are order-free.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::BredAnimals {
            base,
            parent,
            partner,
            child,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if let Some(condition) = &self.child
            && !child.is_some_and(|child| {
                condition.test(&AdvancementContext::for_entity(
                    child,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        {
            return false;
        }
        let matches = |condition: &Condition, entity: &EntityView<'_>| {
            condition.test(&AdvancementContext::for_entity(
                entity,
                base.player,
                base.level,
                base.origin,
                None,
            ))
        };
        match (&self.parent, &self.partner) {
            (Some(parent_condition), Some(partner_condition)) => {
                (matches(parent_condition, parent) && matches(partner_condition, partner))
                    || (matches(parent_condition, partner) && matches(partner_condition, parent))
            }
            (Some(condition), None) => matches(condition, parent) || matches(condition, partner),
            (None, Some(condition)) => matches(condition, parent) || matches(condition, partner),
            (None, None) => true,
        }
    }
}

impl CriterionTriggerInstance for BredAnimalsTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.BRED_ANIMALS`.
#[derive(Debug)]
pub struct BredAnimalsTrigger;

impl CriterionTrigger for BredAnimalsTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("bred_animals")
    }
}

/// The static `minecraft:bred_animals` trigger.
pub static BRED_ANIMALS: BredAnimalsTrigger = BredAnimalsTrigger;

/// Instance of a `cured_zombie_villager` criterion.
#[derive(Debug, Default)]
pub struct CuredZombieVillagerTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `zombie`.
    pub zombie: Option<Condition>,
    /// Vanilla `villager`.
    pub villager: Option<Condition>,
}

impl CuredZombieVillagerTriggerInstance {
    /// Vanilla `TriggerInstance.matches(zombie, villager)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::CuredZombieVillager {
            base,
            zombie,
            villager,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        let matches = |condition: &Condition, entity: &EntityView<'_>| {
            condition.test(&AdvancementContext::for_entity(
                entity,
                base.player,
                base.level,
                base.origin,
                None,
            ))
        };
        self.zombie
            .as_ref()
            .is_none_or(|condition| matches(condition, zombie))
            && self
                .villager
                .as_ref()
                .is_none_or(|condition| matches(condition, villager))
    }
}

impl CriterionTriggerInstance for CuredZombieVillagerTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.CURED_ZOMBIE_VILLAGER`.
#[derive(Debug)]
pub struct CuredZombieVillagerTrigger;

impl CriterionTrigger for CuredZombieVillagerTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("cured_zombie_villager")
    }
}

/// The static `minecraft:cured_zombie_villager` trigger.
pub static CURED_ZOMBIE_VILLAGER: CuredZombieVillagerTrigger = CuredZombieVillagerTrigger;

/// Instance of a `tame_animal`/`summoned_entity` criterion.
#[derive(Debug, Default)]
pub struct EntityConditionTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `entity`.
    pub entity: Option<Condition>,
}

impl EntityConditionTriggerInstance {
    fn matches_inner(&self, base: &EventBase<'_>, entity: &EntityView<'_>) -> bool {
        self.player.matches(&base.context(None))
            && self.entity.as_ref().is_none_or(|condition| {
                condition.test(&AdvancementContext::for_entity(
                    entity,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
    }

    /// Vanilla `TriggerInstance.matches(entity)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        match *event {
            TriggerEvent::TameAnimal { ref base, entity } => self.matches_inner(base, entity),
            TriggerEvent::SummonedEntity { ref base, entity } => self.matches_inner(base, entity),
            TriggerEvent::LightningStrike { .. } | TriggerEvent::ChanneledLightning { .. } => false,
            _ => false,
        }
    }
}

impl CriterionTriggerInstance for EntityConditionTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// An entity-condition trigger singleton; one per `CriteriaTriggers` id.
#[derive(Debug)]
pub struct EntityConditionTrigger {
    id: &'static str,
}

impl EntityConditionTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for EntityConditionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:tame_animal` trigger.
pub static TAME_ANIMAL: EntityConditionTrigger = EntityConditionTrigger::new("tame_animal");
/// The static `minecraft:summoned_entity` trigger.
pub static SUMMONED_ENTITY: EntityConditionTrigger = EntityConditionTrigger::new("summoned_entity");

/// Instance of an `effects_changed` criterion.
#[derive(Debug, Default)]
pub struct EffectsChangedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `effects`.
    pub effects: Option<MobEffectsPredicate>,
    /// Vanilla `source`.
    pub source: Option<Condition>,
}

impl EffectsChangedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(player, source)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::EffectsChanged {
            base,
            effects,
            source,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if let Some(expected) = &self.effects
            && !expected.matches(effects)
        {
            return false;
        }
        self.source.as_ref().is_none_or(|condition| {
            source.is_some_and(|source| {
                condition.test(&AdvancementContext::for_entity(
                    source,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        })
    }
}

impl CriterionTriggerInstance for EffectsChangedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.EFFECTS_CHANGED`.
#[derive(Debug)]
pub struct EffectsChangedTrigger;

impl CriterionTrigger for EffectsChangedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("effects_changed")
    }
}

/// The static `minecraft:effects_changed` trigger.
pub static EFFECTS_CHANGED: EffectsChangedTrigger = EffectsChangedTrigger;

/// Instance of a `channeled_lightning` criterion.
#[derive(Debug, Default)]
pub struct ChanneledLightningTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `victims`.
    pub victims: &'static [Condition],
}

impl ChanneledLightningTriggerInstance {
    /// Vanilla `TriggerInstance.matches(victims)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::ChanneledLightning { base, victims } = *event else {
            return false;
        };
        self.player.matches(&base.context(None)) && matches_victims(self.victims, victims, &base)
    }
}

impl CriterionTriggerInstance for ChanneledLightningTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.CHANNELED_LIGHTNING`.
#[derive(Debug)]
pub struct ChanneledLightningTrigger;

impl CriterionTrigger for ChanneledLightningTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("channeled_lightning")
    }
}

/// The static `minecraft:channeled_lightning` trigger.
pub static CHANNELED_LIGHTNING: ChanneledLightningTrigger = ChanneledLightningTrigger;

/// Instance of a `lightning_strike` criterion.
#[derive(Debug, Default)]
pub struct LightningStrikeTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `lightning`.
    pub lightning: Option<Condition>,
    /// Vanilla `bystander`: matched against any nearby entity.
    pub bystander: Option<Condition>,
}

impl LightningStrikeTriggerInstance {
    /// Vanilla `TriggerInstance.matches(lightning, bystanders)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::LightningStrike {
            base,
            lightning,
            bystanders,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        if let Some(condition) = &self.lightning
            && !condition.test(&AdvancementContext::for_entity(
                lightning,
                base.player,
                base.level,
                base.origin,
                None,
            ))
        {
            return false;
        }
        self.bystander.as_ref().is_none_or(|condition| {
            bystanders.iter().any(|entity| {
                condition.test(&AdvancementContext::for_entity(
                    entity,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        })
    }
}

impl CriterionTriggerInstance for LightningStrikeTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.LIGHTNING_STRIKE`.
#[derive(Debug)]
pub struct LightningStrikeTrigger;

impl CriterionTrigger for LightningStrikeTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("lightning_strike")
    }
}

/// The static `minecraft:lightning_strike` trigger.
pub static LIGHTNING_STRIKE: LightningStrikeTrigger = LightningStrikeTrigger;

/// Instance of a `fishing_rod_hooked` criterion.
#[derive(Debug, Default)]
pub struct FishingRodHookedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `rod`.
    pub rod: Option<ItemPredicate>,
    /// Vanilla `entity`: the hooked entity or hook.
    pub entity: Option<Condition>,
    /// Vanilla `item`: the hooked item.
    pub item: Option<ItemPredicate>,
}

impl FishingRodHookedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(rod, hooked, items)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::FishingRodHooked {
            base,
            rod,
            hooked,
            items,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(Some(rod))) {
            return false;
        }
        if let Some(predicate) = &self.rod
            && !item_predicate_matches(predicate, rod)
        {
            return false;
        }
        if let Some(condition) = &self.entity
            && !hooked.is_some_and(|hooked| {
                condition.test(&AdvancementContext::for_entity(
                    hooked,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
        {
            return false;
        }
        self.item.as_ref().is_none_or(|predicate| {
            items
                .iter()
                .any(|item| item_predicate_matches(predicate, item))
                || hooked.is_some_and(|hooked| hooked.held_item_matches(predicate))
        })
    }
}

impl CriterionTriggerInstance for FishingRodHookedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.FISHING_ROD_HOOKED`.
#[derive(Debug)]
pub struct FishingRodHookedTrigger;

impl CriterionTrigger for FishingRodHookedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("fishing_rod_hooked")
    }
}

/// The static `minecraft:fishing_rod_hooked` trigger.
pub static FISHING_ROD_HOOKED: FishingRodHookedTrigger = FishingRodHookedTrigger;

/// Instance of a `levitation` criterion.
#[derive(Debug, Default)]
pub struct LevitationTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `distance`.
    pub distance: Option<crate::advancement::predicates::DistancePredicate>,
    /// Vanilla `duration`.
    pub duration: IntBounds,
}

impl LevitationTriggerInstance {
    /// Vanilla `TriggerInstance.matches(player, start, duration)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::Levitation {
            base,
            start,
            duration,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(None)) {
            return false;
        }
        self.distance
            .as_ref()
            .is_none_or(|distance| distance.matches(start, base.level.position()))
            && self.duration.matches(duration)
    }
}

impl CriterionTriggerInstance for LevitationTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.LEVITATION`.
#[derive(Debug)]
pub struct LevitationTrigger;

impl CriterionTrigger for LevitationTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("levitation")
    }
}

/// The static `minecraft:levitation` trigger.
pub static LEVITATION: LevitationTrigger = LevitationTrigger;

/// Instance of a `changed_dimension` criterion.
#[derive(Debug, Default)]
pub struct ChangedDimensionTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `from`.
    pub from: Option<Identifier>,
    /// Vanilla `to`.
    pub to: Option<Identifier>,
}

impl ChangedDimensionTriggerInstance {
    /// Vanilla `TriggerInstance.matches(from, to)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::ChangedDimension { base, from, to } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self.from.as_ref().is_none_or(|expected| expected == from)
            && self.to.as_ref().is_none_or(|expected| expected == to)
    }
}

impl CriterionTriggerInstance for ChangedDimensionTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.CHANGED_DIMENSION`.
#[derive(Debug)]
pub struct ChangedDimensionTrigger;

impl CriterionTrigger for ChangedDimensionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("changed_dimension")
    }
}

/// The static `minecraft:changed_dimension` trigger.
pub static CHANGED_DIMENSION: ChangedDimensionTrigger = ChangedDimensionTrigger;

/// Instance of a `brewed_potion` criterion.
#[derive(Debug, Default)]
pub struct BrewedPotionTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `potion`: the brewed potion, if the brewed contents name one.
    pub potion: Option<Identifier>,
}

impl BrewedPotionTriggerInstance {
    /// Vanilla `TriggerInstance.matches(potion)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::BrewedPotion { base, potion } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self
                .potion
                .as_ref()
                .is_none_or(|expected| potion == Some(expected))
    }
}

impl CriterionTriggerInstance for BrewedPotionTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.BREWED_POTION`.
#[derive(Debug)]
pub struct BrewedPotionTrigger;

impl CriterionTrigger for BrewedPotionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("brewed_potion")
    }
}

/// The static `minecraft:brewed_potion` trigger.
pub static BREWED_POTION: BrewedPotionTrigger = BrewedPotionTrigger;

/// Instance of a `target_hit` criterion (`TargetHitTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct TargetHitTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `signal_strength`: the emitted redstone signal strength.
    pub signal_strength: Option<i32>,
    /// Vanilla `projectile`.
    pub projectile: Option<Condition>,
}

impl TargetHitTriggerInstance {
    /// Vanilla `TriggerInstance.matches(projectile, signalStrength)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::TargetHit {
            base,
            projectile,
            signal_strength,
        } = *event
        else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self
                .signal_strength
                .is_none_or(|required| required == signal_strength)
            && self.projectile.as_ref().is_none_or(|condition| {
                condition.test(&AdvancementContext::for_entity(
                    projectile,
                    base.player,
                    base.level,
                    base.origin,
                    None,
                ))
            })
    }
}

impl CriterionTriggerInstance for TargetHitTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.TARGET_BLOCK_HIT`.
#[derive(Debug)]
pub struct TargetHitTrigger;

impl CriterionTrigger for TargetHitTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("target_hit")
    }
}

/// The static `minecraft:target_hit` trigger.
pub static TARGET_HIT: TargetHitTrigger = TargetHitTrigger;

/// Vanilla `ItemStack` conversion helper for triggers holding a tool stack.
pub(crate) type TriggerItemStack = ItemStack;
