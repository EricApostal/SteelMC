//! Furnace minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartFurnace`).
//!
//! Stores its burn time (`Fuel` NBT, 3600 ticks per coal/charcoal, capped at
//! 32000) and a horizontal push vector (`PushX`/`PushZ` NBT) applied while
//! fueled. The synced `id_fuel` flag mirrors vanilla `DATA_ID_FUEL`, driving the
//! lit furnace display block and the smoke particles.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, MinecartFurnaceEntityData};
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, TaggedRegistryExt as _, vanilla_blocks, vanilla_items, vanilla_particle_types,
};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::types::InteractionHand;
use steel_utils::{BlockStateId, DowncastType, DowncastTypeKey};

use crate::behavior::InteractionResult;
use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData};
use crate::player::Player;
use crate::world::World;

use super::minecart_base::{self, Minecart, MinecartState, base_natural_slowdown};

/// Vanilla `MinecartFurnace.FUEL_TICKS_PER_ITEM`.
const FUEL_TICKS_PER_ITEM: i32 = 3600;
/// Vanilla `MinecartFurnace.MAX_FUEL_TICKS`.
const MAX_FUEL_TICKS: i32 = 32000;
/// Vanilla `MinecartFurnace.getMaxSpeed` water multiplier.
const WATER_MAX_SPEED_MULTIPLIER: f64 = 0.75;
/// Vanilla `MinecartFurnace.getMaxSpeed` land multiplier.
const LAND_MAX_SPEED_MULTIPLIER: f64 = 0.5;
/// Vanilla smoke gate: `random.nextInt(4) == 0`.
const SMOKE_CHANCE_BOUND: u32 = 4;

/// A coal-powered minecart that pushes itself and other carts.
#[entity_behavior(class = "MinecartFurnace")]
pub struct FurnaceMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<MinecartFurnaceEntityData>,
    state: SyncMutex<MinecartState>,
    furnace: SyncMutex<FurnaceState>,
}

/// Vanilla `MinecartFurnace.fuel`/`push`, which are plain fields rather than
/// synchronized entity data.
#[derive(Debug, Clone, Copy, PartialEq)]
struct FurnaceState {
    /// Remaining fuel in ticks.
    fuel: i32,
    /// Push force while fueled, horizontal only.
    push: DVec3,
}

impl FurnaceState {
    const fn new() -> Self {
        Self {
            fuel: 0,
            push: DVec3::ZERO,
        }
    }
}

// SAFETY: This key is owned by Steel and uniquely identifies `FurnaceMinecartEntity`.
unsafe impl DowncastType for FurnaceMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/furnace_minecart");
}

impl FurnaceMinecartEntity {
    /// Creates a new furnace minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(MinecartFurnaceEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            furnace: SyncMutex::new(FurnaceState::new()),
        }
    }

    /// Creates a furnace minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(MinecartFurnaceEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            furnace: SyncMutex::new(FurnaceState::new()),
        }
    }

    /// Remaining fuel in ticks, vanilla `fuel`.
    #[must_use]
    pub fn fuel(&self) -> i32 {
        self.furnace.lock().fuel
    }

    /// Current push force, vanilla `push` (horizontal only).
    #[must_use]
    pub fn push_force(&self) -> DVec3 {
        self.furnace.lock().push
    }

    /// Whether the cart is burning fuel, vanilla `hasFuel`.
    #[must_use]
    pub fn has_fuel(&self) -> bool {
        *self.entity_data.lock().id_fuel.get()
    }

    /// Sets the synced fuel flag, vanilla `setHasFuel`.
    pub fn set_has_fuel(&self, fuel: bool) {
        self.entity_data.lock().id_fuel.set(fuel);
    }

    /// Feeds the cart, vanilla `addFuel`.
    ///
    /// Returns `true` when `stack` is furnace-minecart fuel and headroom remains.
    pub fn add_fuel(&self, interacting_pos: DVec3, stack: &ItemStack) -> bool {
        if !REGISTRY
            .items
            .is_in_tag(stack.item(), &ItemTag::FURNACE_MINECART_FUEL)
        {
            return false;
        }

        let mut state = self.furnace.lock();
        if state.fuel + FUEL_TICKS_PER_ITEM > MAX_FUEL_TICKS {
            return false;
        }

        state.fuel += FUEL_TICKS_PER_ITEM;
        if state.fuel > 0 {
            let away = self.position() - interacting_pos;
            state.push = DVec3::new(away.x, 0.0, away.z);
        }
        true
    }

    /// Vanilla `MinecartFurnace.interact`: feed the cart with the held item.
    pub fn interact_furnace(&self, player: &Player, hand: InteractionHand) -> InteractionResult {
        let held = player.inventory.lock().get_item_in_hand(hand).clone();
        if self.add_fuel(player.position(), &held) {
            let has_infinite_materials = player.has_infinite_materials();
            player
                .inventory
                .lock()
                .mutate_item_in_hand(hand, |item| item.consume_one(has_infinite_materials));
        }

        InteractionResult::Success
    }

    /// Vanilla `MinecartFurnace.calculateNewPushAlong`.
    ///
    /// Vanilla aligns the stored push to the current movement direction while
    /// keeping its magnitude: `push.projectedOn(movement).normalize().scale(push.length())`.
    /// Both guards use `horizontalDistanceSqr`.
    fn push_along(&self, movement: DVec3) -> DVec3 {
        let push = self.push_force();
        // Vanilla `Vec3.horizontalDistanceSqr`: the squared length ignoring Y.
        let push_horizontal_sqr = push.x * push.x + push.z * push.z;
        let movement_horizontal_sqr = movement.x * movement.x + movement.z * movement.z;
        if push_horizontal_sqr > 1.0e-4 && movement_horizontal_sqr > 0.001 {
            push.project_onto(movement).normalize() * push.length()
        } else {
            push
        }
    }
}

impl Minecart for FurnaceMinecartEntity {
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
        // Vanilla `MinecartFurnace.getDefaultDisplayBlockState`.
        vanilla_blocks::FURNACE
            .default_state()
            .set_value(&BlockStateProperties::FACING, Direction::North)
            .set_value(&BlockStateProperties::LIT, self.has_fuel())
    }

    fn is_furnace(&self) -> bool {
        // Vanilla `MinecartFurnace.isFurnace`.
        true
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::FURNACE_MINECART)
    }

    fn max_speed(&self, world: &World) -> f64 {
        // Vanilla `MinecartFurnace.getMaxSpeed` scales `super.getMaxSpeed`.
        if self.is_in_water() {
            minecart_base::default_max_speed(self, world) * WATER_MAX_SPEED_MULTIPLIER
        } else {
            minecart_base::default_max_speed(self, world) * LAND_MAX_SPEED_MULTIPLIER
        }
    }

    fn apply_natural_slowdown(&self, movement: DVec3) -> DVec3 {
        let mut new_movement = if self.push_force().length_squared() > 1.0e-7 {
            let push = self.push_along(movement);
            self.furnace.lock().push = push;
            DVec3::new(movement.x * 0.8, 0.0, movement.z * 0.8) + push
        } else {
            DVec3::new(movement.x * 0.98, 0.0, movement.z * 0.98)
        };

        if self.is_in_water() {
            new_movement *= 0.1;
        }

        base_natural_slowdown(self, new_movement)
    }
}

impl Entity for FurnaceMinecartEntity {
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

        {
            let mut state = self.furnace.lock();
            if state.fuel > 0 {
                state.fuel -= 1;
            }
            if state.fuel <= 0 {
                state.push = DVec3::ZERO;
            }
        }
        self.set_has_fuel(self.fuel() > 0);

        if self.has_fuel()
            && rand::random_range(0..SMOKE_CHANCE_BOUND) == 0
            && let Some(world) = self.level()
        {
            let position = self.position();
            world.send_particles(
                ParticleData::simple(&vanilla_particle_types::LARGE_SMOKE),
                DVec3::new(position.x, position.y + 0.8, position.z),
                1,
                DVec3::ZERO,
                0.0,
            );
        }
    }

    fn interact(
        &self,
        player: &Player,
        hand: InteractionHand,
        _location: DVec3,
    ) -> InteractionResult {
        self.interact_furnace(player, hand)
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
        let state = self.furnace.lock();
        nbt.insert("PushX", state.push.x);
        nbt.insert("PushZ", state.push.z);
        nbt.insert("Fuel", state.fuel as i16);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        let push_x = nbt.double("PushX").unwrap_or(0.0);
        let push_z = nbt.double("PushZ").unwrap_or(0.0);
        let mut state = self.furnace.lock();
        state.push = DVec3::new(push_x, 0.0, push_z);
        state.fuel = i32::from(nbt.short("Fuel").unwrap_or(0));
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use simdnbt::borrow::read_compound;
    use simdnbt::owned::NbtCompound;
    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_items};

    use crate::entity::EntityFluidContact;

    use super::*;

    fn furnace_cart() -> FurnaceMinecartEntity {
        let cart = FurnaceMinecartEntity::new(
            &vanilla_entities::FURNACE_MINECART,
            1,
            DVec3::new(8.5, 64.0, 8.5),
            Weak::new(),
        );
        // A level-less fixture keeps vanilla's "no fluid contact" default; state it explicitly so
        // `isInWater()` can never be true here.
        cart.base()
            .set_fluid_contact(EntityFluidContact::from_parts(0.0, 0.0, true, false));
        cart
    }

    /// Re-parses saved NBT and hands the borrowed view to `load_additional`, mirroring the
    /// entity reload path.
    fn reload(nbt: &NbtCompound, load: impl FnOnce(BorrowedNbtCompoundView<'_, '_>)) {
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed = read_compound(&mut Cursor::new(&bytes)).expect("test NBT should reborrow");
        load((&borrowed).into());
    }

    #[test]
    fn furnace_cart_reports_is_furnace() {
        // Vanilla `MinecartFurnace.isFurnace`, the knob `pushOtherMinecart` reads.
        assert!(furnace_cart().is_furnace());
    }

    #[test]
    fn add_fuel_takes_the_tagged_items_and_pushes_away_from_the_player() {
        init_vanilla_registry();
        let cart = furnace_cart();
        let player_pos = DVec3::new(4.5, 64.0, 8.5);

        // Vanilla `ItemTags.FURNACE_MINECART_FUEL` is coal and charcoal only.
        assert!(cart.add_fuel(player_pos, &ItemStack::new(&vanilla_items::COAL)));
        assert_eq!(cart.fuel(), FUEL_TICKS_PER_ITEM);
        assert!(cart.add_fuel(player_pos, &ItemStack::new(&vanilla_items::CHARCOAL)));
        assert_eq!(cart.fuel(), 2 * FUEL_TICKS_PER_ITEM);

        // Vanilla `addFuel`: `push = position - interactingPos`, horizontal only.
        assert_eq!(cart.push_force(), DVec3::new(4.0, 0.0, 0.0));

        // Blaze rods and stone are not minecart fuel.
        assert!(!cart.add_fuel(player_pos, &ItemStack::new(&vanilla_items::BLAZE_ROD)));
        assert!(!cart.add_fuel(player_pos, &ItemStack::new(&vanilla_items::STONE)));
        assert_eq!(cart.fuel(), 2 * FUEL_TICKS_PER_ITEM);
    }

    #[test]
    fn add_fuel_rejects_the_item_that_would_exceed_the_cap() {
        init_vanilla_registry();
        let cart = furnace_cart();
        for _ in 0..8 {
            assert!(cart.add_fuel(DVec3::ZERO, &ItemStack::new(&vanilla_items::COAL)));
        }
        // Eight items fit; the ninth would pass the vanilla `MAX_FUEL_TICKS` of 32000.
        assert_eq!(cart.fuel(), 8 * FUEL_TICKS_PER_ITEM);
        assert!(!cart.add_fuel(DVec3::ZERO, &ItemStack::new(&vanilla_items::COAL)));
        assert_eq!(cart.fuel(), 8 * FUEL_TICKS_PER_ITEM);
    }

    #[test]
    fn tick_burns_one_fuel_tick_and_drops_the_push_when_it_runs_out() {
        init_vanilla_registry();
        let cart = Arc::new(furnace_cart());
        let mut nbt = NbtCompound::new();
        nbt.insert("Fuel", 2i16);
        nbt.insert("PushX", 2.0);
        nbt.insert("PushZ", 1.0);
        reload(&nbt, |view| cart.load_additional(view));

        // The synced `id_fuel` flag is only refreshed by the tick itself.
        assert!(!cart.has_fuel());
        Arc::clone(&cart).tick();
        assert_eq!(cart.fuel(), 1);
        assert!(cart.has_fuel());
        assert_eq!(cart.push_force(), DVec3::new(2.0, 0.0, 1.0));

        Arc::clone(&cart).tick();
        assert_eq!(cart.fuel(), 0);
        assert!(!cart.has_fuel());
        assert_eq!(cart.push_force(), DVec3::ZERO);
    }

    #[test]
    fn push_along_keeps_magnitude_and_alignment_like_vanilla() {
        let cart = furnace_cart();
        cart.furnace.lock().push = DVec3::new(1.0, 0.0, 0.0);

        // `push.projectedOn(movement).normalize().scale(push.length())`: the push
        // keeps its length and is turned to face the movement direction, instead
        // of being shortened by the cosine of the angle between them.
        let aligned = cart.push_along(DVec3::new(1.0, 0.0, 1.0));
        assert!((aligned.length() - 1.0).abs() < 1.0e-12);
        assert!((aligned.x - aligned.z).abs() < 1.0e-12);
        assert!(aligned.x > 0.0);

        // Both guards test horizontal distance, so a purely vertical movement
        // leaves the push untouched.
        assert_eq!(
            cart.push_along(DVec3::new(0.0, 1.0, 0.0)),
            DVec3::new(1.0, 0.0, 0.0)
        );

        // A push below `1.0E-4` horizontal distance is returned verbatim.
        cart.furnace.lock().push = DVec3::new(0.005, 0.0, 0.0);
        assert_eq!(
            cart.push_along(DVec3::new(0.0, 0.0, 1.0)),
            DVec3::new(0.005, 0.0, 0.0)
        );
    }
}
