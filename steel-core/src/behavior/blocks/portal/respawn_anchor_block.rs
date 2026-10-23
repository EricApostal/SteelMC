use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::properties::IntProperty;
use steel_registry::blocks::{
    BlockRef,
    block_state_ext::BlockStateExt,
    properties::{BlockStateProperties, Direction},
};
use steel_registry::fluid::{FluidState, is_water_fluid};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::item::BlockHitResult;
use steel_registry::{sound_events, vanilla_blocks, vanilla_game_events, vanilla_items};
use steel_utils::{
    BlockPos, BlockStateId,
    types::{InteractionHand, UpdateFlags},
};

use crate::entity::dismount_helper;
use crate::fluid::state::get_fluid_state;
use crate::{
    behavior::blocks::redstone::MAX_REDSTONE_SIGNAL,
    behavior::{BlockBehavior, BlockPlaceContext, InteractionResult, InventoryAccess},
    entity::Entity,
    entity::ai::path::PathComputationType,
    level_data::RespawnData,
    player::{Player, PlayerRespawnConfig},
    world::{
        LevelReader, World,
        explosion::{
            ExplosionDamageCalculator, ExplosionInfo, ExplosionInteraction,
            bad_respawn_point_damage_source, base_block_explosion_resistance,
        },
        game_event::GameEventContext,
    },
};

/// Vanilla respawn anchor.
///
/// Charging, spawn-point selection and the invalid-dimension explosion
/// (`RespawnAnchorBlock.explode`: block removal, water-sensitive resistance and
/// the `BAD_RESPAWN_POINT` damage source).
///
/// Deviation: vanilla `RespawnAnchorBlock.animateTick` emits a client-local ambient sound and
/// `REVERSE_PORTAL` particle; Steel lacks a client block-animation tick hook (owner:
/// `steel-core/src/world` block ticker pipeline).
/// Deviation: vanilla `RespawnAnchorBlock.canSetSpawn` reads
/// `EnvironmentAttributes.RESPAWN_ANCHOR_WORKS` per position; Steel lacks an environment-attribute
/// system and reads the dimension type's `respawn_anchor_works` flag instead (owner:
/// `steel-core/src/world` environment attributes).
///
/// Vanilla `RespawnAnchorBlock` does not override `isSignalSource`, `getSignal`, `getDirectSignal`
/// or `getOwnSignal`, so the `BlockBehavior` defaults (`false`/`0`) are used, as in vanilla.
#[block_behavior]
pub struct RespawnAnchorBlock {
    block: BlockRef,
}
const CHARGES: &IntProperty = &BlockStateProperties::RESPAWN_ANCHOR_CHARGES;
impl RespawnAnchorBlock {
    const MAX_CHARGES: u8 = 4;

    /// New respawn anchor behavior
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    #[must_use]
    pub(crate) fn can_use_for_respawn(
        world: &World,
        pos: BlockPos,
        state: BlockStateId,
        forced: bool,
    ) -> bool {
        state.get_block() == &vanilla_blocks::RESPAWN_ANCHOR
            && (forced || state.get_value(CHARGES) > 0)
            && Self::can_set_spawn(world, pos)
    }

    #[must_use]
    pub(crate) const fn can_set_spawn(world: &World, _pos: BlockPos) -> bool {
        world.dimension_type.respawn_anchor_works
    }

    pub(crate) fn find_standup_position(
        world: &Arc<World>,
        entity: &dyn Entity,
        pos: BlockPos,
    ) -> Option<DVec3> {
        for (candidate, check_dangerous) in Self::standup_search_candidates(pos) {
            if let Some(position) = dismount_helper::find_safe_dismount_location(
                world,
                entity,
                candidate,
                check_dangerous,
            ) {
                return Some(position);
            }
        }

        None
    }

    fn state_after_charge_consumed(state: BlockStateId) -> Option<BlockStateId> {
        if state.get_block() != &vanilla_blocks::RESPAWN_ANCHOR {
            return None;
        }
        let charges = state.get_value(CHARGES);
        (charges > 0).then(|| state.set_value(CHARGES, charges - 1))
    }

    #[must_use]
    pub(crate) fn consume_charge(world: &Arc<World>, pos: BlockPos, state: BlockStateId) -> bool {
        if world.get_block_state(pos) != state {
            return false;
        }
        let Some(state) = Self::state_after_charge_consumed(state) else {
            return false;
        };
        world.set_block(pos, state, UpdateFlags::UPDATE_ALL)
    }

    #[must_use]
    pub(crate) const fn should_consume_charge_after_respawn(
        forced: bool,
        consume_spawn_block: bool,
        found_standup_position: bool,
    ) -> bool {
        !forced && consume_spawn_block && found_standup_position
    }

    /// Vanilla `RespawnAnchorBlock.isWaterThatWouldFlow`.
    fn is_water_that_would_flow(world: &Arc<World>, pos: BlockPos) -> bool {
        let fluid = get_fluid_state(world, pos);
        if !is_water_fluid(fluid.fluid_id) {
            return false;
        }
        if fluid.is_source() {
            return true;
        }
        if fluid.amount < 2 {
            return false;
        }
        let below = get_fluid_state(world, pos.below());
        !is_water_fluid(below.fluid_id)
    }

    /// Vanilla `RespawnAnchorBlock.explode`: removes the anchor, then detonates a
    /// bad-respawn-point blast whose resistance at the anchor's own position is
    /// water's when the anchor is flooded.
    fn explode(world: &Arc<World>, pos: BlockPos) {
        // Vanilla `Level.removeBlock(pos, false)`.
        world.remove_block(pos, false);

        let any_water_neighbors = Direction::HORIZONTAL
            .into_iter()
            .any(|direction| Self::is_water_that_would_flow(world, pos.relative(direction)));
        let in_water =
            any_water_neighbors || is_water_fluid(get_fluid_state(world, pos.above()).fluid_id);

        let (boom_x, boom_y, boom_z) = pos.get_center();
        let boom_pos = DVec3::new(boom_x, boom_y, boom_z);
        let calculator = AnchorExplosionDamageCalculator { pos, in_water };
        world.explode_full(
            None,
            Some(bad_respawn_point_damage_source(boom_pos)),
            Some(&calculator),
            boom_x,
            boom_y,
            boom_z,
            5.0,
            true,
            ExplosionInteraction::Block,
        );
    }

    fn standup_candidates(pos: BlockPos) -> impl Iterator<Item = BlockPos> {
        [pos, pos.below(), pos.above()]
            .into_iter()
            .flat_map(Self::horizontal_standup_candidates)
            .chain([pos.above()])
    }

    fn standup_search_candidates(pos: BlockPos) -> impl Iterator<Item = (BlockPos, bool)> {
        [true, false].into_iter().flat_map(move |check_dangerous| {
            Self::standup_candidates(pos).map(move |candidate| (candidate, check_dangerous))
        })
    }

    const fn horizontal_standup_candidates(pos: BlockPos) -> [BlockPos; 8] {
        [
            pos.north(),
            pos.west(),
            pos.south(),
            pos.east(),
            pos.north().west(),
            pos.north().east(),
            pos.south().west(),
            pos.south().east(),
        ]
    }

    fn has_charge(state: BlockStateId) -> bool {
        state.get_value(CHARGES) > 0
    }

    fn can_be_charged(state: BlockStateId) -> bool {
        state.get_value(CHARGES) < Self::MAX_CHARGES
    }

    fn is_respawn_fuel(item_stack: &ItemStack) -> bool {
        item_stack.is(&vanilla_items::GLOWSTONE)
    }

    fn analog_output_signal(charges: u8) -> i32 {
        i32::from(charges) * MAX_REDSTONE_SIGNAL / i32::from(Self::MAX_CHARGES)
    }

    fn player_offhand_has_respawn_fuel(player: &Player) -> bool {
        player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::OffHand)
            .is(&vanilla_items::GLOWSTONE)
    }

    fn charge(source: Option<&dyn Entity>, world: &Arc<World>, pos: BlockPos, state: BlockStateId) {
        let charges = state.get_value(CHARGES);
        let charged_state = state.set_value(CHARGES, charges + 1);
        world.set_block(pos, charged_state, UpdateFlags::UPDATE_ALL);
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(source, Some(charged_state)),
        );
        world.play_sound(
            &sound_events::BLOCK_RESPAWN_ANCHOR_CHARGE,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
    }
}

/// Vanilla `RespawnAnchorBlock.explode`'s anonymous `ExplosionDamageCalculator`:
/// the anchor's own position resists like water when it is flooded, so the blast is
/// damped exactly like vanilla.
struct AnchorExplosionDamageCalculator {
    /// The anchor position being removed.
    pos: BlockPos,
    /// Whether the anchor counts as submerged (water neighbour or water above).
    in_water: bool,
}

impl ExplosionDamageCalculator for AnchorExplosionDamageCalculator {
    fn block_explosion_resistance(
        &self,
        _explosion: &ExplosionInfo<'_>,
        _world: &World,
        test_pos: BlockPos,
        state: BlockStateId,
        fluid: FluidState,
    ) -> Option<f32> {
        if test_pos == self.pos && self.in_water {
            return Some(vanilla_blocks::WATER.config.explosion_resistance);
        }
        base_block_explosion_resistance(state, fluid)
    }
}

impl BlockBehavior for RespawnAnchorBlock {
    /// Vanilla `RespawnAnchorBlock`'s default state: charge 0.
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state())
    }

    /// Vanilla `RespawnAnchorBlock.useItemOn`: glowstone charges an uncharged anchor, and a
    /// glowstone off-hand defers the interaction to the empty-hand path.
    fn use_item_on(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let is_respawn_fuel = inv.with_item(|item_stack| Self::is_respawn_fuel(item_stack));
        if is_respawn_fuel && Self::can_be_charged(state) {
            Self::charge(Some(player), world, pos, state);
            let has_infinite_materials = player.has_infinite_materials();
            inv.with_item(|item_stack| {
                item_stack.consume_one(has_infinite_materials);
            });
            return InteractionResult::Success;
        }

        if hand == InteractionHand::MainHand
            && Self::can_be_charged(state)
            && Self::player_offhand_has_respawn_fuel(player)
        {
            return InteractionResult::Pass;
        }

        InteractionResult::TryEmptyHandInteraction
    }

    /// Vanilla `RespawnAnchorBlock.useWithoutItem`: sets the respawn point, or detonates when the
    /// dimension rejects anchors.
    #[expect(
        clippy::float_cmp,
        reason = "vanilla `ServerPlayer.RespawnConfig.isSamePosition` compares yaw and pitch exactly"
    )]
    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        if !Self::has_charge(state) {
            return InteractionResult::Pass;
        }

        if !Self::can_set_spawn(world, pos) {
            // Vanilla `RespawnAnchorBlock.useWithoutItem`: an anchor that cannot set
            // spawn in this dimension detonates instead of charging a respawn point.
            Self::explode(world, pos);
            return InteractionResult::SuccessServer;
        }

        let respawn_config = PlayerRespawnConfig {
            respawn_data: RespawnData::of(world.key.clone(), pos, 0.0, 0.0),
            forced: false,
        };
        // Vanilla `ServerPlayer.RespawnConfig.isSamePosition`: dimension, position, yaw and
        // pitch must all match the anchor's 0/0 angles.
        let should_update_spawn = player.respawn_config().as_ref().is_none_or(|current| {
            current.respawn_data.global_pos != respawn_config.respawn_data.global_pos
                || current.respawn_data.yaw != respawn_config.respawn_data.yaw
                || current.respawn_data.pitch != respawn_config.respawn_data.pitch
        });

        if !should_update_spawn {
            return InteractionResult::Consume;
        }

        player.set_respawn_position(Some(respawn_config), true);
        world.play_sound(
            &sound_events::BLOCK_RESPAWN_ANCHOR_SET_SPAWN,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
        InteractionResult::SuccessServer
    }

    /// Vanilla `RespawnAnchorBlock.hasAnalogOutputSignal`: always true.
    fn has_analog_output_signal(&self, _state: BlockStateId) -> bool {
        true
    }

    /// Vanilla `RespawnAnchorBlock.getAnalogOutputSignal` / `getScaledChargeLevel(state, 15)`.
    fn get_analog_output_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _direction: Direction,
    ) -> i32 {
        Self::analog_output_signal(state.get_value(CHARGES))
    }

    /// Vanilla `RespawnAnchorBlock.isPathfindable`: never pathfindable.
    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::{init_vanilla_registry, vanilla_blocks};
    use steel_utils::BlockPos;

    #[test]
    fn standup_candidates_match_vanilla_order() {
        let origin = BlockPos::new(10, 64, -20);
        let candidates = RespawnAnchorBlock::standup_candidates(origin).collect::<Vec<_>>();

        assert_eq!(
            candidates,
            vec![
                origin.north(),
                origin.west(),
                origin.south(),
                origin.east(),
                origin.north().west(),
                origin.north().east(),
                origin.south().west(),
                origin.south().east(),
                origin.below().north(),
                origin.below().west(),
                origin.below().south(),
                origin.below().east(),
                origin.below().north().west(),
                origin.below().north().east(),
                origin.below().south().west(),
                origin.below().south().east(),
                origin.above().north(),
                origin.above().west(),
                origin.above().south(),
                origin.above().east(),
                origin.above().north().west(),
                origin.above().north().east(),
                origin.above().south().west(),
                origin.above().south().east(),
                origin.above(),
            ]
        );
    }

    #[test]
    fn charge_helpers_match_vanilla_bounds() {
        init_vanilla_registry();

        let empty = vanilla_blocks::RESPAWN_ANCHOR.default_state();
        let partial = empty.set_value(CHARGES, 1);
        let full = empty.set_value(CHARGES, RespawnAnchorBlock::MAX_CHARGES);

        assert!(!RespawnAnchorBlock::has_charge(empty));
        assert!(RespawnAnchorBlock::can_be_charged(empty));
        assert!(RespawnAnchorBlock::has_charge(partial));
        assert!(RespawnAnchorBlock::can_be_charged(partial));
        assert!(RespawnAnchorBlock::has_charge(full));
        assert!(!RespawnAnchorBlock::can_be_charged(full));
    }

    #[test]
    fn charge_consume_after_respawn_matches_vanilla_condition() {
        assert!(RespawnAnchorBlock::should_consume_charge_after_respawn(
            false, true, true,
        ));
        assert!(!RespawnAnchorBlock::should_consume_charge_after_respawn(
            true, true, true,
        ));
        assert!(!RespawnAnchorBlock::should_consume_charge_after_respawn(
            false, false, true,
        ));
        assert!(!RespawnAnchorBlock::should_consume_charge_after_respawn(
            false, true, false,
        ));
    }

    #[test]
    fn consumed_charge_state_decrements_exactly_once() {
        init_vanilla_registry();

        let charged = vanilla_blocks::RESPAWN_ANCHOR
            .default_state()
            .set_value(CHARGES, 2);
        let Some(depleted) = RespawnAnchorBlock::state_after_charge_consumed(charged) else {
            panic!("charged respawn anchor should produce a depleted state");
        };

        assert_eq!(depleted.get_value(CHARGES), 1);
        assert!(
            RespawnAnchorBlock::state_after_charge_consumed(
                vanilla_blocks::RESPAWN_ANCHOR.default_state()
            )
            .is_none()
        );
        assert!(
            RespawnAnchorBlock::state_after_charge_consumed(vanilla_blocks::STONE.default_state())
                .is_none()
        );
    }
}
