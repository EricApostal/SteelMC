//! Vanilla `TryLaySpawnOnFluidNearLand`: lay frogspawn on fluid beside land.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::registry::TaggedRegistryExt as _;
use steel_registry::sound_events;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_fluid_tags::FluidTag;
use steel_registry::vanilla_game_events;
use steel_utils::BlockPos;
use steel_utils::BlockStateId;
use steel_utils::types::UpdateFlags;

use crate::entity::ai::brain::behavior::{BehaviorContext, OneShot, Trigger};
use crate::entity::ai::brain::memory::module_type as mem;
use crate::world::game_event::GameEventContext;

/// Blocks checked horizontally around the body for a spawn-supporting surface.
const HORIZONTAL_OFFSETS: [(i32, i32); 4] = [(1, 0), (-1, 0), (0, 1), (0, -1)];

/// Vanilla `TryLaySpawnOnFluidNearLand`.
pub struct TryLaySpawnOnFluidNearLand {
    spawn_block: BlockStateId,
}

impl TryLaySpawnOnFluidNearLand {
    /// Vanilla `TryLaySpawnOnFluidNearLand.create(spawnBlock)`.
    #[must_use]
    pub fn create(spawn_block: BlockStateId) -> OneShot<Self> {
        OneShot::new(Self { spawn_block }).with_required_memories(vec![
            &mem::ATTACK_TARGET,
            &mem::WALK_TARGET,
            &mem::IS_PREGNANT,
        ])
    }

    /// Vanilla's fluid/solid spawn-surface test for a horizontal neighbor.
    fn is_spawn_surface(ctx: &BehaviorContext<'_>, pos: BlockPos) -> bool {
        let state = ctx.level.get_block_state(pos);
        steel_registry::REGISTRY.blocks.is_in_tag(
            ctx.level.get_block_state(pos).get_block(),
            &BlockTag::SUPPORTS_FROGSPAWN,
        ) || state
            .get_fluid_state()
            .fluid_id
            .has_tag(&FluidTag::SUPPORTS_FROGSPAWN)
    }
}

impl Trigger for TryLaySpawnOnFluidNearLand {
    fn trigger(&mut self, ctx: &mut BehaviorContext<'_>) -> bool {
        if !ctx.body.is_in_water() && ctx.body.on_ground() {
            return false;
        }
        if !ctx.has_memory_value(&mem::IS_PREGNANT) {
            return false;
        }

        let below = ctx.body.block_position().below();
        for (dx, dz) in HORIZONTAL_OFFSETS {
            let relative = below.offset(dx, 0, dz);
            let spawn_pos = relative.above();
            if !ctx.level.get_block_state(spawn_pos).is_air() {
                continue;
            }
            if !Self::is_spawn_surface(ctx, relative) {
                continue;
            }

            let Some(world) = ctx.body.level() else {
                continue;
            };
            world.set_block(spawn_pos, self.spawn_block, UpdateFlags::UPDATE_ALL);
            world.game_event(
                &vanilla_game_events::BLOCK_PLACE,
                spawn_pos,
                &GameEventContext::new(Some(ctx.body.as_ref()), Some(self.spawn_block)),
            );
            ctx.level.play_block_sound(
                &sound_events::ENTITY_FROG_LAY_SPAWN,
                spawn_pos,
                1.0,
                1.0,
                None,
            );
            ctx.erase_memory(&mem::IS_PREGNANT);
            return true;
        }

        true
    }
}
