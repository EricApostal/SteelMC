//! Vanilla lever behavior.

use std::sync::Arc;

use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty, Direction};
use steel_registry::item_stack::ItemStack;
use steel_registry::{sound_events, vanilla_entities, vanilla_game_events, vanilla_game_rules};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::blocks::face_attached_horizontal_directional_block::FaceAttachedHorizontalDirectionalBlock;
use crate::behavior::blocks::redstone::{MAX_REDSTONE_SIGNAL, MIN_REDSTONE_SIGNAL};
use crate::behavior::{
    BlockBehavior, BlockHitResult, BlockPlaceContext, InteractionResult, InventoryAccess,
};
use crate::player::Player;
use crate::world::explosion::{BlockInteraction, ExplosionInfo, default_on_explosion_hit};
use crate::world::game_event::GameEventContext;
use crate::world::{LevelAccessor, LevelReader, ScheduledTickAccess, SignalQueryContext, World};

/// Vanilla `LeverBlock` source behavior.
#[block_behavior]
pub struct LeverBlock {
    face_attached: FaceAttachedHorizontalDirectionalBlock,
}

const POWERED: &BoolProperty = &BlockStateProperties::POWERED;

impl LeverBlock {
    /// Creates lever behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self {
            face_attached: FaceAttachedHorizontalDirectionalBlock::new(block),
        }
    }

    /// Vanilla `ServerExplosion.canTriggerBlocks`.
    ///
    /// `ServerExplosion::can_trigger_blocks` is private to `world::explosion`,
    /// so its wind-charge `mob_griefing` refinement is mirrored here.
    fn can_trigger_blocks(world: &World, explosion: &ExplosionInfo<'_>) -> bool {
        explosion.block_interaction == BlockInteraction::TriggerBlock
            && (!explosion.source.is_some_and(|source| {
                source.entity_type() == &vanilla_entities::BREEZE_WIND_CHARGE
            }) || world.get_game_rule(&vanilla_game_rules::MOB_GRIEFING))
    }

    fn update_neighbors(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let support_direction =
            FaceAttachedHorizontalDirectionalBlock::connected_direction(state).opposite();
        world.update_neighbors_at(pos, self.face_attached.block);
        world.update_neighbors_at(pos.relative(support_direction), self.face_attached.block);
    }

    fn pull(&self, state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let powered = !state.get_value(POWERED);
        let next_state = state.set_value(POWERED, powered);
        world.set_block(pos, next_state, UpdateFlags::UPDATE_ALL);
        self.update_neighbors(next_state, world, pos);
        Self::emit_transition_effects(world, pos, powered);
    }

    fn emit_transition_effects(level: &dyn LevelAccessor, pos: BlockPos, powered: bool) {
        level.play_block_sound(
            &sound_events::BLOCK_LEVER_CLICK,
            pos,
            0.3,
            if powered { 0.6 } else { 0.5 },
            None,
        );
        level.game_event(
            if powered {
                &vanilla_game_events::BLOCK_ACTIVATE
            } else {
                &vanilla_game_events::BLOCK_DEACTIVATE
            },
            pos,
            &GameEventContext::default(),
        );
    }
}

impl BlockBehavior for LeverBlock {
    fn can_survive(&self, state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        FaceAttachedHorizontalDirectionalBlock::can_survive(state, world, pos)
    }

    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        self.face_attached.state_for_placement(context)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        _neighbor_pos: BlockPos,
        _neighbor_state: BlockStateId,
    ) -> BlockStateId {
        FaceAttachedHorizontalDirectionalBlock::update_shape(state, world, pos, direction)
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _player: &Player,
        _hit_result: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        self.pull(state, world, pos);
        InteractionResult::Success
    }

    fn affect_neighbors_after_removal(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        moved_by_piston: bool,
    ) {
        if !moved_by_piston && state.get_value(POWERED) {
            self.update_neighbors(state, world, pos);
        }
    }

    fn is_signal_source(&self, _state: BlockStateId, _context: SignalQueryContext) -> bool {
        true
    }

    fn get_own_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        _context: SignalQueryContext,
    ) -> i32 {
        if state.get_value(POWERED) {
            MAX_REDSTONE_SIGNAL
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }

    fn get_direct_signal(
        &self,
        state: BlockStateId,
        _world: &dyn LevelReader,
        _pos: BlockPos,
        direction: Direction,
        _context: SignalQueryContext,
    ) -> i32 {
        if state.get_value(POWERED)
            && FaceAttachedHorizontalDirectionalBlock::connected_direction(state) == direction
        {
            MAX_REDSTONE_SIGNAL
        } else {
            MIN_REDSTONE_SIGNAL
        }
    }

    /// Vanilla `LeverBlock.onExplosionHit`: an explosion that can trigger blocks
    /// toggles the lever before the default explosion handling runs.
    fn on_explosion_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        explosion: &ExplosionInfo<'_>,
        on_drop: &mut dyn FnMut(ItemStack, BlockPos),
    ) {
        if Self::can_trigger_blocks(world, explosion) {
            self.pull(state, world, pos);
        }
        default_on_explosion_hit(state, world, pos, explosion, on_drop);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::blocks::properties::{AttachFace, EnumProperty};
    use steel_registry::init_vanilla_registry;
    use steel_registry::{
        sound_events, vanilla_blocks, vanilla_damage_types, vanilla_entities, vanilla_game_events,
    };
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::entity::SharedEntity;
    use crate::entity::damage::DamageSource;
    use crate::test_support::{TestEntity, TestLevel, fresh_test_world, insert_ready_full_chunk};

    const ATTACH_FACE: &EnumProperty<AttachFace> = &BlockStateProperties::ATTACH_FACE;
    const HORIZONTAL_FACING: &EnumProperty<Direction> = &BlockStateProperties::HORIZONTAL_FACING;

    fn lever_state(facing: Direction, face: AttachFace, powered: bool) -> BlockStateId {
        vanilla_blocks::LEVER
            .default_state()
            .set_value(HORIZONTAL_FACING, facing)
            .set_value(ATTACH_FACE, face)
            .set_value(POWERED, powered)
    }

    #[test]
    fn wall_lever_survives_only_with_its_backing_face() {
        init_vanilla_registry();
        let behavior = LeverBlock::new(&vanilla_blocks::LEVER);
        let pos = BlockPos::new(0, 64, 0);
        let state = lever_state(Direction::East, AttachFace::Wall, false);
        let supported =
            TestLevel::default().with_block(pos.west(), vanilla_blocks::STONE.default_state());

        assert!(behavior.can_survive(state, &supported, pos));
        assert!(!behavior.can_survive(state, &TestLevel::default(), pos));
    }

    #[test]
    fn powered_lever_strongly_powers_only_away_from_support() {
        init_vanilla_registry();
        let behavior = LeverBlock::new(&vanilla_blocks::LEVER);
        let state = lever_state(Direction::East, AttachFace::Wall, true);
        let level = TestLevel::default();
        let pos = BlockPos::new(0, 64, 0);

        assert_eq!(
            behavior.get_own_signal(state, &level, pos, SignalQueryContext::DEFAULT),
            15
        );
        assert_eq!(
            behavior.get_direct_signal(
                state,
                &level,
                pos,
                Direction::East,
                SignalQueryContext::DEFAULT,
            ),
            15
        );
        assert_eq!(
            behavior.get_direct_signal(
                state,
                &level,
                pos,
                Direction::West,
                SignalQueryContext::DEFAULT,
            ),
            0
        );
    }

    #[test]
    fn redstone_transition_side_effects_match_vanilla_for_lever() {
        init_vanilla_registry();
        let level = TestLevel::default();
        let pos = BlockPos::new(3, 64, -2);

        LeverBlock::emit_transition_effects(&level, pos, true);
        LeverBlock::emit_transition_effects(&level, pos, false);

        let sounds = level.block_sounds.borrow();
        assert_eq!(sounds.len(), 2);
        assert_eq!(sounds[0].sound, &sound_events::BLOCK_LEVER_CLICK);
        assert_eq!(sounds[0].pitch.to_bits(), 0.6_f32.to_bits());
        assert_eq!(sounds[0].exclude, None);
        assert_eq!(sounds[1].pitch.to_bits(), 0.5_f32.to_bits());
        assert_eq!(sounds[1].exclude, None);

        let events = level.game_events.borrow();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event, &vanilla_game_events::BLOCK_ACTIVATE);
        assert_eq!(events[0].source_entity_id, None);
        assert_eq!(events[1].event, &vanilla_game_events::BLOCK_DEACTIVATE);
        assert_eq!(events[1].source_entity_id, None);
    }

    fn trigger_explosion<'a>(
        source: &'a SharedEntity,
        damage_source: &'a DamageSource,
    ) -> ExplosionInfo<'a> {
        ExplosionInfo {
            center: DVec3::new(8.5, 64.5, 8.5),
            radius: 4.0,
            block_interaction: BlockInteraction::TriggerBlock,
            fire: false,
            source: Some(source),
            indirect_source: None,
            damage_source,
        }
    }

    #[test]
    fn trigger_block_explosion_toggles_lever_like_vanilla() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("lever_explosion_toggle");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let state = vanilla_blocks::LEVER.default_state();
        assert!(world.set_block(pos, state, UpdateFlags::UPDATE_NONE));

        let source: SharedEntity = TestEntity::shared(
            9_003,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
            &vanilla_entities::TNT,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::EXPLOSION);
        let explosion = trigger_explosion(&source, &damage_source);

        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::LEVER);
        let mut drops = Vec::new();
        behavior.on_explosion_hit(state, world, pos, &explosion, &mut |item, drop_pos| {
            drops.push((item, drop_pos));
        });

        // Vanilla `LeverBlock.pull` flips POWERED and keeps the block alive.
        assert!(world.get_block_state(pos).get_value(POWERED));
        assert!(drops.is_empty(), "trigger-block explosions keep the block");

        // A second triggering explosion flips the lever back.
        let powered_state = world.get_block_state(pos);
        behavior.on_explosion_hit(powered_state, world, pos, &explosion, &mut |_, _| {});
        assert!(!world.get_block_state(pos).get_value(POWERED));
    }

    #[test]
    fn wind_charge_without_mob_griefing_does_not_toggle_lever() {
        init_vanilla_registry();
        init_behaviors();
        let world_fixture = fresh_test_world("lever_explosion_wind_charge");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        let state = vanilla_blocks::LEVER.default_state();
        assert!(world.set_block(pos, state, UpdateFlags::UPDATE_NONE));

        let source: SharedEntity = TestEntity::shared(
            9_004,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
            &vanilla_entities::BREEZE_WIND_CHARGE,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::EXPLOSION);
        let explosion = trigger_explosion(&source, &damage_source);
        assert!(world.set_game_rule(&vanilla_game_rules::MOB_GRIEFING, false));

        BLOCK_BEHAVIORS
            .get_behavior(&vanilla_blocks::LEVER)
            .on_explosion_hit(state, world, pos, &explosion, &mut |_, _| {});

        assert!(!world.get_block_state(pos).get_value(POWERED));
    }
}
