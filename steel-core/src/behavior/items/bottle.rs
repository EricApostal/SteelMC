//! Glass bottle item behavior (`BottleItem`).
//!
//! Mirrors vanilla `BottleItem.use`: a living dragon's area-effect cloud within
//! two blocks of the player is milked into a dragon's-breath bottle (shrinking
//! the cloud by half a block of radius), otherwise a water source fills the
//! bottle with a water potion via `ItemUtils.createFilledResult`.

use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
use steel_registry::item_stack::ItemStack;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{
    RegistryReference, sound_events, vanilla_game_events, vanilla_items, vanilla_potions,
};
use steel_utils::Downcast as _;

use crate::behavior::context::{InteractionResult, UseItemContext};
use crate::behavior::item::ItemBehavior;
use crate::behavior::item_utils::{create_filled_result, get_player_pov_hit_result};
use crate::entity::entities::mobs::bosses::EnderDragonEntity;
use crate::entity::entities::objects::projectiles::AreaEffectCloudEntity;
use crate::entity::{Entity, SharedEntity};
use crate::fluid::FluidStateExt;
use crate::world::ClipFluid;
use crate::world::game_event::GameEventContext;

/// Behavior for the glass bottle item.
#[item_behavior(class = "BottleItem")]
pub struct BottleItem;

impl BottleItem {
    /// Vanilla `BottleItem.use` cloud scan: the first living
    /// `AreaEffectCloud` owned by an ender dragon that intersects the player's
    /// bounding box inflated by two blocks.
    fn dragon_breath_cloud(context: &UseItemContext) -> Option<SharedEntity> {
        let aabb = context.player.bounding_box().inflate(2.0);
        context
            .world
            .get_entities_in_aabb(&aabb)
            .into_iter()
            .find(|entity| {
                let Some(cloud) = entity.downcast_ref::<AreaEffectCloudEntity>() else {
                    return false;
                };
                cloud.is_alive()
                    && cloud
                        .owner()
                        .is_some_and(|owner| owner.downcast_ref::<EnderDragonEntity>().is_some())
            })
    }
}

impl ItemBehavior for BottleItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        // Vanilla `BottleItem.use` checks living dragon clouds first.
        if let Some(entity) = Self::dragon_breath_cloud(context) {
            let Some(cloud) = entity.downcast_ref::<AreaEffectCloudEntity>() else {
                return InteractionResult::Pass;
            };
            cloud.set_radius(cloud.radius() - 0.5);
            context.world.play_sound_at(
                &sound_events::ITEM_BOTTLE_FILL_DRAGONBREATH,
                SoundSource::Neutral,
                context.player.position(),
                1.0,
                1.0,
                None,
            );
            context.world.game_event_at(
                &vanilla_game_events::FLUID_PICKUP,
                context.player.position(),
                &GameEventContext::new(Some(context.player.as_ref()), None),
            );
            create_filled_result(context, ItemStack::new(&vanilla_items::DRAGON_BREATH), true);
            return InteractionResult::Success;
        }

        let hit = get_player_pov_hit_result(context.world, context.player, ClipFluid::SourceOnly);
        if hit.miss {
            return InteractionResult::Pass;
        }

        let pos = hit.block_pos;
        if !context.world.may_interact(context.player, pos) {
            return InteractionResult::Pass;
        }

        let fluid_state = context.world.get_block_state(pos).get_fluid_state();
        if !fluid_state.is_water() {
            return InteractionResult::Pass;
        }

        context.world.play_sound_at(
            &sound_events::ITEM_BOTTLE_FILL,
            SoundSource::Neutral,
            context.player.position(),
            1.0,
            1.0,
            Some(context.player.id()),
        );
        context.world.game_event(
            &vanilla_game_events::FLUID_PICKUP,
            pos,
            &GameEventContext::new(Some(context.player.as_ref()), None),
        );

        context.inv.with_item(|item| {
            context
                .player
                .award_stat(&vanilla_stat_types::ITEM_USED, item.item());
        });
        create_filled_result(context, water_potion_stack(), true);

        InteractionResult::Success
    }
}

pub(crate) fn water_potion_stack() -> ItemStack {
    let mut stack = ItemStack::new(&vanilla_items::POTION);
    stack.set(
        POTION_CONTENTS,
        PotionContents::new(
            Some(RegistryReference::new(&vanilla_potions::WATER)),
            None,
            Vec::new(),
            None,
        ),
    );
    stack
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::blocks::block_state_ext::BlockStateExt;
    use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_items, vanilla_potions,
    };
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, BlockStateId, ChunkPos, Downcast as _};

    use crate::behavior::item::ItemBehavior;
    use crate::behavior::{InteractionResult, UseItemContext, init_behaviors};
    use crate::entity::entities::mobs::bosses::EnderDragonEntity;
    use crate::entity::entities::objects::projectiles::AreaEffectCloudEntity;
    use crate::entity::{Entity, SharedEntity};
    use crate::player::Player;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};
    use crate::world::World;

    use super::BottleItem;

    fn looking_down_at(
        world: &Arc<World>,
        feet: DVec3,
        target: BlockPos,
        state: BlockStateId,
    ) -> Arc<Player> {
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(target));
        if world.get_block_state(target) != state {
            assert!(world.set_block(target, state, UpdateFlags::UPDATE_NONE));
        }

        let player = TestPlayerBuilder::new(Arc::clone(world), "BottleTester", 1).build();
        player
            .try_set_position(feet)
            .expect("test player should move onto the target chunk");
        player.set_rotation((0.0, 90.0));
        player
    }

    fn use_bottle(player: &Arc<Player>, world: &Arc<World>, count: i32) -> InteractionResult {
        player.inventory.lock().set_item_in_hand(
            InteractionHand::MainHand,
            ItemStack::with_count(&vanilla_items::GLASS_BOTTLE, count),
        );
        let mut context = UseItemContext::new(
            player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );
        BottleItem.use_item(&mut context)
    }

    fn hand_item(player: &Player) -> ItemStack {
        player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand)
            .clone()
    }

    #[test]
    fn fills_from_a_water_source_and_leaves_the_source() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("bottle_fill_water");
        let world = &world_fixture.world;
        let water_pos = BlockPos::new(0, 80, 0);
        let player = looking_down_at(
            world,
            DVec3::new(0.5, 81.0, 0.5),
            water_pos,
            vanilla_blocks::WATER.default_state(),
        );

        assert_eq!(use_bottle(&player, world, 1), InteractionResult::Success);

        let filled = hand_item(&player);
        assert_eq!(filled.item.key, vanilla_items::POTION.key);
        assert!(
            filled
                .get(POTION_CONTENTS)
                .is_some_and(|contents| contents.is(&vanilla_potions::WATER))
        );
        assert_eq!(
            world.get_block_state(water_pos).get_block(),
            &vanilla_blocks::WATER
        );
    }

    #[test]
    fn non_water_blocks_and_misses_do_not_fill() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("bottle_fill_stone");
        let world = &world_fixture.world;
        let player = looking_down_at(
            world,
            DVec3::new(0.5, 81.0, 0.5),
            BlockPos::new(0, 80, 0),
            vanilla_blocks::STONE.default_state(),
        );

        assert_eq!(use_bottle(&player, world, 1), InteractionResult::Pass);
        assert_eq!(hand_item(&player).item.key, vanilla_items::GLASS_BOTTLE.key);

        let air_world_fixture = fresh_test_world("bottle_fill_air");
        let air_world = &air_world_fixture.world;
        let air_player = looking_down_at(
            air_world,
            DVec3::new(0.5, 81.0, 0.5),
            BlockPos::new(0, 80, 0),
            vanilla_blocks::AIR.default_state(),
        );
        assert_eq!(
            use_bottle(&air_player, air_world, 1),
            InteractionResult::Pass
        );
    }

    #[test]
    fn fills_from_a_dragon_breath_cloud_and_shrinks_it() {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world("bottle_fill_dragon_breath");
        let world = &world_fixture.world;
        let pos = DVec3::new(0.5, 64.0, 0.5);
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let player = TestPlayerBuilder::new(Arc::clone(world), "BreathTester", 1).build();
        player
            .try_set_position(pos)
            .expect("test player should move onto the loaded chunk");

        let dragon = Arc::new(EnderDragonEntity::new(
            &vanilla_entities::ENDER_DRAGON,
            2,
            pos,
            Arc::downgrade(world),
        ));
        let dragon_entity: SharedEntity = dragon;
        let cloud_entity: SharedEntity = AreaEffectCloudEntity::new_at_position(world, pos);
        let Some(cloud) = cloud_entity.downcast_ref::<AreaEffectCloudEntity>() else {
            panic!("spawned entity should be an area effect cloud");
        };
        cloud.set_owner(&dragon_entity);
        assert!(world.try_add_entity(Arc::clone(&cloud_entity)).is_ok());

        assert_eq!(use_bottle(&player, world, 1), InteractionResult::Success);

        let filled = hand_item(&player);
        assert_eq!(filled.item.key, vanilla_items::DRAGON_BREATH.key);
        // Default cloud radius is 3.0; vanilla shrinks it by 0.5 per bottle.
        assert!((cloud.radius() - 2.5).abs() < 1.0e-4);
    }
}
