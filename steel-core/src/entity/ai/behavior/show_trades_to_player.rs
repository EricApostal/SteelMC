//! Vanilla `ShowTradesToPlayer`.

use steel_registry::equipment::EquipmentSlot;
use steel_registry::item_stack::ItemStack;
use steel_registry::vanilla_entities;
use steel_utils::types::InteractionHand;

use super::utils;
use super::villager;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;
use crate::entity::entities::mobs::npc::villager::VillagerEntity;
use crate::entity::npc::trading::{Merchant as _, MerchantOffer};
use crate::entity::{LivingEntity as _, Mob as _, SharedEntity};

/// Vanilla `ShowTradesToPlayer.MAX_LOOK_TIME`.
const MAX_LOOK_TIME: i32 = 900;
/// Vanilla `ShowTradesToPlayer.STARTING_LOOK_TIME`.
const STARTING_LOOK_TIME: i32 = 40;
/// Vanilla `ShowTradesToPlayer.displayCyclingItems` cycle length, `++cycleCounter >= 40`.
const CYCLE_INTERVAL: i32 = 40;
/// Vanilla `ShowTradesToPlayer.checkExtraStartConditions`, `distanceToSqr(...) <= 17.0`.
const MAX_TRADE_DISTANCE_SQR: f64 = 17.0;
/// Vanilla `ShowTradesToPlayer.clearHeldItem` restores `Villager`'s default
/// `MAINHAND` drop chance.
const DEFAULT_MAIN_HAND_DROP_CHANCE: f32 = 0.085;

/// Vanilla `ShowTradesToPlayer`: holds a trade offer's result item while the
/// player looks at the villager with a matching cost item in hand.
pub struct ShowTradesToPlayer {
    state: BehaviorState,
    player_item_stack: Option<ItemStack>,
    display_items: Vec<ItemStack>,
    cycle_counter: i32,
    display_index: i32,
    look_time: i32,
}

impl ShowTradesToPlayer {
    /// Vanilla `ShowTradesToPlayer(int minDuration, int maxDuration)`.
    #[must_use]
    pub fn new(min_duration: i32, max_duration: i32) -> Self {
        Self {
            state: BehaviorState::new(
                vec![(&mem::INTERACTION_TARGET, MemoryStatus::ValuePresent)],
                min_duration,
                max_duration,
            ),
            player_item_stack: None,
            display_items: Vec::new(),
            cycle_counter: 0,
            display_index: 0,
            look_time: 0,
        }
    }

    /// Vanilla `ShowTradesToPlayer.checkExtraStartConditions`.
    fn extra_start_conditions(ctx: &BehaviorContext<'_>) -> bool {
        let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) else {
            return false;
        };

        target.entity_type() == &vanilla_entities::PLAYER
            && ctx.body.is_alive()
            && target.is_alive()
            && !villager::is_baby_villager(ctx.body)
            && ctx.body.position().distance_squared(target.position()) <= MAX_TRADE_DISTANCE_SQR
    }

    /// Vanilla `ShowTradesToPlayer.findItemsToDisplay`.
    fn find_items_to_display(&mut self, player: &SharedEntity, villager: &VillagerEntity) {
        let current_player_item_stack = player
            .as_living_entity()
            .map_or_else(ItemStack::empty, |living| {
                living.get_item_in_hand(InteractionHand::MainHand)
            });

        let mut changed = false;
        let unchanged = self
            .player_item_stack
            .as_ref()
            .is_some_and(|previous| ItemStack::is_same_item(previous, &current_player_item_stack));
        if !unchanged {
            self.player_item_stack = Some(current_player_item_stack.clone());
            changed = true;
            self.display_items.clear();
        }

        if changed && !current_player_item_stack.is_empty() {
            self.update_display_items(villager);
            if !self.display_items.is_empty() {
                self.look_time = MAX_LOOK_TIME;
                if let Some(first) = self.display_items.first().cloned() {
                    display_as_held_item(villager, first);
                }
            }
        }
    }

    /// Vanilla `ShowTradesToPlayer.updateDisplayItems`.
    fn update_display_items(&mut self, villager: &VillagerEntity) {
        let Some(player_item_stack) = self.player_item_stack.clone() else {
            return;
        };

        // Vanilla `Villager.getOffers` builds the offer list on first use;
        // `Merchant::offers` does the same.
        for offer in villager.offers().iter() {
            if !offer.is_out_of_stock()
                && player_item_stack_matches_cost_of_offer(&player_item_stack, offer)
            {
                self.display_items.push(offer.assemble());
            }
        }
    }

    /// Vanilla `ShowTradesToPlayer.displayCyclingItems`.
    fn display_cycling_items(&mut self, villager: &VillagerEntity) {
        if self.display_items.len() < 2 {
            return;
        }

        self.cycle_counter += 1;
        if self.cycle_counter < CYCLE_INTERVAL {
            return;
        }

        self.display_index += 1;
        self.cycle_counter = 0;
        if self.display_index > self.display_items.len() as i32 - 1 {
            self.display_index = 0;
        }

        if let Some(item) = self.display_items.get(self.display_index as usize).cloned() {
            display_as_held_item(villager, item);
        }
    }
}

/// Vanilla `ShowTradesToPlayer.playerItemStackMatchesCostOfOffer`.
fn player_item_stack_matches_cost_of_offer(
    player_item_stack: &ItemStack,
    offer: &MerchantOffer,
) -> bool {
    ItemStack::is_same_item(player_item_stack, &offer.cost_a())
        || ItemStack::is_same_item(player_item_stack, &offer.cost_b())
}

/// Vanilla `ShowTradesToPlayer.clearHeldItem`.
fn clear_held_item(villager: &VillagerEntity) {
    set_held_item(villager, ItemStack::empty());
    villager.set_equipment_drop_chance(EquipmentSlot::MainHand, DEFAULT_MAIN_HAND_DROP_CHANCE);
}

/// Vanilla `ShowTradesToPlayer.displayAsHeldItem`.
fn display_as_held_item(villager: &VillagerEntity, item_stack: ItemStack) {
    set_held_item(villager, item_stack);
    villager.set_equipment_drop_chance(EquipmentSlot::MainHand, 0.0);
}

/// Vanilla `Villager.setItemSlot(EquipmentSlot.MAINHAND, stack)`.
fn set_held_item(villager: &VillagerEntity, item_stack: ItemStack) {
    villager.with_equipment_slot_mut(EquipmentSlot::MainHand, &mut |slot| {
        *slot = item_stack.clone();
    });
}

impl Behavior for ShowTradesToPlayer {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::extra_start_conditions(ctx)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        Self::extra_start_conditions(ctx)
            && self.look_time > 0
            && ctx.has_memory_value(&mem::INTERACTION_TARGET)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) {
            utils::look_at_entity(ctx, &target);
        }

        self.cycle_counter = 0;
        self.display_index = 0;
        self.look_time = STARTING_LOOK_TIME;
    }

    fn tick(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(target) = ctx.get_memory(&mem::INTERACTION_TARGET) else {
            return;
        };
        utils::look_at_entity(ctx, &target);

        let Some(villager) = villager::as_villager(ctx.body) else {
            return;
        };

        self.find_items_to_display(&target, villager);

        if self.display_items.is_empty() {
            clear_held_item(villager);
            self.look_time = self.look_time.min(STARTING_LOOK_TIME);
        } else {
            self.display_cycling_items(villager);
        }

        self.look_time -= 1;
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        ctx.erase_memory(&mem::INTERACTION_TARGET);

        if let Some(villager) = villager::as_villager(ctx.body) {
            clear_held_item(villager);
        }

        self.player_item_stack = None;
    }
}
