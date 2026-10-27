//! Block breaking state machine for players.
//!
//! This module implements the logic from Java's `ServerPlayerGameMode` for handling
//! block breaking, including progress tracking and validation.

use std::sync::Arc;

use steel_protocol::packets::game::CBlockUpdate;
use steel_registry::attribute::AttributeRef;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::equipment::EquipmentSlot;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_attributes;
use steel_registry::{
    REGISTRY, blocks::properties::Direction, item_stack::ItemStack, level_events, vanilla_blocks,
    vanilla_game_events, vanilla_mob_effects,
};
use steel_utils::{
    BlockPos, BlockStateId,
    types::{GameType, InteractionHand, UpdateFlags},
};

use crate::behavior::{BLOCK_BEHAVIORS, BlockLootContext};
use crate::block_entity::BlockEntity;
use crate::entity::{Entity, LivingEntity};
use crate::fluid::fluid_state_to_block;
use crate::player::Player;
use crate::player::food_data::food_constants;
use crate::world::{ConditionalBlockSetResult, World, game_event::GameEventContext};

use super::adventure_mode;

impl Player {
    /// Vanilla `Player.getDestroySpeed`.
    ///
    /// Applies the tool speed and multipliers in the exact vanilla order:
    /// selected-item speed, `MINING_EFFICIENCY` (only when the tool speed is
    /// above 1.0), dig-speed effects (Haste / Conduit Power take the maximum
    /// amplifier), Mining Fatigue, `BLOCK_BREAK_SPEED`, the eye-in-water
    /// `SUBMERGED_MINING_SPEED` factor, and finally the airborne `/ 5.0`.
    pub fn get_destroy_speed(&self, block_state: BlockStateId) -> f32 {
        // Take the held item's destroy speed in a scoped inventory borrow so
        // the attribute and mob-effect guards below never nest inside it.
        let mut speed = {
            let inventory = self.inventory.lock();
            inventory
                .get_item_in_hand(InteractionHand::MainHand)
                .get_destroy_speed(block_state)
        };

        if speed > 1.0 {
            speed += attribute_value(self, vanilla_attributes::MINING_EFFICIENCY);
        }

        if has_dig_speed(self) {
            speed *= 1.0 + (dig_speed_amplification(self) + 1) as f32 * 0.2;
        }

        if let Some(mining_fatigue) = self.mob_effect(vanilla_mob_effects::MINING_FATIGUE) {
            speed *= 0.3f64.powf(f64::from(mining_fatigue.amplifier() + 1)) as f32;
        }

        speed *= attribute_value(self, vanilla_attributes::BLOCK_BREAK_SPEED);

        // Aqua Affinity is intentionally not special-cased: vanilla
        // `getDestroySpeed` has no such branch.
        if self.is_eye_in_water() {
            speed *= attribute_value(self, vanilla_attributes::SUBMERGED_MINING_SPEED);
        }

        if !self.on_ground() {
            speed /= 5.0;
        }

        speed
    }

    /// Mirrors vanilla `Player.blockActionRestricted` for block breaking.
    pub(super) fn block_action_restricted(&self, world: &World, pos: BlockPos) -> bool {
        let game_mode = self.game_mode();
        if !matches!(game_mode, GameType::Adventure | GameType::Spectator) {
            return false;
        }
        if game_mode == GameType::Spectator {
            return true;
        }
        if self.abilities.lock().may_build {
            return false;
        }

        let item = {
            let inventory = self.inventory.lock();
            let item = inventory.get_selected_item();
            if item.is_empty() {
                return true;
            }
            item.clone()
        };
        !adventure_mode::can_break(&item, world, pos)
    }
}

/// Reads a living-attribute value, falling back to the vanilla registry
/// default when the entity map does not carry the instance.
///
/// Vanilla's `getAttributeValue` assumes the attribute exists on the player
/// (all of these are registered in `Player.createAttributes`); Steel guards
/// the lookup so a reordered `default_attributes` list degrades to the same
/// vanilla default instead of panicking.
fn attribute_value(player: &Player, attribute: AttributeRef) -> f32 {
    player
        .attributes()
        .lock()
        .get_value(attribute)
        .unwrap_or(attribute.default_value) as f32
}

/// Vanilla `MobEffectUtil.hasDigSpeed`: Haste or Conduit Power is active.
fn has_dig_speed(player: &Player) -> bool {
    player.has_mob_effect(vanilla_mob_effects::HASTE)
        || player.has_mob_effect(vanilla_mob_effects::CONDUIT_POWER)
}

/// Vanilla `MobEffectUtil.getDigSpeedAmplification`: the maximum of the
/// active Haste and Conduit Power amplifiers, defaulting to 0 each.
fn dig_speed_amplification(player: &Player) -> i32 {
    let haste = player
        .mob_effect(vanilla_mob_effects::HASTE)
        .map_or(0, |effect| effect.amplifier());
    let conduit_power = player
        .mob_effect(vanilla_mob_effects::CONDUIT_POWER)
        .map_or(0, |effect| effect.amplifier());
    haste.max(conduit_power)
}

/// Manages the block breaking state for a player.
///
/// Based on Java's `ServerPlayerGameMode` fields and logic.
pub struct BlockBreakingManager {
    /// Whether the player is currently breaking a block.
    is_destroying_block: bool,
    /// The tick when destruction started.
    destroy_progress_start: u64,
    /// The position of the block being destroyed.
    destroy_pos: BlockPos,
    /// The face being mined, used as the data value for the destroy-progress level event.
    destroy_direction: Direction,
    /// The current game tick counter.
    game_ticks: u64,
    /// Whether there's a delayed destroy pending (for slow mining).
    has_delayed_destroy: bool,
    /// Position of the delayed destroy.
    delayed_destroy_pos: BlockPos,
    /// The tick when delayed destroy started.
    delayed_tick_start: u64,
    /// The last sent destruction progress state (0-9, or -1 for none).
    last_sent_state: i32,
}

impl Default for BlockBreakingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl BlockBreakingManager {
    /// Creates a new block breaking manager.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            is_destroying_block: false,
            destroy_progress_start: 0,
            destroy_pos: BlockPos::new(0, 0, 0),
            destroy_direction: Direction::Down,
            game_ticks: 0,
            has_delayed_destroy: false,
            delayed_destroy_pos: BlockPos::new(0, 0, 0),
            delayed_tick_start: 0,
            last_sent_state: -1,
        }
    }

    /// Ticks the block breaking manager.
    ///
    /// This handles delayed destruction and updates break progress.
    pub fn tick(&mut self, player: &Player, world: &Arc<World>) {
        self.game_ticks += 1;

        if self.has_delayed_destroy {
            let state = world.get_block_state(self.delayed_destroy_pos);
            if is_air(state) {
                self.has_delayed_destroy = false;
            } else {
                let progress = self.increment_destroy_progress(
                    player,
                    world,
                    state,
                    self.delayed_destroy_pos,
                    self.delayed_tick_start,
                );
                if progress >= 1.0 {
                    self.has_delayed_destroy = false;
                    self.destroy_block(player, world, self.delayed_destroy_pos);
                }
            }
        } else if self.is_destroying_block {
            let state = world.get_block_state(self.destroy_pos);
            if is_air(state) {
                // Block was broken by something else
                world.broadcast_block_destruction(player.id(), self.destroy_pos, -1);
                self.last_sent_state = -1;
                self.is_destroying_block = false;
            } else {
                let ticks_spent_destroying =
                    self.game_ticks.saturating_sub(self.destroy_progress_start);
                let event = if ticks_spent_destroying.is_multiple_of(4) {
                    level_events::PARTICLES_AND_SOUND_DESTROY_PROGRESS
                } else {
                    level_events::PARTICLES_DESTROY_PROGRESS
                };

                self.increment_destroy_progress(
                    player,
                    world,
                    state,
                    self.destroy_pos,
                    self.destroy_progress_start,
                );
                world.level_event(
                    event,
                    self.destroy_pos,
                    self.destroy_direction.get_3d_data_value(),
                    None,
                );
            }
        }
    }

    /// Calculates and updates destruction progress, broadcasting to clients.
    fn increment_destroy_progress(
        &mut self,
        player: &Player,
        world: &Arc<World>,
        block_state: BlockStateId,
        pos: BlockPos,
        destroy_start_tick: u64,
    ) -> f32 {
        let ticks_spent = self.game_ticks.saturating_sub(destroy_start_tick);
        let destroy_speed = get_destroy_progress(player, block_state);
        let progress = destroy_speed * (ticks_spent + 1) as f32;
        let state = (progress * 10.0) as i32;

        if state != self.last_sent_state {
            world.broadcast_block_destruction(player.id(), pos, state);
            self.last_sent_state = state;
        }

        progress
    }

    /// Handles a block break action from the client.
    ///
    /// Note: The caller (packet handler) is responsible for calling `ack_block_changes_up_to`
    /// after this method returns, matching vanilla behavior.
    pub fn handle_block_break_action(
        &mut self,
        player: &Player,
        world: &Arc<World>,
        pos: BlockPos,
        action: BlockBreakAction,
        direction: Direction,
    ) {
        // Validate interaction range
        if !player.is_within_block_interaction_range(pos) {
            if self.is_destroying_block && action == BlockBreakAction::Abort {
                self.abort_destroy_block(player, world, pos);
            }
            return;
        }

        // Validate Y coordinate
        if pos.y() >= world.max_build_height() {
            player.send_packet(CBlockUpdate {
                pos,
                block_state: world.get_block_state(pos),
            });
            return;
        }

        match action {
            BlockBreakAction::ChangeDirection => {
                self.destroy_direction = direction;
            }

            BlockBreakAction::Start => {
                // Check may_interact permission
                if !world.may_interact(player, pos) {
                    player.send_packet(CBlockUpdate {
                        pos,
                        block_state: world.get_block_state(pos),
                    });
                    return;
                }

                // Creative instant break is driven by the instabuild ability in
                // vanilla `ServerPlayerGameMode.handleBlockBreakAction`.
                if player.get_abilities().instabuild {
                    self.destroy_and_ack(player, world, pos);
                    return;
                }

                if player.block_action_restricted(world, pos) {
                    player.send_packet(CBlockUpdate {
                        pos,
                        block_state: world.get_block_state(pos),
                    });
                    return;
                }

                self.destroy_progress_start = self.game_ticks;
                let block_state = world.get_block_state(pos);

                if !is_air(block_state) {
                    // TODO: Call EnchantmentHelper.onHitBlock before blockState.attack.
                    BLOCK_BEHAVIORS
                        .get_behavior(block_state.get_block())
                        .attack(block_state, world, pos, player);

                    let progress = get_destroy_progress(player, block_state);

                    if progress >= 1.0 {
                        // Insta-mine
                        self.destroy_and_ack(player, world, pos);
                    } else {
                        // Start breaking
                        if self.is_destroying_block {
                            // Send block update for the old position to cancel client prediction
                            player.send_packet(CBlockUpdate {
                                pos: self.destroy_pos,
                                block_state: world.get_block_state(self.destroy_pos),
                            });
                        }

                        self.is_destroying_block = true;
                        self.destroy_pos = pos;
                        self.destroy_direction = direction;
                        let state = (progress * 10.0) as i32;
                        world.broadcast_block_destruction(player.id(), pos, state);
                        self.last_sent_state = state;
                    }
                }
            }

            BlockBreakAction::Stop => {
                if pos == self.destroy_pos {
                    let ticks_spent = self.game_ticks.saturating_sub(self.destroy_progress_start);
                    let block_state = world.get_block_state(pos);

                    if !is_air(block_state) {
                        let destroy_speed = get_destroy_progress(player, block_state);
                        let progress = destroy_speed * (ticks_spent + 1) as f32;

                        if progress >= 0.7 {
                            // Complete the break
                            self.is_destroying_block = false;
                            world.broadcast_block_destruction(player.id(), pos, -1);
                            self.destroy_and_ack(player, world, pos);
                            return;
                        }

                        if !self.has_delayed_destroy {
                            // Set up delayed destroy
                            self.is_destroying_block = false;
                            self.has_delayed_destroy = true;
                            self.delayed_destroy_pos = pos;
                            self.delayed_tick_start = self.destroy_progress_start;
                        }
                    }
                }
            }

            BlockBreakAction::Abort => {
                self.abort_destroy_block(player, world, pos);
            }
        }
    }

    fn abort_destroy_block(&mut self, player: &Player, world: &Arc<World>, pos: BlockPos) {
        self.is_destroying_block = false;

        if self.destroy_pos != pos {
            log::warn!(
                "Mismatch in destroy block pos: {:?} vs {:?}",
                self.destroy_pos,
                pos
            );
            world.broadcast_block_destruction(player.id(), self.destroy_pos, -1);
        }

        world.broadcast_block_destruction(player.id(), pos, -1);
    }

    /// Destroys a block and sends appropriate response.
    fn destroy_and_ack(&mut self, player: &Player, world: &Arc<World>, pos: BlockPos) {
        if !self.destroy_block(player, world, pos) {
            // Send block update to resync client
            player.send_packet(CBlockUpdate {
                pos,
                block_state: world.get_block_state(pos),
            });
        }
    }

    /// Destroys a block at the given position.
    ///
    /// Returns true if the block was successfully destroyed.
    #[expect(
        clippy::unused_self,
        reason = "method belongs logically to BlockBreakingManager and will use self when additional state is added"
    )]
    fn destroy_block(&self, player: &Player, world: &Arc<World>, pos: BlockPos) -> bool {
        let state = world.get_block_state(pos);

        if player.get_abilities().instabuild && !held_item_can_destroy_in_creative(player) {
            return false;
        }

        // Get block info
        let Some(_block) = REGISTRY.blocks.by_state_id(state) else {
            return false;
        };

        // TODO: Check for GameMasterBlock (command blocks, etc.)

        if player.block_action_restricted(world, pos) {
            return false;
        }

        let behavior = BLOCK_BEHAVIORS.get_behavior(state.get_block());
        let adjusted_state = behavior.player_will_destroy(state, world, pos, player);
        world.game_event(
            &vanilla_game_events::BLOCK_DESTROY,
            pos,
            &GameEventContext::new(Some(player), Some(adjusted_state)),
        );
        let state_after_player_will_destroy = world.get_block_state(pos);

        // Vanilla parity: fluidState.createLegacyBlock() — breaking a waterlogged
        // block leaves water behind instead of air.
        let replacement = fluid_state_to_block(state.get_fluid_state());

        // Read before the removal below, so loot generation still sees the
        // block entity's contents.
        let block_entity = world.get_block_entity(pos);

        // Vanilla removes the live state after `playerWillDestroy`; tripwire uses
        // that callback to set DISARMED before the same block is removed.
        let removed_by_player_break = !state_after_player_will_destroy.is_air()
            && world.set_block_if_unchanged(
                pos,
                state_after_player_will_destroy,
                replacement,
                UpdateFlags::UPDATE_ALL,
            ) == ConditionalBlockSetResult::Changed;
        let changed_by_player_will_destroy = state_after_player_will_destroy != state;
        let changed = changed_by_player_will_destroy || removed_by_player_break;

        if removed_by_player_break {
            behavior.destroy(adjusted_state, world, pos);

            // Play block destruction particles and sound (skip for fire blocks like vanilla)
            // Exclude the breaking player as they see the effect client-side
            let block = REGISTRY.blocks.by_state_id(adjusted_state);
            let is_fire = block.is_some_and(|b| {
                b.key == vanilla_blocks::FIRE.key || b.key == vanilla_blocks::SOUL_FIRE.key
            });
            if !is_fire {
                world.destroy_block_effect(pos, u32::from(adjusted_state.0), Some(player.id()));
            }

            // Vanilla snapshots the tool before Item.mineBlock can damage or
            // consume it, then uses that snapshot for loot and post-break effects.
            let (has_correct_tool, destroyed_with) = {
                let inv = player.inventory.lock();
                let main_hand = inv.get_item_in_hand(InteractionHand::MainHand);
                (
                    main_hand.is_correct_tool_for_drops(adjusted_state)
                        || !requires_correct_tool(adjusted_state),
                    main_hand.copy_with_count(main_hand.count),
                )
            };

            // Damage the tool if the block has non-zero destroy time
            // This is done before playerDestroy, matching vanilla's Item.mineBlock
            let block_destroy_time = REGISTRY
                .blocks
                .by_state_id(adjusted_state)
                .map_or(0.0, |b| b.config.destroy_time);

            if block_destroy_time != 0.0 {
                let broke = {
                    let mut inv = player.inventory.lock();
                    let damage_per_block = inv.get_selected_item().get_tool_damage_per_block();

                    if damage_per_block > 0 {
                        // Use with_selected_item_mut to ensure set_changed() is called
                        // Skip damage if player has infinite materials (creative mode)
                        let has_infinite_materials = player.has_infinite_materials();
                        inv.with_selected_item_mut(|main_hand| {
                            let item = main_hand.item;
                            main_hand
                                .hurt_and_break(damage_per_block, has_infinite_materials)
                                .then_some(item)
                        })
                    } else {
                        None
                    }
                };

                if let Some(item) = broke {
                    player.on_equipped_item_broken(item, EquipmentSlot::MainHand);
                    log::debug!("Tool broke while mining block at {pos:?}");
                }
            }

            // Vanilla `ServerPlayerGameMode.destroyBlock` skips loot when
            // `preventsBlockDrops()` (instabuild) is set; spectators never reach
            // this path because `block_action_restricted` rejects the break.
            let game_mode = player.game_mode();
            if game_mode != GameType::Spectator
                && !player.prevents_block_drops()
                && has_correct_tool
            {
                player.award_stat(&vanilla_stat_types::BLOCK_MINED, state.get_block());
                player.cause_food_exhaustion(food_constants::EXHAUSTION_MINE);

                drop_block_loot(
                    player,
                    world,
                    pos,
                    adjusted_state,
                    &destroyed_with,
                    block_entity.as_deref(),
                );
                behavior.player_destroy(
                    world,
                    player,
                    pos,
                    adjusted_state,
                    block_entity.as_ref(),
                    &destroyed_with,
                );
            }
        }

        changed
    }
}

/// Block break action types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockBreakAction {
    /// Player started breaking a block.
    Start,
    /// Player redirected the block-break raycast without resetting progress.
    ChangeDirection,
    /// Player stopped breaking a block (finished or released).
    Stop,
    /// Player aborted breaking a block.
    Abort,
}

fn held_item_can_destroy_in_creative(player: &Player) -> bool {
    let inventory = player.inventory.lock();
    let main_hand = inventory.get_item_in_hand(InteractionHand::MainHand);
    main_hand.can_destroy_blocks_in_creative()
}

/// Checks if a block state is air.
fn is_air(state: BlockStateId) -> bool {
    let Some(block) = REGISTRY.blocks.by_state_id(state) else {
        return true;
    };
    block.config.is_air
}

/// Checks if a block requires the correct tool for drops.
fn requires_correct_tool(state: BlockStateId) -> bool {
    let Some(block) = REGISTRY.blocks.by_state_id(state) else {
        return false;
    };
    block.config.requires_correct_tool_for_drops
}

/// Gets the destroy progress per tick for a block.
///
/// This is based on the vanilla formula:
/// `1.0 / (destroy_time * 30.0)` for survival with correct tool
/// `1.0 / (destroy_time * 100.0)` for survival with wrong tool
/// Instant break for creative mode.
fn get_destroy_progress(player: &Player, block_state: BlockStateId) -> f32 {
    let Some(block) = REGISTRY.blocks.by_state_id(block_state) else {
        return 0.0;
    };

    let destroy_time = block.config.destroy_time;

    // Instant break follows the instabuild ability, matching the creative-destroy
    // branch of vanilla `ServerPlayerGameMode.handleBlockBreakAction`.
    if player.get_abilities().instabuild {
        return 1.0;
    }

    // Unbreakable block
    if destroy_time < 0.0 {
        return 0.0;
    }

    // Instant break for destroy_time == 0
    if destroy_time == 0.0 {
        return 1.0;
    }

    // Get player's mining speed, including tool, attributes and effects
    // (vanilla `Player.getDestroySpeed`).
    let speed = player.get_destroy_speed(block_state);

    // Check if player has the correct tool
    let has_correct_tool = {
        let inv = player.inventory.lock();
        let main_hand = inv.get_item_in_hand(InteractionHand::MainHand);
        main_hand.is_correct_tool_for_drops(block_state)
    };

    // Calculate destroy progress per tick
    // Vanilla formula: speed / hardness / (hasCorrectTool ? 30 : 100)
    let divisor = if has_correct_tool || !block.config.requires_correct_tool_for_drops {
        30.0
    } else {
        100.0
    };

    speed / destroy_time / divisor
}

/// Drops loot for a destroyed block using its loot table.
fn drop_block_loot(
    player: &Player,
    world: &Arc<World>,
    pos: BlockPos,
    state: BlockStateId,
    tool: &ItemStack,
    block_entity: Option<&dyn BlockEntity>,
) {
    let luck = player
        .attributes()
        .lock()
        .get_value(vanilla_attributes::LUCK)
        .unwrap_or(0.0) as f32;

    let drops = BlockLootContext::new(world, pos)
        .with_luck(luck)
        .with_tool(tool)
        .with_block_entity(block_entity)
        .get_drops(state);

    // Spawn each dropped item using the player's world reference (Arc<World>)
    for item in drops {
        if !item.is_empty() {
            player.get_world().pop_resource(pos, item);
        }
    }

    BLOCK_BEHAVIORS
        .get_behavior(state.get_block())
        .spawn_after_break(state, world, pos, tool, true);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use steel_registry::attribute::AttributeModifierOperation;
    use steel_registry::{
        vanilla_attributes, vanilla_blocks, vanilla_enchantments, vanilla_items,
        vanilla_mob_effects,
    };
    use steel_utils::Identifier;

    use super::*;
    use crate::bootstrap::init_globals;
    use crate::entity::EntityFluidContact;
    use crate::entity::attribute::AttributeModifier;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};

    /// Wooden pickaxe against stone: base speed 2.0 (> 1.0, so Efficiency applies).
    fn stone_state() -> BlockStateId {
        vanilla_blocks::STONE.default_state()
    }

    fn mining_player(world: &Arc<World>) -> Arc<Player> {
        let player = TestPlayerBuilder::new(Arc::clone(world), "DigSpeedTester", 1).build();
        // Neutral mining conditions: grounded, dry eyes, no effects.
        player.set_on_ground(true);
        player
            .base()
            .set_fluid_contact(EntityFluidContact::from_parts(0.0, 0.0, false, false));
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::WOODEN_PICKAXE));
        player
    }

    fn apply_mining_efficiency_bonus(player: &Player, bonus: f64) {
        player.attributes().lock().set_modifier(
            vanilla_attributes::MINING_EFFICIENCY,
            AttributeModifier {
                id: Identifier::vanilla_static("test_dig_speed"),
                amount: bonus,
                operation: AttributeModifierOperation::AddValue,
            },
            false,
        );
    }

    #[test]
    fn efficiency_levels_add_level_squared_plus_one() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_dig_speed");
        let player = mining_player(&fixture.world);

        let base = ItemStack::new(&vanilla_items::WOODEN_PICKAXE).get_destroy_speed(stone_state());
        assert!(base > 1.0);
        assert_relative(base, player.get_destroy_speed(stone_state()));

        // The game's Efficiency attribute effect must stay at level^2 + 1.
        let effects = vanilla_enchantments::EFFICIENCY.effects.attributes;
        let efficiency = effects
            .iter()
            .find(|effect| effect.attribute.key == vanilla_attributes::MINING_EFFICIENCY.key)
            .expect("efficiency should grant mining_efficiency");
        assert_eq!(efficiency.operation, AttributeModifierOperation::AddValue);
        for level in 1..=5 {
            let expected = (level * level + 1) as f32;
            assert_eq!(
                efficiency.amount.calculate(level),
                expected,
                "efficiency level {level}"
            );
        }

        // `get_destroy_speed` must add that bonus to the tool speed.
        for level in 1..=5 {
            apply_mining_efficiency_bonus(&player, f64::from(level * level + 1));
            assert_relative(
                base + (level * level + 1) as f32,
                player.get_destroy_speed(stone_state()),
            );
        }
    }

    #[test]
    fn haste_scales_by_amplifier() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_haste");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        player.set_mob_effect(vanilla_mob_effects::HASTE, 0);
        assert_relative(base * 1.2, player.get_destroy_speed(stone_state()));

        player.set_mob_effect(vanilla_mob_effects::HASTE, 1);
        assert_relative(base * 1.4, player.get_destroy_speed(stone_state()));
    }

    #[test]
    fn conduit_power_takes_max_amplifier() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_conduit");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        // Conduit Power alone behaves like Haste.
        player.set_mob_effect(vanilla_mob_effects::CONDUIT_POWER, 0);
        assert_relative(base * 1.2, player.get_destroy_speed(stone_state()));

        // The stronger of the two effects wins (max, not sum).
        player.set_mob_effect(vanilla_mob_effects::HASTE, 2);
        player.set_mob_effect(vanilla_mob_effects::CONDUIT_POWER, 0);
        assert_relative(base * 1.6, player.get_destroy_speed(stone_state()));
    }

    #[test]
    fn mining_fatigue_scales_by_power_of_three_tenths() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_fatigue");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        // Mining Fatigue I: *= 0.3.
        player.set_mob_effect(vanilla_mob_effects::MINING_FATIGUE, 0);
        assert_relative(base * 0.3, player.get_destroy_speed(stone_state()));

        // Mining Fatigue II: *= 0.09.
        player.set_mob_effect(vanilla_mob_effects::MINING_FATIGUE, 1);
        assert_relative(base * 0.09, player.get_destroy_speed(stone_state()));
    }

    #[test]
    fn submerged_scales_by_submerged_mining_speed() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_submerged");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        player
            .base()
            .set_fluid_contact(EntityFluidContact::from_parts(0.5, 0.0, true, false));
        let expected = base * attribute_value(&player, vanilla_attributes::SUBMERGED_MINING_SPEED);
        assert_relative(expected, player.get_destroy_speed(stone_state()));
        // The vanilla default for the eye-in-water factor is 0.2.
        assert_relative(base * 0.2, player.get_destroy_speed(stone_state()));
    }

    #[test]
    fn airborne_divides_by_five() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_airborne");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        player.set_on_ground(false);
        assert_relative(base / 5.0, player.get_destroy_speed(stone_state()));
    }

    #[test]
    fn all_penalties_compose_in_vanilla_order() {
        init_globals();
        let fixture = fresh_test_world("block_breaking_order");
        let player = mining_player(&fixture.world);
        let base = player.get_destroy_speed(stone_state());

        // Efficiency +2, Haste I x1.2, Fatigue II x0.09, block break x1,
        // submerged x0.2, airborne /5 — folded in vanilla order.
        apply_mining_efficiency_bonus(&player, 2.0);
        player.set_mob_effect(vanilla_mob_effects::HASTE, 0);
        player.set_mob_effect(vanilla_mob_effects::MINING_FATIGUE, 1);
        player
            .base()
            .set_fluid_contact(EntityFluidContact::from_parts(0.5, 0.0, true, false));
        player.set_on_ground(false);

        let expected = (base + 2.0) * 1.2 * 0.09 * 0.2 / 5.0;
        assert_relative(expected, player.get_destroy_speed(stone_state()));
    }

    /// Float comparison with room for the f64->f32 casts in the damage chain.
    fn assert_relative(expected: f32, actual: f32) {
        let tolerance = 1.0e-4 * expected.abs().max(1.0);
        assert!(
            (expected - actual).abs() <= tolerance,
            "expected {expected}, got {actual}",
        );
    }
}
