//! `TntBlock` behavior (`net.minecraft.world.level.block.TntBlock`).
//!
//! Deviation: vanilla passes an arrow's pickup stack as the adventure-mode break
//! tool in `TntBlock.onProjectileHit`; Steel's `Projectile` trait exposes no
//! pickup-item stack (`entity/projectile`), so the empty stack is used there.

use std::sync::Arc;

use glam::DVec3;
use steel_macros::block_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, BoolProperty};
use steel_registry::data_components::vanilla_components::CAN_BREAK;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_game_rules::TNT_EXPLODES;
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_entities, vanilla_game_events, vanilla_items,
};
use steel_utils::nbt::compare_nbt_compounds;
use steel_utils::translations;
use steel_utils::types::{GameType, InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::InventoryAccess;
use crate::behavior::{BlockBehavior, BlockHitResult, BlockPlaceContext, InteractionResult};
use crate::block_entity::BlockEntity as _;
use crate::entity::entities::objects::explosives::PrimedTntEntity;
use crate::entity::projectile::Projectile;
use crate::entity::{ENTITIES, Entity, SharedEntity, next_entity_id};
use crate::player::Player;
use crate::world::explosion::{BlockInteraction, ExplosionInfo};
use crate::world::game_event::GameEventContext;
use crate::world::{ClipHitResult, SignalGetter as _, World};

const UNSTABLE: &BoolProperty = &BlockStateProperties::UNSTABLE;

/// Vanilla `ItemStack.canBreakBlockInAdventureMode`.
///
/// Mirrors `player::game_mode::adventure_mode::can_break`, which is private to the
/// player game-mode module.
fn can_break_block_in_adventure_mode(stack: &ItemStack, world: &World, pos: BlockPos) -> bool {
    let Some(predicate) = stack.get(CAN_BREAK) else {
        return false;
    };
    predicate.predicates().iter().any(|predicate| {
        if !predicate.matches_state(world.get_block_state(pos)) {
            return false;
        }
        let Some(expected_nbt) = predicate.nbt() else {
            return true;
        };
        let Some(block_entity) = world.get_block_entity(pos) else {
            return false;
        };
        compare_nbt_compounds(
            expected_nbt.tag(),
            &block_entity.save_with_full_metadata(),
            true,
        )
    })
}

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
    /// Spawns a primed TNT entity owned by `source`, plays `ENTITY_TNT_PRIMED` and
    /// fires the `PRIME_FUSE` game event. Returns `false` when the `tntExplodes`
    /// game rule is off, when the source player may not break the block in
    /// adventure mode, or when the entity could not be created.
    pub fn prime(
        world: &Arc<World>,
        pos: BlockPos,
        source: Option<&dyn Entity>,
        item_stack: &ItemStack,
    ) -> bool {
        if !world.get_game_rule(&TNT_EXPLODES) || !ENTITIES.has_factory(&vanilla_entities::TNT) {
            return false;
        }
        if let Some(player) = source.and_then(Entity::as_player)
            && player.game_mode() == GameType::Adventure
            && !can_break_block_in_adventure_mode(item_stack, world, pos)
        {
            return false;
        }

        let owner = source.and_then(|source| world.get_entity_by_id(source.id()));
        let position = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let tnt = PrimedTntEntity::primed(
            &vanilla_entities::TNT,
            next_entity_id(),
            position,
            Arc::downgrade(world),
            owner.as_ref(),
        );
        let entity: SharedEntity = Arc::new(tnt);
        if world.try_add_entity(entity).is_err() {
            return false;
        }

        world.play_sound(
            &sound_events::ENTITY_TNT_PRIMED,
            SoundSource::Blocks,
            pos,
            1.0,
            1.0,
            None,
        );
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
            && Self::prime(world, pos, None, &ItemStack::empty())
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
        if world.has_neighbor_signal(pos) && Self::prime(world, pos, None, &ItemStack::empty()) {
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
            let _ = Self::prime(world, pos, Some(player), &ItemStack::empty());
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

        let Some((igniter, stack)) = inv.with_item(|stack| {
            if stack.is(&vanilla_items::FLINT_AND_STEEL) {
                Some((Igniter::FlintAndSteel, stack.clone()))
            } else if stack.is(&vanilla_items::FIRE_CHARGE) {
                Some((Igniter::FireCharge, stack.clone()))
            } else {
                None
            }
        }) else {
            return InteractionResult::TryEmptyHandInteraction;
        };

        if !Self::prime(world, pos, Some(player), &stack) {
            // Vanilla `TntBlock.useItemOn`: only the disabled game rule reports
            // back to the player; an adventure-mode refusal stays silent.
            if !world.get_game_rule(&TNT_EXPLODES) {
                player.send_overlay_message(&translations::BLOCK_MINECRAFT_TNT_DISABLED.msg());
            }
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
        let source = projectile.get_owner();
        // Vanilla passes `AbstractArrow.getPickupItemStackOrigin()`; Steel's
        // `Projectile` trait exposes no pickup stack, so the non-arrow empty stack
        // is used for every projectile.
        if projectile.is_on_fire()
            && projectile.projectile_may_interact(world, pos)
            && Self::prime(world, pos, source.as_deref(), &ItemStack::empty())
        {
            Self::remove_primed_block(world, pos);
        }
    }

    /// Vanilla `TntBlock.dropFromExplosion` returns `false`, so an exploded TNT
    /// block drops nothing; Steel's default explosion handler always drops loot,
    /// so the body is mirrored here without the loot pass.
    fn on_explosion_hit(
        &self,
        state: BlockStateId,
        world: &Arc<World>,
        pos: BlockPos,
        explosion: &ExplosionInfo<'_>,
        _on_drop: &mut dyn FnMut(ItemStack, BlockPos),
    ) {
        if state.is_air() || explosion.block_interaction == BlockInteraction::TriggerBlock {
            return;
        }
        world.set_block(
            pos,
            vanilla_blocks::AIR.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        self.was_exploded(world, pos, explosion);
    }

    /// Vanilla `TntBlock.wasExploded`: spawn a primed TNT with a randomized short
    /// fuse owned by the explosion's indirect source.
    fn was_exploded(&self, world: &Arc<World>, pos: BlockPos, explosion: &ExplosionInfo<'_>) {
        if !world.get_game_rule(&TNT_EXPLODES) || !ENTITIES.has_factory(&vanilla_entities::TNT) {
            return;
        }

        let position = DVec3::new(
            f64::from(pos.x()) + 0.5,
            f64::from(pos.y()),
            f64::from(pos.z()) + 0.5,
        );
        let tnt = PrimedTntEntity::primed(
            &vanilla_entities::TNT,
            next_entity_id(),
            position,
            Arc::downgrade(world),
            explosion.indirect_source,
        );
        tnt.set_fuse(PrimedTntEntity::random_short_fuse(tnt.fuse()));
        if let Err(error) = world.try_add_entity(Arc::new(tnt)) {
            log::debug!("failed to spawn primed tnt: {error}");
        }
    }
}
