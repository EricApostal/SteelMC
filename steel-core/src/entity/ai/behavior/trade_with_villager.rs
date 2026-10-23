//! Vanilla `TradeWithVillager`.
//!
//! `figureOutWhatIAmWillingToTrade` reads `VillagerProfession.requestedItems`, which the
//! vendored extractor output does not carry (vanilla hard-codes it in
//! `VillagerProfession.bootstrap`). The build generator reads it from
//! `build_assets/villager_professions.json`, where only `farmer` declares a set
//! (`wheat, wheat_seeds, beetroot_seeds, bone_meal`) — every other profession has an
//! empty one, exactly as in vanilla, so the willing-to-trade set is non-empty only
//! between two villagers with different professions.

use steel_registry::data_components::vanilla_components::VILLAGER_FOOD;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_items;

use super::utils;
use super::villager;
use crate::entity::SharedEntity;
use crate::entity::ai::brain::behavior::{
    Behavior, BehaviorContext, BehaviorState, DEFAULT_DURATION,
};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::inventory::container::Container as _;

/// Vanilla `TradeWithVillager` interaction reach, `distanceToSqr(target) <= 5.0`.
const INTERACTION_DISTANCE_SQR: f64 = 5.0;
/// Vanilla `TradeWithVillager.start` walk speed.
const INTERACT_SPEED_MODIFIER: f32 = 0.5;
/// Vanilla `BehaviorUtils.lockGazeAndWalkToEachOther(..., 2)` close-enough distance.
const WALK_CLOSE_ENOUGH: i32 = 2;
/// Vanilla `throwHalfStack`'s `itemStack.getCount() > 24` retention.
const MIN_STACK_RETENTION: i32 = 24;
/// Vanilla `Items.WHEAT.getDefaultMaxStackSize() / 2`.
const WHEAT_HALF_STACK: i32 = 32;

/// Vanilla `TradeWithVillager`: two villagers facing each other exchange gossip,
/// excess food and (for farmers) surplus wheat.
pub struct TradeWithVillager {
    state: BehaviorState,
    trades: Vec<ItemRef>,
}

impl TradeWithVillager {
    /// Vanilla `TradeWithVillager()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::INTERACTION_TARGET, MemoryStatus::ValuePresent),
                    (
                        &mem::NEAREST_VISIBLE_LIVING_ENTITIES,
                        MemoryStatus::ValuePresent,
                    ),
                ],
                DEFAULT_DURATION,
                DEFAULT_DURATION,
            ),
            trades: Vec::new(),
        }
    }
}

impl Default for TradeWithVillager {
    fn default() -> Self {
        Self::new()
    }
}

/// Vanilla `BehaviorUtils.targetIsValid(brain, INTERACTION_TARGET, EntityTypes.VILLAGER)`.
fn target_is_valid(ctx: &BehaviorContext<'_>) -> bool {
    let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) else {
        return false;
    };

    target.entity_type() == &vanilla_entities::VILLAGER
        && target.is_alive()
        && utils::entity_is_visible(ctx, &target)
}

/// Vanilla `TradeWithVillager.figureOutWhatIAmWillingToTrade`.
fn figure_out_what_i_am_willing_to_trade(
    body: &VillagerEntity,
    target: &VillagerEntity,
) -> Vec<ItemRef> {
    let own_items = body.profession().requested_items;
    target
        .profession()
        .requested_items
        .iter()
        .copied()
        .filter(|item| !own_items.contains(item))
        .collect()
}

/// Vanilla `SimpleContainer.countItem`.
fn count_item(villager: &VillagerEntity, item: ItemRef) -> i32 {
    villager.with_inventory_mut(|inventory| {
        (0..inventory.size())
            .map(|slot| {
                let stack = inventory.get_item(slot);
                if stack.is(item) { stack.count() } else { 0 }
            })
            .sum()
    })
}

/// Vanilla `SimpleContainer.hasAnyOf`.
fn inventory_has_any_of(villager: &VillagerEntity, items: &[ItemRef]) -> bool {
    villager.with_inventory_mut(|inventory| {
        (0..inventory.size()).any(|slot| {
            let stack = inventory.get_item(slot);
            !stack.is_empty() && items.contains(&stack.item())
        })
    })
}

/// Vanilla `TradeWithVillager.throwHalfStack`.
fn throw_half_stack(
    thrower: &SharedEntity,
    villager: &VillagerEntity,
    predicate: impl Fn(&ItemStack) -> bool,
    target: &SharedEntity,
) {
    let thrown = villager.with_inventory_mut(|inventory| {
        for slot in 0..inventory.size() {
            let stack = inventory.get_item(slot);
            if stack.is_empty() || !predicate(stack) {
                continue;
            }

            let count = if stack.count() > stack.max_stack_size() / 2 {
                stack.count() / 2
            } else if stack.count() > MIN_STACK_RETENTION {
                stack.count() - MIN_STACK_RETENTION
            } else {
                continue;
            };

            return Some(inventory.remove_item(slot, count));
        }

        None
    });

    if let Some(thrown) = thrown {
        utils::throw_item(thrower, thrown, target.position());
    }
}

impl Behavior for TradeWithVillager {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        target_is_valid(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        target_is_valid(ctx)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) else {
            return;
        };

        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &target,
            INTERACT_SPEED_MODIFIER,
            WALK_CLOSE_ENOUGH,
        );

        let (Some(body), Some(target_villager)) = (
            villager::as_villager(ctx.body),
            villager::as_villager(&target),
        ) else {
            return;
        };

        self.trades = figure_out_what_i_am_willing_to_trade(body, target_villager);
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) else {
            return;
        };

        if ctx.body.position().distance_squared(target.position()) > INTERACTION_DISTANCE_SQR {
            return;
        }

        utils::lock_gaze_and_walk_to_each_other(
            ctx,
            &target,
            INTERACT_SPEED_MODIFIER,
            WALK_CLOSE_ENOUGH,
        );

        let (Some(body), Some(target_villager)) = (
            villager::as_villager(ctx.body),
            villager::as_villager(&target),
        ) else {
            return;
        };

        body.gossip(ctx.level, target_villager, ctx.timestamp);

        let is_farmer = body.profession().key.path == "farmer";
        if body.has_excess_food() && (is_farmer || target_villager.wants_more_food()) {
            throw_half_stack(
                ctx.body,
                body,
                |stack| stack.get(VILLAGER_FOOD).is_some(),
                &target,
            );
        }

        if is_farmer && count_item(body, &vanilla_items::WHEAT) > WHEAT_HALF_STACK {
            throw_half_stack(
                ctx.body,
                body,
                |stack| stack.is(&vanilla_items::WHEAT),
                &target,
            );
        }

        if !self.trades.is_empty() && inventory_has_any_of(body, &self.trades) {
            throw_half_stack(
                ctx.body,
                body,
                |stack| self.trades.contains(&stack.item()),
                &target,
            );
        }
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::INTERACTION_TARGET);
    }
}
