//! End crystal item behavior (`EndCrystalItem`).
//!
//! Vanilla reference: `EndCrystalItem.useOn`
//! (`/tmp/vanilla/src/world/item/EndCrystalItem.java:21-55`):
//!
//! 1. the clicked block must be obsidian or bedrock, otherwise `FAIL`;
//! 2. the block above must be empty (`level.isEmptyBlock`), otherwise `FAIL`;
//! 3. no entity may occupy the `1 × 2 × 1` box above the base block, otherwise
//!    `FAIL`;
//! 4. server-side an `EndCrystal` is constructed at `(x + 0.5, y, z + 0.5)`,
//!    `setShowBottom(false)` is applied, the crystal is added,
//!    `GameEvent.ENTITY_PLACE` fires at the base-above position, and the dragon
//!    fight is asked to `tryRespawn()`;
//! 5. `stack.shrink(1)` and `SUCCESS`.
//!
//! Vanilla deliberately does *not* apply `createDefaultStackConfig` here, so the
//! item stack's entity data is ignored — this port keeps that.
//!
//! The dragon-fight step is not reachable: `EnderDragonFight::try_respawn`
//! exists (`steel-core/src/entity/boss/dragon_fight.rs:645`) but no world holds a
//! fight yet — `steel-core/src/world/` has no `dragon_fight` accessor, and the
//! End Crystal entity carries the same TODO
//! (`steel-core/src/entity/entities/objects/explosives/end_crystal.rs:156-159`).

use std::sync::Arc;

use glam::DVec3;
use steel_macros::item_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entities;
use steel_registry::vanilla_game_events;
use steel_utils::{BlockPos, Downcast as _, WorldAabb};

use crate::behavior::ItemBehavior;
use crate::behavior::context::{InteractionResult, UseOnContext};
use crate::entity::entities::EndCrystalEntity;
use crate::entity::{add_spawned_entity, create_entity_instance};
use crate::world::game_event::GameEventContext;

/// Behavior for the `end_crystal` item.
#[item_behavior]
pub struct EndCrystalItem;

impl EndCrystalItem {
    /// Vanilla `new AABB(x, y, z, x + 1.0, y + 2.0, z + 1.0)` above the base.
    fn placement_box(above: BlockPos) -> WorldAabb {
        let x = f64::from(above.x());
        let y = f64::from(above.y());
        let z = f64::from(above.z());
        WorldAabb::new(x, y, z, x + 1.0, y + 2.0, z + 1.0)
    }
}

impl ItemBehavior for EndCrystalItem {
    fn use_on(&self, context: &mut UseOnContext) -> InteractionResult {
        let pos = context.hit_result.block_pos;
        let state = context.world.get_block_state(pos);
        let block = state.get_block();

        if block != &vanilla_blocks::OBSIDIAN && block != &vanilla_blocks::BEDROCK {
            return InteractionResult::Fail;
        }

        let above = pos.above();
        if !context.world.get_block_state(above).is_air() {
            return InteractionResult::Fail;
        }

        if !context
            .world
            .get_entities_in_aabb(&Self::placement_box(above))
            .is_empty()
        {
            return InteractionResult::Fail;
        }

        let spawn_pos = DVec3::new(
            f64::from(above.x()) + 0.5,
            f64::from(above.y()),
            f64::from(above.z()) + 0.5,
        );
        let crystal = match create_entity_instance(
            context.world,
            &vanilla_entities::END_CRYSTAL,
            spawn_pos,
        ) {
            Ok(crystal) => crystal,
            Err(error) => {
                log::warn!("Failed to create end crystal at {above:?}: {error:?}");
                return InteractionResult::Fail;
            }
        };

        // Vanilla `crystal.setShowBottom(false)`.
        if let Some(crystal) = crystal.as_ref().downcast_ref::<EndCrystalEntity>() {
            crystal.set_show_bottom(false);
        }

        if let Err(error) = add_spawned_entity(context.world, Arc::clone(&crystal)) {
            log::warn!("Failed to add end crystal to the world: {error:?}");
            return InteractionResult::Fail;
        }
        context.world.game_event(
            &vanilla_game_events::ENTITY_PLACE,
            above,
            &GameEventContext::new(Some(&**context.player), None),
        );

        // Vanilla `serverLevel.getDragonFight().tryRespawn()`: Steel has no
        // world-level dragon fight yet, so this step cannot run.

        context.inv.with_item(|item| item.shrink(1));
        InteractionResult::Success
    }
}
