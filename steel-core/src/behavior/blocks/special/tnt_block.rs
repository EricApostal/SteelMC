//! `TntBlock` behavior (`net.minecraft.world.level.block.TntBlock`).

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_entities, vanilla_game_events, vanilla_items,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::InventoryAccess;
use crate::behavior::{BlockBehavior, BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::entity::projectile::Projectile;
use crate::entity::{ENTITIES, Entity, next_entity_id};
use crate::player::Player;
use crate::world::game_event::GameEventContext;
use crate::world::{ClipHitResult, SignalGetter as _, World};

const UNSTABLE: &BoolProperty = &BlockStateProperties::UNSTABLE;

/// `TntBlock` behavior.
#[block_behavior]
pub struct TntBlock {
    block: BlockRef,
}

impl TntBlock {
    /// Creates a TNT block behavior.
    #[must_use]
    pub const fn new(block: BlockRef) -> Self {
        Self { block }
    }

    /// Vanilla `TntBlock.prime`.
    ///
    /// Returns `false` when the primed TNT entity could not be created, matching
    /// vanilla's gamerule/adventure-mode guards.
    ///
    /// TODO(deps): Steel has no `PrimedTnt` entity implementation yet, so this
    /// returns `false` (and therefore does not prime) until the entity lane
    /// registers a factory for [`vanilla_entities::TNT`] whose fuse can be set
    /// through `Entity`. The spawn path below is already correct.
    pub fn prime(world: &Arc<World>, pos: BlockPos, source: Option<&dyn Entity>) -> bool {
        if !ENTITIES.has_factory(&vanilla_entities::TNT) {
            return false;
        }

        let position = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let Some(entity) = ENTITIES.create(
            &vanilla_entities::TNT,
            next_entity_id(),
            position,
            Arc::downgrade(world),
        ) else {
            return false;
        };
        if world.try_add_entity(entity).is_err() {
            return false;
        }

        world.play_block_sound(&sound_events::ENTITY_TNT_PRIMED, pos, 1.0, 1.0, None);
        world.game_event(
            &vanilla_game_events::PRIME_FUSE,
            pos,
            &GameEventContext::new(source, None),
        );
        true
    }

    fn remove_primed_block(world: &Arc<World>, pos: BlockPos) {
        let _ = world.remove_block(pos, false);
    }
}

impl BlockBehavior for TntBlock {
    fn get_state_for_placement(&self, _context: &BlockPlaceContext<'_>) -> Option<BlockStateId> {
        Some(self.block.default_state().set_value(UNSTABLE, false))
    }

    fn on_place(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        old_state: BlockStateId,
        _moved_by_piston: bool,
    ) {
        if old_state.get_block() != self.block
            && world.has_neighbor_signal(pos)
            && Self::prime(world, pos, None)
        {
            Self::remove_primed_block(world, pos);
        }
    }

    fn handle_neighbor_changed(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        _source_block: BlockRef,
        _moved_by_piston: bool,
    ) {
        if world.has_neighbor_signal(pos) && Self::prime(world, pos, None) {
            Self::remove_primed_block(world, pos);
        }
    }

    fn player_will_destroy(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
    ) -> BlockStateId {
        if !player.has_infinite_materials() && state.get_value(UNSTABLE) {
            let _ = Self::prime(world, pos, Some(player));
        }

        state
    }

    fn use_item_on(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        player: &Player,
        _hand: InteractionHand,
        _hit_result: &BlockHitResult,
        inv: &mut InventoryAccess,
    ) -> InteractionResult {
        enum Igniter {
            FlintAndSteel,
            FireCharge,
        }

        let Some(igniter) = inv.with_item(|stack| {
            if stack.is(&vanilla_items::FLINT_AND_STEEL) {
                Some(Igniter::FlintAndSteel)
            } else if stack.is(&vanilla_items::FIRE_CHARGE) {
                Some(Igniter::FireCharge)
            } else {
                None
            }
        }) else {
            return InteractionResult::TryEmptyHandInteraction;
        };

        if !Self::prime(world, pos, Some(player)) {
            return InteractionResult::Pass;
        }

        world.set_block(
            pos,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_NEIGHBORS
                | UpdateFlags::UPDATE_CLIENTS
                | UpdateFlags::UPDATE_SUPPRESS_DROPS,
        );

        let has_infinite_materials = player.has_infinite_materials();
        match igniter {
            Igniter::FlintAndSteel => {
                inv.with_item(|stack| stack.hurt_and_break(1, has_infinite_materials));
                player.award_stat(
                    &vanilla_stat_types::ITEM_USED,
                    &vanilla_items::FLINT_AND_STEEL,
                );
            }
            Igniter::FireCharge => {
                if !has_infinite_materials {
                    inv.with_item(|stack| stack.shrink(1));
                }
                player.award_stat(&vanilla_stat_types::ITEM_USED, &vanilla_items::FIRE_CHARGE);
            }
        }

        InteractionResult::Success
    }

    fn on_projectile_hit(
        &self,
        _state: BlockStateId,
        world: &Arc<World>,
        hit: &ClipHitResult,
        projectile: &dyn Projectile,
    ) {
        let pos = hit.block_pos;
        // TODO(deps): Vanilla passes the arrow's pickup stack as the adventure-mode
        // break-tool; Steel's `Projectile` trait does not expose it yet, so the
        // empty stack (vanilla's non-arrow case) is used.
        let source = projectile.get_owner();
        if projectile.is_on_fire()
            && projectile.projectile_may_interact(world, pos)
            && Self::prime(world, pos, source.as_deref())
        {
            Self::remove_primed_block(world, pos);
        }
    }
}
