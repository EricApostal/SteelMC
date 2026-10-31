//! Vanilla `SculkCatalystBlockEntity`.
//!
//! Vanilla reference: `SculkCatalystBlockEntity`. Reacts immediately to `entity_die` within 8
//! blocks (no vibration travel), converts the victim's experience into sculk charge cursors
//! (`SculkSpreader.createLevelSpreader()`) and pulses the block's `bloom` state for 8 ticks. Every
//! tick `serverTick` runs `SculkSpreader.updateCursors`, which is what grows sculk, sculk veins and
//! the sensor/shrieker growths around the catalyst.

use std::mem;
use std::sync::{Arc, Weak};

use simdnbt::borrow::BaseNbtCompound as BorrowedNbtCompound;
use simdnbt::owned::NbtCompound;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::game_events::GameEventRef;
use steel_registry::{vanilla_block_entity_types, vanilla_game_events};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::blocks::{ChargeCursor, SculkCatalystBlock, SculkSpreader};
use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::world::World;
use crate::world::game_event::vibration::CATALYST_LISTENER_RADIUS;
use crate::world::game_event::{
    GameEventContext, GameEventDeliveryMode, GameEventListener, SharedGameEventListener,
};

/// Vanilla `SculkCatalystBlockEntity.CatalystListener`: immediate `entity_die` handling.
pub struct CatalystListener {
    pos: BlockPos,
    /// Vanilla `CatalystListener.sculkSpreader`, always a level spreader here.
    spreader: SyncMutex<SculkSpreader>,
}

impl CatalystListener {
    /// Vanilla `CatalystListener.handleGameEvent` for `ENTITY_DIE`.
    fn on_entity_death(
        &self,
        world: &Arc<World>,
        context: &GameEventContext<'_>,
        source_pos: glam::DVec3,
    ) -> bool {
        let Some(living) = context
            .source_entity()
            .and_then(|entity| entity.as_living_entity())
        else {
            return false;
        };
        if living.was_experience_consumed() {
            return true;
        }

        // `LivingEntity::experience_reward(&World, Option<&dyn Entity>)`; the killer is not carried
        // through Steel's game-event context, so enchantment/weapon bonuses are not applied.
        let reward = living.experience_reward(world, None);
        if living.should_drop_experience() && reward > 0 {
            // Vanilla `BlockPos.containing(sourcePosition.relative(Direction.UP, 0.5))`.
            let cursor_pos = BlockPos::containing(source_pos.x, source_pos.y + 0.5, source_pos.z);
            self.spreader.lock().add_cursors(cursor_pos, reward);
        }
        // Vanilla `mob.skipDropExperience()`: the death must not also drop XP orbs.
        living.skip_drop_experience();
        let state = world.get_block_state(self.pos);
        SculkCatalystBlock::bloom(state.get_block(), world, self.pos, state);

        if let Some(entity) = world.get_block_entity(self.pos) {
            entity.set_changed();
        }
        true
    }
}

impl GameEventListener for CatalystListener {
    fn listener_pos(&self) -> Option<glam::DVec3> {
        Some(glam::DVec3::new(
            f64::from(self.pos.x()) + 0.5,
            f64::from(self.pos.y()) + 0.5,
            f64::from(self.pos.z()) + 0.5,
        ))
    }

    fn listener_radius(&self) -> i32 {
        CATALYST_LISTENER_RADIUS
    }

    fn delivery_mode(&self) -> GameEventDeliveryMode {
        GameEventDeliveryMode::ByDistance
    }

    fn handle_game_event(
        &self,
        world: &Arc<World>,
        event: GameEventRef,
        context: &GameEventContext<'_>,
        source_pos: glam::DVec3,
    ) -> bool {
        if event.key != vanilla_game_events::ENTITY_DIE.key {
            return false;
        }
        self.on_entity_death(world, context, source_pos)
    }
}

/// Vanilla `SculkCatalystBlockEntity`.
pub struct SculkCatalystBlockEntity {
    base: BlockEntityBase,
    listener: Arc<CatalystListener>,
}

// SAFETY: Steel-owned key uniquely identifies this concrete entity type.
unsafe impl DowncastType for SculkCatalystBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/sculk_catalyst");
}

impl SculkCatalystBlockEntity {
    /// Creates a catalyst block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        Self {
            base: BlockEntityBase::new(
                &vanilla_block_entity_types::SCULK_CATALYST,
                level,
                pos,
                state,
            ),
            listener: Arc::new(CatalystListener {
                pos,
                spreader: SyncMutex::new(SculkSpreader::level()),
            }),
        }
    }

    /// Total unspread charge (sum over vanilla `SculkSpreader` cursors).
    #[must_use]
    pub fn pending_charge(&self) -> i32 {
        self.listener
            .spreader
            .lock()
            .cursors()
            .iter()
            .map(ChargeCursor::charge)
            .sum()
    }

    /// Cursor positions currently carried by the level spreader.
    #[must_use]
    pub fn charge_cursors(&self) -> Vec<BlockPos> {
        self.listener
            .spreader
            .lock()
            .cursors()
            .iter()
            .map(ChargeCursor::pos)
            .collect()
    }

    /// The registered game-event listener.
    #[must_use]
    pub const fn catalyst_listener(&self) -> &Arc<CatalystListener> {
        &self.listener
    }
}

impl BlockEntity for SculkCatalystBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        self.listener.spreader.lock().load(nbt);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.listener.spreader.lock().save(nbt);
    }

    fn tick(&self, world: &Arc<World>) {
        // Vanilla `serverTick` runs `sculkSpreader.updateCursors(level, pos, random, true)`.
        // The cursor list is taken out for the duration of the update so the world mutations
        // below never run while the listener's lock is held.
        let mut spreader = {
            let mut guard = self.listener.spreader.lock();
            if guard.cursors().is_empty() {
                return;
            }
            mem::replace(&mut *guard, SculkSpreader::level())
        };

        let mut rng = rand::rng();
        spreader.update_cursors(world, self.get_block_pos(), &mut rng, true);

        *self.listener.spreader.lock() = spreader;
    }

    fn game_event_listener(&self) -> Option<SharedGameEventListener> {
        Some(Arc::clone(&self.listener) as SharedGameEventListener)
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
    use steel_utils::Downcast as _;
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use crate::behavior::init_behaviors;
    use crate::entity::entities::PigEntity;
    use crate::entity::{Entity, SharedEntity, init_entities};
    use crate::test_support::{fresh_test_world, insert_entity_ticking_chunks, tick_test_world};

    use super::SculkCatalystBlockEntity;

    /// Real-path sculk catalysis: a mob dies next to a catalyst, the catalyst turns its experience
    /// into `entity_die` charge cursors, `serverTick`'s `updateCursors` spreads them into sculk and
    /// sculk veins, and the charge is spent instead of accumulating forever.
    #[test]
    fn catalyst_grows_sculk_from_killed_mob_and_spends_its_charge() {
        init_vanilla_registry();
        init_entities();
        init_behaviors();

        let fixture = fresh_test_world("sculk_catalyst_spread");
        let world = &fixture.world;
        insert_entity_ticking_chunks(world, ChunkPos::new(0, 0), 2);

        // Stone floor so a sculk vein can attach downward and `attemptPlaceSculk` has a
        // `#minecraft:sculk_replaceable` neighbour to convert into sculk. The extra stone block
        // north of the victim gives the vein a second face so it survives the conversion.
        let stone = vanilla_blocks::STONE.default_state();
        for x in -3..=3 {
            for z in -3..=3 {
                world.set_block(BlockPos::new(x, 63, z), stone, UpdateFlags::UPDATE_ALL);
            }
        }
        world.set_block(BlockPos::new(1, 64, 1), stone, UpdateFlags::UPDATE_ALL);

        let catalyst_pos = BlockPos::new(0, 64, 0);
        world.set_block(
            catalyst_pos,
            vanilla_blocks::SCULK_CATALYST.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        let catalyst_block_entity = world
            .get_block_entity(catalyst_pos)
            .expect("placing a catalyst creates its block entity");
        let catalyst = catalyst_block_entity
            .downcast_ref::<SculkCatalystBlockEntity>()
            .expect("the catalyst position holds a SculkCatalystBlockEntity");
        assert_eq!(catalyst.pending_charge(), 0);

        // The victim stands on the floor one block east of the catalyst. Its death position
        // (mob position + 0.5 blocks up, containing) is the air block above the floor.
        let pig = Arc::new(PigEntity::new(
            &vanilla_entities::PIG,
            1,
            DVec3::new(1.5, 64.0, 0.5),
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&pig) as SharedEntity)
            .expect("the victim joins the entity-ticking fixture chunk");
        pig.kill(world);

        let died_at = BlockPos::new(1, 64, 0);
        assert_eq!(
            catalyst.charge_cursors(),
            vec![died_at],
            "the catalyst must record the death position as its cursor"
        );
        let initial_charge = catalyst.pending_charge();
        assert!(
            initial_charge > 0,
            "a killed pig must leave sculk charge behind"
        );

        // Vanilla shape, step one: the first update puts a sculk vein on the cursor block and
        // `attemptPlaceSculk` converts one attached replaceable neighbour into sculk, spending one
        // charge.
        tick_test_world(world, 1, true);
        assert_eq!(
            world.get_block_state(died_at).get_block(),
            &vanilla_blocks::SCULK_VEIN,
            "the cursor must place a sculk vein where the mob died"
        );
        let converted_floor =
            world.get_block_state(BlockPos::new(1, 63, 0)).get_block() == &vanilla_blocks::SCULK;
        let converted_wall =
            world.get_block_state(BlockPos::new(1, 64, 1)).get_block() == &vanilla_blocks::SCULK;
        assert!(
            converted_floor || converted_wall,
            "the vein must convert one of its replaceable neighbours into sculk"
        );
        assert_eq!(
            catalyst.pending_charge(),
            initial_charge - 1,
            "placing sculk costs exactly one charge"
        );

        // Step two: the remaining charge keeps decaying instead of persisting forever.
        let mut ticks = 1;
        while ticks < 400 && catalyst.pending_charge() > 0 {
            tick_test_world(world, 1, true);
            ticks += 1;
        }

        assert!(
            converted_floor || converted_wall,
            "the converted sculk block must survive the spread"
        );
        let grown = (-4..=4)
            .flat_map(|x| {
                (-4..=4).flat_map(move |z| (-2..=2).map(move |y| BlockPos::new(x, 64 + y, z)))
            })
            .filter(|pos| {
                let block = world.get_block_state(*pos).get_block();
                block == &vanilla_blocks::SCULK || block == &vanilla_blocks::SCULK_VEIN
            })
            .count();
        assert!(
            grown >= 1,
            "the catalyst must leave sculk behind, saw {grown} sculk blocks"
        );
        assert_eq!(
            catalyst.pending_charge(),
            0,
            "charge must decay instead of persisting forever"
        );
    }
}
