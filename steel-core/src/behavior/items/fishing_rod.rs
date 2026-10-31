use crate::behavior::{InteractionResult, ItemBehavior, UseItemContext};
use crate::entity::entities::objects::projectiles::FishingHookEntity;
use crate::entity::{Entity, LivingEntity, RemovalReason, SharedEntity, next_entity_id};
use glam::DVec3;
use rand::{RngExt, rng};
use std::sync::Arc;
use steel_macros::item_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::sound_events::{ENTITY_FISHING_BOBBER_RETRIEVE, ENTITY_FISHING_BOBBER_THROW};
use steel_registry::stat::vanilla_stat_types;
use steel_registry::{vanilla_entities, vanilla_game_events, vanilla_items};

/// Behavior for the fishing rod item.
#[item_behavior]
pub struct FishingRodItem;

impl ItemBehavior for FishingRodItem {
    fn use_item(&self, context: &mut UseItemContext) -> InteractionResult {
        let player = context.player;
        let infinite_materials = context.player.has_infinite_materials();
        let world = context.world;
        let item = context.inv.with_item(|item| item.clone());

        let pitch = 0.4 / rng().random_range(0.8..1.2);

        if let Some(fishing) = player.fishing_hook() {
            if item.is(&vanilla_items::FISHING_ROD) {
                let damage = fishing.retrieve(&item);

                context.inv.with_item(|item| {
                    item.hurt_and_break(damage, infinite_materials);
                });
            }

            world.play_sound_at(
                &ENTITY_FISHING_BOBBER_RETRIEVE,
                SoundSource::Neutral,
                player.position(),
                1.0,
                pitch,
                None,
            );
            // Vanilla `FishingRodItem.use`: `stack.causeUseVibration(player, ITEM_INTERACT_FINISH)`.
            player.cause_use_vibration(&item, &vanilla_game_events::ITEM_INTERACT_FINISH);
        } else {
            world.play_sound_at(
                &ENTITY_FISHING_BOBBER_THROW,
                SoundSource::Neutral,
                player.position(),
                0.5,
                pitch,
                None,
            );

            let player_pos = player.position();
            let spawn_pos = DVec3::new(player_pos.x, player.get_eye_y() - 0.1, player_pos.z);

            let luck = item.apply_unconditional_enchantment_value_effects(
                EnchantmentEffectComponent::FishingLuckBonus,
                0.0,
            );

            let lure_speed = item.apply_unconditional_enchantment_value_effects(
                EnchantmentEffectComponent::FishingTimeReduction,
                0.0,
            );

            let hook = Arc::new(FishingHookEntity::new(
                &vanilla_entities::FISHING_BOBBER,
                next_entity_id(),
                spawn_pos,
                Arc::downgrade(world),
            ));

            let player_arc = world
                .players
                .get_by_uuid(&player.gameprofile.id())
                .expect("Failed to obtain `player_arc` for launching fishing hook: `player` must be registered in the world.");

            hook.shoot_from_player(&player_arc, luck as i32, lure_speed as i32);

            let entity: SharedEntity = hook;

            if let Err(error) = world.try_add_entity(Arc::clone(&entity)) {
                entity.set_removed(RemovalReason::Discarded);
                log::error!("Failed to spawn fishing hook: {error}");
                return InteractionResult::Fail;
            }

            player.award_stat(&vanilla_stat_types::ITEM_USED, item.item());

            // Vanilla `FishingRodItem.use`: `stack.causeUseVibration(player, ITEM_INTERACT_START)`.
            player.cause_use_vibration(&item, &vanilla_game_events::ITEM_INTERACT_START);
        }
        InteractionResult::Success
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::game_events::GameEventRef;
    use steel_registry::{item_stack::ItemStack, vanilla_items};
    use steel_utils::types::InteractionHand;
    use steel_utils::{BlockPos, SectionPos};
    use uuid::Uuid;

    use super::*;
    use crate::behavior::UseItemContext;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world};
    use crate::world::World;

    #[test]
    fn retrieving_grounded_hook_does_not_relock_inventory() {
        let world_fixture = fresh_test_world("fishing_rod_grounded_retrieve");
        let world = &world_fixture.world;
        let player = TestPlayerBuilder::new(Arc::clone(world), "Player", 1)
            .uuid(Uuid::from_u128(1))
            .build();
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::FISHING_ROD));

        let player_owner = Arc::clone(&player);
        let owner: SharedEntity = player_owner;
        let hook = Arc::new(FishingHookEntity::new(
            &vanilla_entities::FISHING_BOBBER,
            2,
            DVec3::ZERO,
            Arc::downgrade(world),
        ));
        hook.set_owner(&owner);
        hook.set_on_ground(true);

        let mut context = UseItemContext::new(
            &player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );

        assert_eq!(
            FishingRodItem.use_item(&mut context),
            InteractionResult::Success
        );
        assert!(hook.is_removed());
        assert!(player.fishing_hook().is_none());
        assert_eq!(context.inv.with_item(|item| item.get_damage_value()), 2);
    }

    #[test]
    fn casting_fishing_rod_emits_interact_vibrations() {
        use crate::test_support::insert_ready_full_chunk;
        use crate::world::game_event::{
            GameEventContext, GameEventListener, SharedGameEventListener,
        };
        use steel_utils::ChunkPos;
        use steel_utils::locks::SyncMutex;

        struct RecordingListener {
            pos: DVec3,
            events: Arc<SyncMutex<Vec<GameEventRef>>>,
        }

        impl GameEventListener for RecordingListener {
            fn listener_pos(&self) -> Option<DVec3> {
                Some(self.pos)
            }

            fn listener_radius(&self) -> i32 {
                16
            }

            fn handle_game_event(
                &self,
                _world: &Arc<World>,
                event: GameEventRef,
                _context: &GameEventContext<'_>,
                _source_pos: DVec3,
            ) -> bool {
                self.events.lock().push(event);
                true
            }
        }

        let world_fixture = fresh_test_world("fishing_rod_vibrations");
        let world = &world_fixture.world;
        let pos = DVec3::new(8.0, 64.0, 8.0);
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Player", 20)
            .uuid(Uuid::from_u128(3))
            .build();
        player
            .try_set_position(pos)
            .expect("should position player in center of chunk");
        world.players.insert(Arc::clone(&player));
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::FISHING_ROD));

        let events = Arc::new(SyncMutex::new(Vec::new()));
        let listener: SharedGameEventListener = Arc::new(RecordingListener {
            pos,
            events: Arc::clone(&events),
        });
        world.register_game_event_listener(
            SectionPos::from_block_pos(BlockPos::new(8, 64, 8)),
            listener,
        );

        let mut context = UseItemContext::new(
            &player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );
        assert_eq!(
            FishingRodItem.use_item(&mut context),
            InteractionResult::Success
        );
        assert!(
            events
                .lock()
                .contains(&&vanilla_game_events::ITEM_INTERACT_START)
        );

        // Reeling the hook back in emits the finish vibration.
        let Some(hook) = player.fishing_hook() else {
            panic!("cast should link a hook");
        };
        hook.set_on_ground(true);
        events.lock().clear();
        let mut context = UseItemContext::new(
            &player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );
        assert_eq!(
            FishingRodItem.use_item(&mut context),
            InteractionResult::Success
        );
        assert!(
            events
                .lock()
                .contains(&&vanilla_game_events::ITEM_INTERACT_FINISH)
        );
    }

    #[test]
    fn casting_fishing_rod_spawns_and_links_hook() {
        use crate::test_support::insert_ready_full_chunk;
        use steel_utils::ChunkPos;

        let world_fixture = fresh_test_world("fishing_rod_cast");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));
        let player = TestPlayerBuilder::new(Arc::clone(world), "Player", 10)
            .uuid(Uuid::from_u128(2))
            .build();
        player
            .try_set_position(DVec3::new(8.0, 64.0, 8.0))
            .expect("should position player in center of chunk");
        world.players.insert(Arc::clone(&player));
        player
            .inventory
            .lock()
            .set_selected_item(ItemStack::new(&vanilla_items::FISHING_ROD));

        let mut context = UseItemContext::new(
            &player,
            InteractionHand::MainHand,
            world,
            Arc::clone(&player.inventory),
        );

        assert_eq!(
            FishingRodItem.use_item(&mut context),
            InteractionResult::Success
        );
        assert!(player.fishing_hook().is_some());
    }
}
