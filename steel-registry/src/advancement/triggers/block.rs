//! Block-state and location criteria.
//!
//! Covers `EnterBlockTrigger`, `SlideDownBlockTrigger`, `BeeNestDestroyedTrigger`,
//! `ItemUsedOnLocationTrigger` (`placed_block`, `item_used_on_block`,
//! `allay_drop_item_on_block`), `AnyBlockInteractionTrigger` and
//! `DefaultBlockInteractionTrigger`.

use crate::advancement::criterion::{CriterionTrigger, CriterionTriggerInstance};
use crate::advancement::predicates::{
    Condition, HolderSet, IntBounds, ItemPredicate, StatePropertiesPredicate,
    item_predicate_matches,
};
use crate::advancement::triggers::{PlayerCondition, TriggerEvent};
use steel_utils::{BlockStateId, Identifier};

/// Vanilla `enter_block`/`slide_down_block` matching: the block must belong to
/// the set and every state property must match.
pub(crate) fn matches_block_conditions(
    blocks: Option<&HolderSet>,
    state_predicate: Option<&StatePropertiesPredicate>,
    state: BlockStateId,
) -> bool {
    use crate::blocks::block_state_ext::BlockStateExt;
    if let Some(blocks) = blocks
        && !blocks.contains_block(&state.get_block().key)
    {
        return false;
    }
    state_predicate.is_none_or(|expected| expected.matches(state))
}

/// Instance of an `enter_block` criterion (`EnterBlockTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct EnterBlockTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `blocks` holder set.
    pub blocks: Option<HolderSet>,
    /// Vanilla `state`.
    pub state: Option<StatePropertiesPredicate>,
}

impl EnterBlockTriggerInstance {
    /// Vanilla `TriggerInstance.matches(state)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::EnterBlock { base, state } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && matches_block_conditions(self.blocks.as_ref(), self.state.as_ref(), state)
    }
}

impl CriterionTriggerInstance for EnterBlockTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.ENTER_BLOCK`.
#[derive(Debug)]
pub struct EnterBlockTrigger;

impl CriterionTrigger for EnterBlockTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("enter_block")
    }
}

/// The static `minecraft:enter_block` trigger.
pub static ENTER_BLOCK: EnterBlockTrigger = EnterBlockTrigger;

/// Instance of a `slide_down_block` criterion (`SlideDownBlockTrigger.TriggerInstance`).
#[derive(Debug, Default)]
pub struct SlideDownBlockTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `blocks` holder set.
    pub blocks: Option<HolderSet>,
    /// Vanilla `state`.
    pub state: Option<StatePropertiesPredicate>,
}

impl SlideDownBlockTriggerInstance {
    /// Vanilla `TriggerInstance.matches(state)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::SlideDownBlock { base, state } = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && matches_block_conditions(self.blocks.as_ref(), self.state.as_ref(), state)
    }
}

impl CriterionTriggerInstance for SlideDownBlockTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.HONEY_BLOCK_SLIDE`.
#[derive(Debug)]
pub struct SlideDownBlockTrigger;

impl CriterionTrigger for SlideDownBlockTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("slide_down_block")
    }
}

/// The static `minecraft:slide_down_block` trigger.
pub static SLIDE_DOWN_BLOCK: SlideDownBlockTrigger = SlideDownBlockTrigger;

/// Instance of a `bee_nest_destroyed` criterion.
#[derive(Debug, Default)]
pub struct BeeNestDestroyedTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `blocks` holder set.
    pub blocks: Option<HolderSet>,
    /// Vanilla `state`.
    pub state: Option<StatePropertiesPredicate>,
    /// Vanilla `item`: the tool used to destroy the nest.
    pub item: Option<ItemPredicate>,
    /// Vanilla `num_bees_inside`: bounds on the bees inside.
    pub num_bees_inside: IntBounds,
}

impl BeeNestDestroyedTriggerInstance {
    /// Vanilla `TriggerInstance.matches(state, itemStack, numBeesInside)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::BeeNestDestroyed {
            base,
            state,
            tool,
            bees_inside,
        } = *event
        else {
            return false;
        };
        if !self.player.matches(&base.context(Some(tool))) {
            return false;
        }
        matches_block_conditions(self.blocks.as_ref(), self.state.as_ref(), state)
            && self
                .item
                .as_ref()
                .is_none_or(|predicate| item_predicate_matches(predicate, tool))
            && self.num_bees_inside.matches(bees_inside)
    }
}

impl CriterionTriggerInstance for BeeNestDestroyedTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.BEE_NEST_DESTROYED`.
#[derive(Debug)]
pub struct BeeNestDestroyedTrigger;

impl CriterionTrigger for BeeNestDestroyedTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("bee_nest_destroyed")
    }
}

/// The static `minecraft:bee_nest_destroyed` trigger.
pub static BEE_NEST_DESTROYED: BeeNestDestroyedTrigger = BeeNestDestroyedTrigger;

/// Instance of a `placed_block`/`item_used_on_block`/`allay_drop_item_on_block` criterion.
///
/// Vanilla wraps the `location` condition around a location loot context holding
/// the origin, player, block state, block entity and tool; Steel exposes the same
/// facts through the event so the codegen can emit a plain `Condition` here.
#[derive(Debug, Default)]
pub struct ItemUsedOnBlockTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `location`.
    pub location: Option<Condition>,
}

impl ItemUsedOnBlockTriggerInstance {
    /// Vanilla `TriggerInstance.matches(locationContext)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let (base, tool) = match *event {
            TriggerEvent::ItemUsedOnBlock { ref base, tool } => (base, Some(tool)),
            TriggerEvent::PlacedBlock { ref base, tool } => (base, Some(tool)),
            TriggerEvent::AllayDropItemOnBlock { ref base, tool } => (base, Some(tool)),
            _ => return false,
        };
        self.player.matches(&base.context(tool))
            && self.location.as_ref().is_none_or(|condition| {
                condition.test(
                    &crate::advancement::predicates::AdvancementContext::for_player(
                        base.player,
                        base.level,
                        base.origin,
                        tool,
                    ),
                )
            })
    }
}

impl CriterionTriggerInstance for ItemUsedOnBlockTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.PLACED_BLOCK`/`ITEM_USED_ON_BLOCK`/`ALLAY_DROP_ITEM_ON_BLOCK`.
#[derive(Debug)]
pub struct ItemUsedOnBlockTrigger {
    id: &'static str,
}

impl ItemUsedOnBlockTrigger {
    /// Creates the trigger registered under `id`.
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self { id }
    }
}

impl CriterionTrigger for ItemUsedOnBlockTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static(self.id)
    }
}

/// The static `minecraft:placed_block` trigger.
pub static PLACED_BLOCK: ItemUsedOnBlockTrigger = ItemUsedOnBlockTrigger::new("placed_block");
/// The static `minecraft:item_used_on_block` trigger.
pub static ITEM_USED_ON_BLOCK: ItemUsedOnBlockTrigger =
    ItemUsedOnBlockTrigger::new("item_used_on_block");
/// The static `minecraft:allay_drop_item_on_block` trigger.
pub static ALLAY_DROP_ITEM_ON_BLOCK: ItemUsedOnBlockTrigger =
    ItemUsedOnBlockTrigger::new("allay_drop_item_on_block");

/// Instance of an `any_block_use`/`default_block_use` criterion.
#[derive(Debug, Default)]
pub struct BlockUseTriggerInstance {
    /// The shared `player` condition.
    pub player: PlayerCondition,
    /// Vanilla `location`.
    pub location: Option<Condition>,
}

impl BlockUseTriggerInstance {
    /// Vanilla `TriggerInstance.matches(locationContext)`.
    #[must_use]
    pub fn matches(&self, event: &TriggerEvent<'_>) -> bool {
        let TriggerEvent::BlockUse(base) = *event else {
            return false;
        };
        self.player.matches(&base.context(None))
            && self.location.as_ref().is_none_or(|condition| {
                condition.test(
                    &crate::advancement::predicates::AdvancementContext::for_player(
                        base.player,
                        base.level,
                        base.origin,
                        None,
                    ),
                )
            })
    }
}

impl CriterionTriggerInstance for BlockUseTriggerInstance {
    fn test(&self, event: &TriggerEvent<'_>) -> bool {
        self.matches(event)
    }
}

/// `CriteriaTriggers.ANY_BLOCK_USE`.
#[derive(Debug)]
pub struct AnyBlockInteractionTrigger;

impl CriterionTrigger for AnyBlockInteractionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("any_block_use")
    }
}

/// The static `minecraft:any_block_use` trigger.
pub static ANY_BLOCK_USE: AnyBlockInteractionTrigger = AnyBlockInteractionTrigger;

/// `CriteriaTriggers.DEFAULT_BLOCK_USE`.
#[derive(Debug)]
pub struct DefaultBlockInteractionTrigger;

impl CriterionTrigger for DefaultBlockInteractionTrigger {
    fn id(&self) -> Identifier {
        Identifier::vanilla_static("default_block_use")
    }
}

/// The static `minecraft:default_block_use` trigger.
pub static DEFAULT_BLOCK_USE: DefaultBlockInteractionTrigger = DefaultBlockInteractionTrigger;
