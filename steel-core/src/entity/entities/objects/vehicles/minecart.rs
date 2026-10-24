//! Plain minecart entity (`net.minecraft.world.entity.vehicle.minecart.Minecart`).
//!
//! Rides rails using the shared [`super::minecart_base`] physics, can be boarded
//! by players and mobs, and is ejected/hurt by an active activator rail.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, MinecartEntityData};
use steel_registry::vanilla_items;
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::player::Player;
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState};

/// A plain rideable minecart.
#[entity_behavior(class = "Minecart")]
pub struct MinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<MinecartEntityData>,
    state: SyncMutex<MinecartState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `MinecartEntity`.
unsafe impl DowncastType for MinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/minecart");
}

impl MinecartEntity {
    /// Creates a new minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(MinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
        }
    }

    /// Creates a minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(MinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
        }
    }
}

impl Minecart for MinecartEntity {
    fn cart_state(&self) -> &SyncMutex<MinecartState> {
        &self.state
    }

    fn with_minecart_data<R>(&self, f: impl FnOnce(&AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart())
    }

    fn with_minecart_data_mut<R>(&self, f: impl FnOnce(&mut AbstractMinecartEntityData) -> R) -> R {
        f(self.entity_data.lock().abstract_minecart_mut())
    }

    fn default_display_block_state(&self) -> BlockStateId {
        // Vanilla `getDefaultDisplayBlockState` is air for a plain minecart.
        vanilla_blocks::AIR.default_state()
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::MINECART)
    }

    fn is_rideable(&self) -> bool {
        true
    }

    fn activate_minecart(&self, _world: &Arc<World>, _pos: BlockPos, powered: bool) {
        // Vanilla `Minecart.activateMinecart`: eject riders and wobble the cart.
        if !powered {
            return;
        }

        for passenger in self.passengers().iter().rev() {
            passenger.stop_riding();
        }

        if self.hurt_time() == 0 {
            self.set_hurt_dir(-self.hurt_dir());
            self.set_hurt_time(10);
            self.set_hurt_damage(50.0);
            self.mark_hurt();
        }
    }
}

impl Entity for MinecartEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).minecart_tick();
    }

    fn interact(
        &self,
        player: &Player,
        _hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        // Vanilla `Minecart.interact`: `super.interact` is `Entity.interact`, which is `PASS`
        // for a cart, so only the boarding branch remains. The client-side
        // `playerRotationOffset` interpolation bookkeeping is not mirrored.
        if player.is_secondary_use_active() || self.is_vehicle() {
            return InteractionResult::Pass;
        }

        let Some(world) = self.level() else {
            return InteractionResult::Pass;
        };
        let Some(vehicle) = world.get_entity_by_id(self.id()) else {
            return InteractionResult::Pass;
        };

        if player.start_riding(&vehicle) {
            InteractionResult::SuccessServer
        } else {
            InteractionResult::Consume
        }
    }

    fn push_entity(&self, entity: &dyn Entity) {
        // Vanilla `AbstractMinecart.push`.
        self.minecart_push(entity);
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn is_pushable(&self) -> bool {
        true
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn is_on_rails(&self) -> bool {
        self.is_on_minecart_rails()
    }

    fn get_default_gravity(&self) -> f64 {
        self.minecart_default_gravity()
    }

    fn get_air_drag(&self) -> f64 {
        minecart_base::AIR_DRAG
    }

    fn dimension_changing_delay(&self) -> i32 {
        10
    }

    fn get_relative_portal_position(&self, axis: Axis, portal_area: FoundRectangle) -> DVec3 {
        minecart_base::minecart_relative_portal_position(self, axis, portal_area)
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.minecart_hurt(world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
    }
}

#[cfg(test)]
mod tests {
    use steel_registry::{init_vanilla_registry, vanilla_entities};
    use steel_utils::ChunkPos;

    use crate::entity::{SharedEntity, next_entity_id};
    use crate::player::ResetReason;
    use crate::test_support::{TestPlayerBuilder, fresh_test_world, insert_ready_full_chunk};

    use super::*;

    const CART_POSITION: DVec3 = DVec3::new(8.5, 64.0, 8.5);

    #[test]
    fn sneaking_player_passes_on_a_plain_cart() {
        // Vanilla `Minecart.interact`: `player.isSecondaryUseActive()` returns `PASS` before any
        // boarding attempt, so no world lookup or mount happens.
        init_vanilla_registry();
        let world_fixture = fresh_test_world("minecart_sneak_pass");
        let world = &world_fixture.world;
        let cart = MinecartEntity::new(
            &vanilla_entities::MINECART,
            next_entity_id(),
            CART_POSITION,
            Arc::downgrade(world),
        );
        let player =
            TestPlayerBuilder::new(Arc::clone(world), "CartSneaker", next_entity_id()).build();
        player.set_shared_shift_key_down(true);
        assert!(player.is_secondary_use_active());

        assert_eq!(
            cart.interact(&player, InteractionHand::MainHand, CART_POSITION),
            InteractionResult::Pass
        );
        assert!(cart.passengers().is_empty());
    }

    #[test]
    fn empty_cart_boards_a_player_and_reports_success_server() {
        // Vanilla `Minecart.interact` mounts the player and returns `SUCCESS_SERVER` on the
        // server when `startRiding` succeeded.
        init_vanilla_registry();
        let world_fixture = fresh_test_world("minecart_boardable");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(CART_POSITION));

        let cart = Arc::new(MinecartEntity::new(
            &vanilla_entities::MINECART,
            next_entity_id(),
            CART_POSITION,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&cart) as SharedEntity)
            .expect("cart should attach to the loaded chunk");

        let player =
            TestPlayerBuilder::new(Arc::clone(world), "CartRider", next_entity_id()).build();
        assert!(world.add_player(Arc::clone(&player), ResetReason::InitialJoin));

        assert_eq!(
            cart.interact(&player, InteractionHand::MainHand, CART_POSITION),
            InteractionResult::SuccessServer
        );
        assert_eq!(
            cart.passengers().len(),
            1,
            "the player should be sitting in the cart"
        );

        // Vanilla refuses a second rider while the cart is occupied.
        let other =
            TestPlayerBuilder::new(Arc::clone(world), "CartRider2", next_entity_id()).build();
        assert!(world.add_player(Arc::clone(&other), ResetReason::InitialJoin));
        assert_eq!(
            cart.interact(&other, InteractionHand::MainHand, CART_POSITION),
            InteractionResult::Pass
        );
    }
}
