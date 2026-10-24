//! Vanilla bell behavior.

use std::sync::{Arc, Weak};

use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::block_entity_type::BlockEntityTypeRef;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{
    BellAttachType, BlockStateProperties, BoolProperty, Direction, EnumProperty,
};
use steel_registry::blocks::shapes::SupportType;
use steel_registry::vanilla_custom_stats::BELL_RING;
use steel_registry::{
    item_stack::ItemStack, sound_events, vanilla_block_entity_types, vanilla_block_tags::BlockTag,
    vanilla_blocks, vanilla_entities, vanilla_game_events, vanilla_game_rules,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::behavior::{
    BlockBehavior, BlockEntityCreation, BlockHitResult, BlockPlaceContext, InteractionResult,
    InventoryAccess,
};
use crate::block_entity::entities::BellBlockEntity;
use crate::block_entity::{BLOCK_ENTITIES, BlockEntityTicker};
use crate::entity::Entity;
use crate::entity::ai::path::PathComputationType;
use crate::entity::projectile::Projectile;
use crate::player::Player;
use crate::world::explosion::{BlockInteraction, ExplosionInfo, default_on_explosion_hit};
use crate::world::game_event::GameEventContext;
use crate::world::{ClipHitResult, LevelReader, ScheduledTickAccess, SignalGetter as _, World};

/// Vanilla compares against the promoted `0.8124F` literal used by `BellBlock.isProperHit`.
const MAX_HIT_HEIGHT: f64 = 0.8124_f32 as f64;
const FACING: &EnumProperty<Direction> = &BlockStateProperties::FACING;
const BELL_ATTACHMENT: &EnumProperty<BellAttachType> = &BlockStateProperties::BELL_ATTACHMENT;
const POWERED: &BoolProperty = &BlockStateProperties::POWERED;

/// Vanilla `BellBlock`, including placement, ringing, and redstone activation.
#[block_behavior]
pub struct BellBlock {
    block: BlockRef,
}

impl BellBlock {
    /// Creates bell behavior for `block`.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    fn connected_direction(state: BlockStateId) -> Direction {
        match state.get_value(BELL_ATTACHMENT) {
            BellAttachType::Floor => Direction::Down,
            BellAttachType::Ceiling => Direction::Up,
            BellAttachType::SingleWall | BellAttachType::DoubleWall => state.get_value(FACING),
        }
    }

    /// Vanilla `Block.canSupportCenter`.
    ///
    /// Unlike a full-face check this accepts any shape that covers the centre of
    /// the face, and rejects `minecraft:unstable_bottom_center` blocks when the
    /// required support direction is `DOWN`.
    fn can_support_center(world: &dyn LevelReader, pos: BlockPos, direction: Direction) -> bool {
        let state = world.get_block_state(pos);
        !(direction == Direction::Down
            && state.get_block().has_tag(&BlockTag::UNSTABLE_BOTTOM_CENTER))
            && world.is_face_sturdy_for(state, pos, direction, SupportType::Center)
    }

    /// Vanilla `BellBlock.canSurvive` support test for one connection direction.
    ///
    /// A ceiling bell hangs from `Block.canSupportCenter`; every other attachment
    /// uses `FaceAttachedHorizontalDirectionalBlock.canAttach`.
    fn has_support(world: &dyn LevelReader, pos: BlockPos, direction: Direction) -> bool {
        let support_pos = pos.relative(direction);
        if direction == Direction::Up {
            return Self::can_support_center(world, support_pos, Direction::Down);
        }

        let support = world.get_block_state(support_pos);
        world.is_face_sturdy(support, support_pos, direction.opposite())
    }

    /// Vanilla `BellBlock.attemptToRing(Entity, Level, BlockPos, @Nullable Direction)`.
    ///
    /// Rings the bell at `pos`, notifying villagers through the block entity and playing
    /// the bell sound and game event. `direction` is the hit direction; `None` uses the
    /// bell's facing, matching vanilla's `attemptToRing(level, pos, null)` overload.
    ///
    /// Returns whether the position held a bell block entity, i.e. whether it rang.
    pub fn attempt_to_ring(
        source: Option<&dyn Entity>,
        world: &Arc<World>,
        pos: BlockPos,
        direction: Option<Direction>,
    ) -> bool {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        let Some(bell) = block_entity.downcast_ref::<BellBlockEntity>() else {
            return false;
        };

        let direction = direction.unwrap_or_else(|| world.get_block_state(pos).get_value(FACING));
        bell.on_hit(direction);

        world.play_sound(
            &sound_events::BLOCK_BELL_USE,
            SoundSource::Blocks,
            pos,
            2.0,
            1.0,
            None,
        );
        world.game_event(
            &vanilla_game_events::BLOCK_CHANGE,
            pos,
            &GameEventContext::new(source, None),
        );
        if let Some(entity) = source
            && let Some(player) = entity.as_player()
        {
            player.award_custom_stat(&BELL_RING);
        }

        true
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

    fn is_proper_hit(state: BlockStateId, direction: Direction, height: f64) -> bool {
        if direction.axis().is_vertical() {
            return false;
        }

        if height > MAX_HIT_HEIGHT {
            return false;
        }

        let facing = state.get_value(FACING);
        let attachment = state.get_value(BELL_ATTACHMENT);

        match attachment {
            BellAttachType::Floor => facing.axis() == direction.axis(),
            BellAttachType::Ceiling => true,
            BellAttachType::SingleWall | BellAttachType::DoubleWall => {
                facing.axis() != direction.axis()
            }
        }
    }

    fn update_powered(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
        let powered = world.has_neighbor_signal(pos);
        let old_powered = state.get_value(POWERED);

        if powered == old_powered {
            return;
        }

        if powered {
            let direction = state.get_value(FACING);
            Self::attempt_to_ring(None, world, pos, Some(direction));
        }

        let new_state = state.set_value(POWERED, powered);
        world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
    }
}

impl BlockBehavior for BellBlock {
    fn get_state_for_placement(&self, context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        let pos = context.place_pos();
        let clicked_face = context.clicked_face();
        let player_facing = context.horizontal_direction();

        let mut state = self.block.default_state();
        state = state.set_value(FACING, player_facing);

        if clicked_face == Direction::Up {
            state = state.set_value(BELL_ATTACHMENT, BellAttachType::Floor);
        } else if clicked_face == Direction::Down {
            state = state.set_value(BELL_ATTACHMENT, BellAttachType::Ceiling);
        } else {
            let wall_facing = clicked_face.opposite();

            state = state.set_value(FACING, wall_facing);

            let opposite = wall_facing.opposite();

            let double_wall = Self::has_support(context.world, pos, wall_facing)
                && Self::has_support(context.world, pos, opposite);

            let attachment = if double_wall {
                BellAttachType::DoubleWall
            } else {
                BellAttachType::SingleWall
            };

            state = state.set_value(BELL_ATTACHMENT, attachment);

            if self.can_survive(state, context.world, pos) {
                return Some(state);
            }

            let can_attach_below = Self::has_support(context.world, pos, Direction::Down);
            let fallback = if can_attach_below {
                BellAttachType::Floor
            } else {
                BellAttachType::Ceiling
            };
            state = state.set_value(BELL_ATTACHMENT, fallback);
        }

        self.can_survive(state, context.world, pos).then_some(state)
    }

    fn can_survive(&self, state: BlockStateId, world: &dyn LevelReader, pos: BlockPos) -> bool {
        let direction = Self::connected_direction(state);
        Self::has_support(world, pos, direction)
    }

    fn update_shape(
        &self,
        state: BlockStateId,
        world: &dyn ScheduledTickAccess,
        pos: BlockPos,
        direction: Direction,
        neighbor_pos: BlockPos,
        neighbor_state: BlockStateId,
    ) -> BlockStateId {
        let attachment = state.get_value(BELL_ATTACHMENT);
        let support_direction = Self::connected_direction(state);

        if support_direction == direction
            && attachment != BellAttachType::DoubleWall
            && !Self::has_support(world, pos, support_direction)
        {
            return vanilla_blocks::AIR.default_state();
        }

        let facing = state.get_value(FACING);

        if direction.axis() == facing.axis() {
            if attachment == BellAttachType::DoubleWall
                && !world.is_face_sturdy(neighbor_state, neighbor_pos, direction)
            {
                return state
                    .set_value(FACING, direction.opposite())
                    .set_value(BELL_ATTACHMENT, BellAttachType::SingleWall);
            }

            if attachment == BellAttachType::SingleWall
                && support_direction.opposite() == direction
                && world.is_face_sturdy(neighbor_state, neighbor_pos, facing)
            {
                return state.set_value(BELL_ATTACHMENT, BellAttachType::DoubleWall);
            }
        }

        state
    }

    fn handle_neighbor_changed(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        Self::update_powered(state, world, pos);
    }

    fn use_without_item(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        hit: &BlockHitResult,
        _inv: &mut InventoryAccess,
    ) -> InteractionResult {
        let height = hit.location.y - f64::from(pos.y());
        if !Self::is_proper_hit(state, hit.direction, height) {
            return InteractionResult::Pass;
        }

        Self::attempt_to_ring(Some(player), world, pos, Some(hit.direction));
        InteractionResult::Success
    }

    fn on_projectile_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        hit: &ClipHitResult,
        projectile: &dyn Projectile,
    ) {
        let height = hit.location.y - f64::from(hit.block_pos.y());
        if !Self::is_proper_hit(state, hit.direction, height) {
            return;
        }

        let owner = projectile.get_owner();
        let source = owner
            .as_deref()
            .filter(|entity| entity.as_player().is_some());
        Self::attempt_to_ring(source, world, hit.block_pos, Some(hit.direction));
    }

    fn new_block_entity(
        &self,
        level: Weak<World>,
        pos: BlockPos,
        state: BlockStateId,
    ) -> BlockEntityCreation {
        BlockEntityCreation::from_registered_factory(BLOCK_ENTITIES.create(
            &vanilla_block_entity_types::BELL,
            level,
            pos,
            state,
        ))
    }

    fn get_block_entity_ticker(
        &self,
        _world: &Arc<World>,
        _state: BlockStateId,
        block_entity_type: BlockEntityTypeRef,
    ) -> Option<BlockEntityTicker> {
        BlockEntityTicker::for_matching_entity_tick(
            block_entity_type,
            &vanilla_block_entity_types::BELL,
        )
    }

    fn trigger_event(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        event: i32,
        data: i32,
    ) -> bool {
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        block_entity.trigger_event(event, data)
    }

    fn is_pathfindable(
        &self,
        _state: BlockStateId,
        _computation_type: PathComputationType,
    ) -> bool {
        false
    }

    /// Vanilla `BellBlock.onExplosionHit`: an explosion that can trigger blocks
    /// rings the bell before the default explosion handling runs.
    fn on_explosion_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        explosion: &ExplosionInfo<'_>,
        on_drop: &mut dyn FnMut(ItemStack, BlockPos),
    ) {
        if Self::can_trigger_blocks(world, explosion) {
            Self::attempt_to_ring(None, world, pos, None);
        }
        default_on_explosion_hit(state, world, pos, explosion, on_drop);
    }
}

#[cfg(test)]
mod tests {
    use glam::DVec3;
    use steel_registry::init_vanilla_registry;
    use steel_registry::vanilla_damage_types;
    use steel_utils::ChunkPos;

    use super::*;
    use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
    use crate::block_entity::init_block_entities;
    use crate::entity::SharedEntity;
    use crate::entity::damage::DamageSource;
    use crate::test_support::{TestEntity, TestLevel, fresh_test_world, insert_ready_full_chunk};

    #[test]
    fn wall_attachment_uses_facing_as_support_direction() {
        init_vanilla_registry();
        let state = vanilla_blocks::BELL
            .default_state()
            .set_value(BELL_ATTACHMENT, BellAttachType::SingleWall)
            .set_value(FACING, Direction::West);

        assert_eq!(BellBlock::connected_direction(state), Direction::West);
    }

    #[test]
    fn generated_registry_bell_behavior_creates_registered_typed_entity() {
        init_vanilla_registry();
        init_block_entities();
        init_behaviors();
        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::BELL);
        let entity = behavior
            .new_block_entity(
                Weak::new(),
                BlockPos::new(0, 64, 0),
                vanilla_blocks::BELL.default_state(),
            )
            .into_created()
            .expect("bell should create its registered block entity");

        assert!(BLOCK_ENTITIES.has_factory(&vanilla_block_entity_types::BELL));
        assert!(entity.downcast_ref::<BellBlockEntity>().is_some());
    }

    #[test]
    fn placed_bell_is_stored_with_its_ticker() {
        init_vanilla_registry();
        init_block_entities();
        init_behaviors();
        let world_fixture = fresh_test_world("placed_bell_entity");
        let world = &world_fixture.world;
        let pos = BlockPos::new(4, 64, 4);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        assert!(world.set_block(
            pos.relative(Direction::Down),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let state = vanilla_blocks::BELL.default_state();
        assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));

        let entity = world
            .get_block_entity(pos)
            .expect("placed bell should be stored as a block entity");
        assert!(entity.downcast_ref::<BellBlockEntity>().is_some());
        assert!(
            BLOCK_BEHAVIORS
                .get_behavior(&vanilla_blocks::BELL)
                .get_block_entity_ticker(world, state, entity.get_type())
                .is_some()
        );
    }

    #[test]
    fn ceiling_bell_hangs_from_center_support_and_rejects_unstable_bottom_center() {
        init_vanilla_registry();
        init_behaviors();
        let behavior = BellBlock::new(&vanilla_blocks::BELL);
        let pos = BlockPos::new(0, 64, 0);
        let ceiling = vanilla_blocks::BELL
            .default_state()
            .set_value(BELL_ATTACHMENT, BellAttachType::Ceiling);

        // A hopper's bottom covers only the centre of its downward face, so
        // vanilla's `Block.canSupportCenter` accepts it while a full-face
        // `isFaceSturdy` check does not.
        let hopper_pos = pos.above();
        let hopper_state = vanilla_blocks::HOPPER.default_state();
        let hopper = TestLevel::default().with_block(hopper_pos, hopper_state);
        assert!(
            hopper.is_face_sturdy_for(
                hopper_state,
                hopper_pos,
                Direction::Down,
                SupportType::Center,
            ),
            "hopper bottom should provide centre support"
        );
        assert!(
            !hopper.is_face_sturdy(hopper_state, hopper_pos, Direction::Down),
            "hopper bottom should not be full-face sturdy"
        );
        assert!(behavior.can_survive(ceiling, &hopper, pos));

        // Fence gates are listed in `minecraft:unstable_bottom_center`, and their
        // vanilla support shape (`Block.column(16.0, 4.0, 5.0, 24.0)`) also starts
        // 5/16 above the block bottom, so `Block.canSupportCenter` rejects them on
        // both counts: `getFaceShape(DOWN)` slices the shape below its lowest
        // coordinate and comes back empty.
        let gate_pos = pos.above();
        let gate_state = vanilla_blocks::OAK_FENCE_GATE.default_state();
        let gate = TestLevel::default().with_block(gate_pos, gate_state);
        assert!(
            gate_state
                .get_block()
                .has_tag(&BlockTag::UNSTABLE_BOTTOM_CENTER)
        );
        assert!(
            !gate.is_face_sturdy_for(gate_state, gate_pos, Direction::Down, SupportType::Center),
            "a closed fence gate's support shape does not touch its bottom face"
        );
        assert!(!behavior.can_survive(ceiling, &gate, pos));

        // Floor attachment still uses the full-face `canAttach` check.
        let floor = vanilla_blocks::BELL.default_state();
        assert!(!behavior.can_survive(floor, &hopper, pos));
    }

    fn explosion<'a>(
        source: &'a SharedEntity,
        block_interaction: BlockInteraction,
        damage_source: &'a DamageSource,
    ) -> ExplosionInfo<'a> {
        ExplosionInfo {
            center: DVec3::new(8.5, 64.5, 8.5),
            radius: 4.0,
            block_interaction,
            fire: false,
            source: Some(source),
            indirect_source: None,
            damage_source,
        }
    }

    #[test]
    fn trigger_block_explosion_rings_bell_like_vanilla() {
        init_vanilla_registry();
        init_block_entities();
        init_behaviors();
        let world_fixture = fresh_test_world("bell_explosion_ring");
        let world = &world_fixture.world;
        let pos = BlockPos::new(8, 64, 8);
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(pos));
        assert!(world.set_block(
            pos.relative(Direction::Down),
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL,
        ));
        let state = vanilla_blocks::BELL.default_state();
        assert!(world.set_block(pos, state, UpdateFlags::UPDATE_ALL));

        let source: SharedEntity = TestEntity::shared(
            9_101,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
            &vanilla_entities::TNT,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::EXPLOSION);
        let explosion = explosion(&source, BlockInteraction::TriggerBlock, &damage_source);

        let behavior = BLOCK_BEHAVIORS.get_behavior(&vanilla_blocks::BELL);
        let mut drops = Vec::new();
        behavior.on_explosion_hit(state, world, pos, &explosion, &mut |item, drop_pos| {
            drops.push((item, drop_pos));
        });

        // Vanilla `BellBlock.attemptToRing(level, pos, null)` rings through the
        // block entity and keeps the bell alive for trigger-block explosions.
        let entity = world
            .get_block_entity(pos)
            .expect("belled ring should leave its block entity in place");
        assert!(entity.downcast_ref::<BellBlockEntity>().is_some());
        assert_eq!(
            world.get_block_state(pos).get_block(),
            &vanilla_blocks::BELL
        );
        assert!(drops.is_empty(), "trigger-block explosions keep the block");
    }

    #[test]
    fn can_trigger_blocks_matches_vanilla_explosion_modes() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("bell_explosion_modes");
        let world = &world_fixture.world;
        let source: SharedEntity = TestEntity::shared(
            9_102,
            DVec3::new(8.5, 64.0, 8.5),
            Arc::downgrade(world),
            &vanilla_entities::BREEZE_WIND_CHARGE,
        );
        let damage_source = DamageSource::environment(&vanilla_damage_types::EXPLOSION);

        assert!(BellBlock::can_trigger_blocks(
            world,
            &explosion(&source, BlockInteraction::TriggerBlock, &damage_source)
        ));
        assert!(!BellBlock::can_trigger_blocks(
            world,
            &explosion(&source, BlockInteraction::Destroy, &damage_source)
        ));

        // Breeze wind charges additionally follow the `mob_griefing` rule.
        assert!(world.set_game_rule(&vanilla_game_rules::MOB_GRIEFING, false));
        assert!(!BellBlock::can_trigger_blocks(
            world,
            &explosion(&source, BlockInteraction::TriggerBlock, &damage_source)
        ));
    }
}
