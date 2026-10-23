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
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
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

    fn with_minecart_data<R>(
        &self,
        f: impl FnOnce(&steel_registry::vanilla_entity_data::AbstractMinecartEntityData) -> R,
    ) -> R {
        f(self.entity_data.lock().abstract_minecart())
    }

    fn with_minecart_data_mut<R>(
        &self,
        f: impl FnOnce(&mut steel_registry::vanilla_entity_data::AbstractMinecartEntityData) -> R,
    ) -> R {
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

    fn hurt(
        &self,
        world: &World,
        source: &crate::entity::damage::DamageSource,
        amount: f32,
    ) -> bool {
        self.minecart_hurt(world, source, amount)
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
    }
}
