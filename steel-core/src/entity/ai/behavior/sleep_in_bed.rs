//! Vanilla `SleepInBed`: sleep in the remembered HOME bed.

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::BlockStateProperties;
use steel_registry::vanilla_block_tags::BlockTag;

use super::interact_with_door::close_doors_that_i_have_opened_or_passed_through;
use crate::entity::ai::brain::behavior::{Behavior, BehaviorContext, BehaviorState};
use crate::entity::ai::brain::memory::MemoryStatus;
use crate::entity::ai::brain::memory::module_type as mem;

/// Vanilla `SleepInBed.COOLDOWN_AFTER_BEING_WOKEN`.
pub const COOLDOWN_AFTER_BEING_WOKEN: i64 = 100;
/// Vanilla `SleepInBed.stop`'s `nextOkStartTime = timestamp + 40`.
const WAKE_COOLDOWN: i64 = 40;
/// Vanilla `SleepInBed.timedOut` returns `false` unconditionally. `BehaviorState`
/// derives its timeout from a duration, so the largest duration stands in for "never";
/// see the report for this deviation.
const NEVER_TIMES_OUT: i32 = i32::MAX;

/// Vanilla `SleepInBed`.
pub struct SleepInBed {
    state: BehaviorState,
    next_ok_start_time: i64,
}

impl SleepInBed {
    /// Vanilla `new SleepInBed()`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: BehaviorState::new(
                vec![
                    (&mem::HOME, MemoryStatus::ValuePresent),
                    (&mem::LAST_WOKEN, MemoryStatus::Registered),
                    (&mem::LAST_SLEPT, MemoryStatus::Registered),
                    (&mem::WALK_TARGET, MemoryStatus::Registered),
                    (&mem::CANT_REACH_WALK_TARGET_SINCE, MemoryStatus::Registered),
                ],
                NEVER_TIMES_OUT,
                NEVER_TIMES_OUT,
            ),
            next_ok_start_time: 0,
        }
    }
}

impl Default for SleepInBed {
    fn default() -> Self {
        Self::new()
    }
}

impl Behavior for SleepInBed {
    fn state(&self) -> &BehaviorState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut BehaviorState {
        &mut self.state
    }

    fn check_extra_start_conditions(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        if ctx.body.is_passenger() {
            return false;
        }

        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return false;
        };
        if ctx.level.key != home.dimension {
            return false;
        }

        if let Some(last_woken) = ctx.get_memory(&mem::LAST_WOKEN) {
            let time_since_last_woken = ctx.timestamp - last_woken;
            if time_since_last_woken > 0 && time_since_last_woken < COOLDOWN_AFTER_BEING_WOKEN {
                return false;
            }
        }

        let bed_state = ctx.level.get_block_state(home.pos);
        home.pos.closer_to_center_than(ctx.body.position(), 2.0)
            && bed_state
                .get_block()
                .has_tag(&BlockTag::VILLAGERS_CAN_SLEEP_ON_BED)
            && !bed_state.get_value(&BlockStateProperties::OCCUPIED)
    }

    fn can_still_use(&self, ctx: &mut BehaviorContext<'_>) -> bool {
        // Vanilla additionally requires the brain's active activity to be `REST`;
        // the activity arbiter applies that gate centrally.
        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return false;
        };

        ctx.body.position().y > f64::from(home.pos.y()) + 0.4
            && home.pos.closer_to_center_than(ctx.body.position(), 1.14)
    }

    fn start(&mut self, ctx: &mut BehaviorContext<'_>) {
        if ctx.timestamp <= self.next_ok_start_time {
            return;
        }

        let Some(home) = ctx.get_memory(&mem::HOME) else {
            return;
        };

        // Vanilla closes the doors the body opened or passed through before sleeping.
        if let Some(mut doors) = ctx.get_memory(&mem::DOORS_TO_CLOSE)
            && let Some(world) = ctx.body.level()
        {
            let nearest = ctx.get_memory(&mem::NEAREST_LIVING_ENTITIES);
            close_doors_that_i_have_opened_or_passed_through(
                &world,
                ctx.body,
                None,
                None,
                &mut doors,
                nearest.as_deref(),
            );
            ctx.set_memory(&mem::DOORS_TO_CLOSE, doors);
        }

        let slept = ctx
            .body
            .as_living_entity()
            .is_some_and(|living| living.start_sleeping(home.pos).is_ok());
        if slept {
            ctx.set_memory(&mem::LAST_SLEPT, ctx.timestamp);
        }

        ctx.erase_memory(&mem::WALK_TARGET);
        ctx.erase_memory(&mem::CANT_REACH_WALK_TARGET_SINCE);
    }

    fn stop(&mut self, ctx: &mut BehaviorContext<'_>) {
        let Some(living) = ctx.body.as_living_entity() else {
            return;
        };
        if living.is_sleeping() {
            living.stop_sleeping();
            self.next_ok_start_time = ctx.timestamp + WAKE_COOLDOWN;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;

    use glam::DVec3;
    use steel_protocol::packet_traits::EncodedPacket;
    use steel_protocol::packets::game::SUseItemOn;
    use steel_registry::blocks::properties::{BedPart, BlockStateProperties};
    use steel_registry::packets::play::C_BLOCK_UPDATE;
    use steel_registry::{
        REGISTRY, init_vanilla_registry, vanilla_blocks, vanilla_entities, vanilla_world_clocks,
    };
    use steel_utils::codec::VarInt;
    use steel_utils::locks::SyncMutex;
    use steel_utils::serial::ReadFrom as _;
    use steel_utils::types::{InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, BlockStateId, ChunkPos, Direction, Downcast as _};

    use super::*;
    use crate::behavior::{BlockHitResult, init_behaviors};
    use crate::entity::ai::brain::activity::Activity;
    use crate::entity::entities::mobs::npc::villager::VillagerEntity;
    use crate::entity::{
        Entity as _, EntitySpawnReason, LivingEntity, Mob, SharedEntity, add_spawned_entity,
        init_entities,
    };
    use crate::player::{Player, PlayerConnection, ResetReason};
    use crate::test_support::{
        TestConnection, TestPlayerBuilder, advance_test_game_time_to, fresh_test_world,
        insert_ready_full_chunk,
    };
    use crate::world::World;

    /// Decodes every `CBlockUpdate` the server sent for `pos`, in order.
    fn sent_block_updates(
        packets: &SyncMutex<Vec<EncodedPacket>>,
        pos: BlockPos,
    ) -> Vec<BlockStateId> {
        packets
            .lock()
            .iter()
            .filter_map(|packet| {
                let mut cursor = Cursor::new(packet.encoded_data.as_slice());
                let _length = VarInt::read(&mut cursor).ok()?;
                if VarInt::read(&mut cursor).ok()?.0 != C_BLOCK_UPDATE {
                    return None;
                }
                let updated = BlockPos::read(&mut cursor).ok()?;
                let state = BlockStateId::read(&mut cursor).ok()?;
                (updated == pos).then_some(state)
            })
            .collect()
    }

    /// Places a stone floor plus a two-block bed with its foot at `foot_pos`, facing
    /// north (so the head sits at `foot_pos` minus one on Z), the way a player places one.
    fn place_floor_and_bed(world: &Arc<World>, foot_pos: BlockPos) -> BlockPos {
        let stone = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::STONE);
        for x in -4..=4 {
            for z in -4..=10 {
                world.set_block(BlockPos::new(x, foot_pos.y() - 1, z), stone, UpdateFlags::UPDATE_ALL);
            }
        }

        let bed = REGISTRY.blocks.get_default_state_id(&vanilla_blocks::RED_BED);
        let facing = Direction::North;
        let head_pos = facing.relative(foot_pos);
        world.set_block(
            foot_pos,
            bed.set_value(&BlockStateProperties::HORIZONTAL_FACING, facing),
            UpdateFlags::UPDATE_ALL,
        );
        world.set_block(
            head_pos,
            bed.set_value(&BlockStateProperties::HORIZONTAL_FACING, facing)
                .set_value(&BlockStateProperties::BED_PART, BedPart::Head),
            UpdateFlags::UPDATE_ALL,
        );

        head_pos
    }

    /// Regression for the player report "when I try to kick a villager out of its bed, it
    /// immediately returns".
    ///
    /// The whole live sequence: a real world whose bed the villager claims as its `HOME`,
    /// the night schedule that puts it in that bed, the real interaction packet a player's
    /// right click produces (`Player::handle_use_item_on` → `BedBlock::use_without_item` →
    /// `AbstractBedBlock.kickVillagerOutOfBed` → `Villager.stopSleeping`), and then world
    /// ticks. Vanilla `Villager.stopSleeping` is `super.stopSleeping()` plus
    /// `brain.setMemory(MemoryModuleType.LAST_WOKEN, level.getGameTime())`, and
    /// `SleepInBed.checkExtraStartConditions` refuses to start again while
    /// `0 < gameTime - LAST_WOKEN < COOLDOWN_AFTER_BEING_WOKEN` (100). Without that memory
    /// the villager stops sleeping and then re-enters the bed two brain ticks later, which
    /// is exactly what the player saw.
    #[test]
    fn woken_villager_keeps_the_vanilla_wake_cooldown() {
        init_vanilla_registry();
        init_entities();
        init_behaviors();

        let fixture = fresh_test_world("villager_woken_from_bed");
        let world = Arc::clone(&fixture.world);
        insert_ready_full_chunk(&world, ChunkPos::new(0, 0));

        // A player keeps the chunk entity-ticking so `tick_game` steps the villager, and is
        // the one who later right-clicks the bed.
        let connection = TestConnection::default();
        let packets = connection.sent_handle();
        let player: Arc<Player> = TestPlayerBuilder::new(Arc::clone(&world), "BedKicker", 1)
            .connection(Arc::new(PlayerConnection::Other(Box::new(connection))))
            .build();
        player.set_client_loaded(true);
        player.base().set_position_local(DVec3::new(2.5, 64.0, 8.5));
        assert!(
            world.add_player(Arc::clone(&player), ResetReason::InitialJoin),
            "the test player must hold the chunk active"
        );

        let foot_pos = BlockPos::new(2, 64, 3);
        let head_pos = place_floor_and_bed(&world, foot_pos);
        assert!(
            world.poi_storage.lock().exists(head_pos, &|_| true),
            "placing a bed must record a HOME POI"
        );

        let villager: SharedEntity = Arc::new(VillagerEntity::new(
            &vanilla_entities::VILLAGER,
            2,
            DVec3::new(2.5, 64.0, 4.2),
            Arc::downgrade(&world),
        ));
        let entity = villager
            .downcast_ref::<VillagerEntity>()
            .expect("villager downcast");
        entity.finalize_spawn(&world, EntitySpawnReason::Natural, None);
        add_spawned_entity(&world, Arc::clone(&villager)).expect("the villager joins the world");

        // Night: the villager schedule's REST window runs from tick 12000 to 24000.
        advance_test_game_time_to(&world, 13_000);
        world.set_clock_total_ticks(&vanilla_world_clocks::OVERWORLD, 13_000);

        world.set_game_rule(&steel_registry::vanilla_game_rules::SPAWN_MOBS, false);
        world.set_game_rule(&steel_registry::vanilla_game_rules::SPAWN_MONSTERS, false);

        let mut ticks = 0_u64;
        while !LivingEntity::is_sleeping(entity) && ticks < 6_000 {
            world.game_time.advance();
            world.tick_game(ticks, true);
            ticks += 1;
        }

        assert!(
            Mob::brain(entity)
                .expect("a villager has a brain")
                .lock()
                .is_active(Activity::Rest),
            "the schedule must be in REST for this test to mean anything"
        );
        assert!(
            LivingEntity::is_sleeping(entity),
            "the villager must claim the bed and sleep in it (ticked {ticks})"
        );
        let occupied_state = world.get_block_state(head_pos);
        assert!(
            occupied_state.get_value(&BlockStateProperties::OCCUPIED),
            "a sleeping villager marks its bed occupied"
        );

        // The real wake a player performs: right-click the occupied bed.
        player.base().set_position_local(DVec3::new(2.5, 64.0, 6.5));
        player.handle_use_item_on(SUseItemOn {
            hand: InteractionHand::MainHand,
            block_hit: BlockHitResult {
                location: DVec3::new(2.5, 64.5, 2.5),
                direction: Direction::Up,
                block_pos: head_pos,
                miss: false,
                inside: false,
                world_border_hit: false,
            },
            sequence: 1,
        });

        assert!(
            !LivingEntity::is_sleeping(entity),
            "kicking the villager out of its bed must wake it"
        );
        let freed_state = world.get_block_state(head_pos);
        assert!(
            !freed_state.get_value(&BlockStateProperties::OCCUPIED),
            "the kicked-out villager's bed must be free again"
        );
        assert!(
            sent_block_updates(&packets, head_pos).contains(&freed_state),
            "the client must be told the bed is free again"
        );

        let woken_at = world.game_time();
        assert_eq!(
            Mob::brain(entity)
                .expect("a villager has a brain")
                .lock()
                .get_memory(&mem::LAST_WOKEN),
            Some(woken_at),
            "vanilla `Villager.stopSleeping` records LAST_WOKEN, which is the whole wake cooldown"
        );

        // `SleepInBed` stops the tick after the wake and may start again on the tick after
        // that; for `COOLDOWN_AFTER_BEING_WOKEN` ticks it must not.
        for _ in 0..COOLDOWN_AFTER_BEING_WOKEN {
            world.game_time.advance();
            world.tick_game(ticks, true);
            ticks += 1;
            assert!(
                !LivingEntity::is_sleeping(entity),
                "the woken villager went straight back to bed (ticked {ticks} since the kick)"
            );
        }

        // The cooldown delays the villager, it does not keep it out of bed: once the
        // `LAST_WOKEN` window has passed and REST is still the scheduled activity, the
        // villager goes back to sleep.
        while !LivingEntity::is_sleeping(entity) && ticks < 12_000 {
            world.game_time.advance();
            world.tick_game(ticks, true);
            ticks += 1;
        }
        assert!(
            LivingEntity::is_sleeping(entity),
            "after the wake cooldown the villager must be able to sleep again (ticked {ticks})"
        );
    }
}
