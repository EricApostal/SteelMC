//! Shared vanilla `AbstractMinecart` + `OldMinecartBehavior` mechanics.
//!
//! Every minecart variant in this module family reuses these functions for rail
//! riding, display-block state, wobble/damage, slowdown, derailing, and portal
//! reuse, mirroring vanilla's `AbstractMinecart` base class.
//!
//! Deliberate differences from vanilla:
//! - Vanilla switches to the experimental `NewMinecartBehavior` when the
//!   `minecart_improvements` feature flag is enabled; Steel always uses the
//!   stable `OldMinecartBehavior` physics implemented here.
//! - Client-only branches (interpolation start, `reapplyPosition`, rider
//!   rotation offsets) are server-irrelevant and are not mirrored.
//! - Vanilla's `EntitySelector.pushableBy` honors team collision rules, which
//!   Steel does not model yet; the push/pickup filter here uses vanilla's
//!   pickable/pushable/spectator test instead.

use std::sync::Arc;

use glam::{DVec3, IVec3};
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_math::{RAD_TO_DEG_F64, wrap_degrees};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, RailShape};
use steel_registry::data_components::vanilla_components;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_entity_data::AbstractMinecartEntityData;
use steel_registry::{vanilla_blocks, vanilla_game_events, vanilla_game_rules};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, Downcast as _};

use crate::block_entity::block_state_nbt;
use crate::entity::damage::DamageSource;
use crate::entity::entities::mobs::golem::IronGolemEntity;
use crate::entity::{Entity, RemovalReason, reset_forward_direction_of_relative_portal_position};
use crate::physics::MoverType;
use crate::portal::portal_shape::PortalShape;
use crate::world::{LevelReader as _, SignalGetter, World};

use super::chest_minecart::ChestMinecartEntity;
use super::command_block_minecart::CommandBlockMinecartEntity;
use super::furnace_minecart::FurnaceMinecartEntity;
use super::hopper_minecart::HopperMinecartEntity;
use super::minecart::MinecartEntity;
use super::spawner_minecart::SpawnerMinecartEntity;
use super::tnt_minecart::TntMinecartEntity;

/// Vanilla `OldMinecartBehavior.MAX_SPEED_ON_LAND`, in blocks per tick.
const MAX_SPEED_ON_LAND: f64 = 0.4;
/// Vanilla `OldMinecartBehavior.MAX_SPEED_IN_WATER`, in blocks per tick.
const MAX_SPEED_IN_WATER: f64 = 0.2;

/// Vanilla `OldMinecartBehavior.getMaxSpeed`: `0.4` blocks per tick on land and
/// `0.2` in water.
///
/// Vanilla reads the `max_minecart_speed` game rule only from the experimental
/// `NewMinecartBehavior`, which is gated behind the off-by-default
/// `minecart_improvements` feature flag. Steel always uses the stable behaviour
/// (see the module docs), so — exactly as in vanilla — the rule has no effect on
/// minecart speed.
pub(super) fn default_max_speed(cart: &impl Minecart, _world: &World) -> f64 {
    if cart.is_in_water() {
        MAX_SPEED_IN_WATER
    } else {
        MAX_SPEED_ON_LAND
    }
}
/// Vanilla `AbstractMinecart.WATER_SLOWDOWN_FACTOR`.
pub(super) const WATER_SLOWDOWN_FACTOR: f64 = 0.95;
/// Vanilla `AbstractMinecart.getAirDrag`.
pub(super) const AIR_DRAG: f64 = 0.95;
/// Vanilla `AbstractMinecart.getDefaultDisplayOffset`.
pub(super) const DEFAULT_DISPLAY_OFFSET: i32 = 6;
/// Vanilla `AbstractMinecart.getDefaultGravity` on land.
const LAND_GRAVITY: f64 = 0.04;
/// Vanilla `AbstractMinecart.getDefaultGravity` in water.
const WATER_GRAVITY: f64 = 0.005;
/// Vanilla `Entity.lavaIgnite` burn duration in ticks (15 seconds).
const LAVA_IGNITE_TICKS: i32 = 300;
/// Vanilla `VehicleEntity.hurtServer` damage threshold for destroying a vehicle.
const DESTROY_DAMAGE_THRESHOLD: f32 = 40.0;

/// Rail contact and rotation state shared by every vanilla minecart.
///
/// Mirrors vanilla `AbstractMinecart.onRails`/`flipped`, which are plain fields
/// rather than synchronized entity data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct MinecartState {
    on_rails: bool,
    flipped: bool,
}

impl MinecartState {
    /// Creates the vanilla default minecart state.
    pub(super) const fn new() -> Self {
        Self {
            on_rails: false,
            flipped: false,
        }
    }
}

/// Vanilla `BaseRailBlock.isRail` using the accessible block-tag test.
pub(super) fn is_rail_state(state: BlockStateId) -> bool {
    state.get_block().has_tag(&BlockTag::RAILS)
}

/// Returns the rail shape of a rail state, when the block carries the property.
fn rail_shape(state: BlockStateId) -> Option<RailShape> {
    state.try_get_value(&BlockStateProperties::RAIL_SHAPE)
}

/// Vanilla `AbstractMinecart.EXITS`: the two track endpoints for a rail shape.
///
/// Endpoints are block-relative offsets; a non-zero Y means that endpoint is one
/// block up or down.
pub(super) const fn exits(shape: RailShape) -> (IVec3, IVec3) {
    let x_neg = IVec3::new(-1, 0, 0);
    let x_pos = IVec3::new(1, 0, 0);
    let z_neg = IVec3::new(0, 0, -1);
    let z_pos = IVec3::new(0, 0, 1);
    let x_neg_below = IVec3::new(-1, -1, 0);
    let x_pos_below = IVec3::new(1, -1, 0);
    let z_neg_below = IVec3::new(0, -1, -1);
    let z_pos_below = IVec3::new(0, -1, 1);

    match shape {
        RailShape::NorthSouth => (z_neg, z_pos),
        RailShape::EastWest => (x_neg, x_pos),
        RailShape::AscendingEast => (x_neg_below, x_pos),
        RailShape::AscendingWest => (x_neg, x_pos_below),
        RailShape::AscendingNorth => (z_neg, z_pos_below),
        RailShape::AscendingSouth => (z_neg_below, z_pos),
        RailShape::SouthEast => (z_pos, x_pos),
        RailShape::SouthWest => (z_pos, x_neg),
        RailShape::NorthWest => (z_neg, x_neg),
        RailShape::NorthEast => (z_neg, x_pos),
    }
}

/// Vanilla `OldMinecartBehavior.getPos`: snaps a position onto the rail under it.
pub(super) fn rail_position(world: &World, x: f64, y: f64, z: f64) -> Option<DVec3> {
    let xt = x.floor() as i32;
    let mut yt = y.floor() as i32;
    let zt = z.floor() as i32;
    if is_rail_state(world.get_block_state(BlockPos::new(xt, yt - 1, zt))) {
        yt -= 1;
    }

    let state = world.get_block_state(BlockPos::new(xt, yt, zt));
    if !is_rail_state(state) {
        return None;
    }
    let shape = rail_shape(state)?;

    let (exit0, exit1) = exits(shape);
    let x0 = f64::from(xt) + 0.5 + f64::from(exit0.x) * 0.5;
    let y0 = f64::from(yt) + 0.0625 + f64::from(exit0.y) * 0.5;
    let z0 = f64::from(zt) + 0.5 + f64::from(exit0.z) * 0.5;
    let x1 = f64::from(xt) + 0.5 + f64::from(exit1.x) * 0.5;
    let y1 = f64::from(yt) + 0.0625 + f64::from(exit1.y) * 0.5;
    let z1 = f64::from(zt) + 0.5 + f64::from(exit1.z) * 0.5;

    let xd = x1 - x0;
    let yd = (y1 - y0) * 2.0;
    let zd = z1 - z0;
    let progress = if xd == 0.0 {
        z - f64::from(zt)
    } else if zd == 0.0 {
        x - f64::from(xt)
    } else {
        let xx = x - x0;
        let zz = z - z0;
        (xx * xd + zz * zd) * 2.0
    };

    let mut y = y0 + yd * progress;
    if yd < 0.0 {
        y += 1.0;
    } else if yd > 0.0 {
        y += 0.5;
    }

    Some(DVec3::new(x0 + xd * progress, y, z0 + zd * progress))
}

/// Steel downcast test for vanilla's `instanceof AbstractMinecart`.
pub(super) fn is_minecart_entity(entity: &dyn Entity) -> bool {
    entity.is::<MinecartEntity>()
        || entity.is::<ChestMinecartEntity>()
        || entity.is::<FurnaceMinecartEntity>()
        || entity.is::<HopperMinecartEntity>()
        || entity.is::<TntMinecartEntity>()
        || entity.is::<SpawnerMinecartEntity>()
        || entity.is::<CommandBlockMinecartEntity>()
}

/// Steel downcast test for vanilla's `AbstractMinecart.isFurnace`.
pub(super) fn is_furnace_entity(entity: &dyn Entity) -> bool {
    entity.is::<FurnaceMinecartEntity>()
}

/// Vanilla `EntitySelector.pushableBy` without the unimplemented team rules.
fn is_pushable_by(entity: &dyn Entity, cart: &dyn Entity) -> bool {
    entity.is_pickable()
        && entity.is_pushable()
        && !entity.is_spectator()
        && !entity.is_passenger_of_same_vehicle(cart)
}

/// Vanilla `AbstractMinecart.applyNaturalSlowdown`, shared by carts that override
/// the slowdown with their own pre-scaling (furnace, container carts).
pub(super) fn base_natural_slowdown(cart: &impl Minecart, movement: DVec3) -> DVec3 {
    let factor = cart.slowdown_factor();
    let new_movement = DVec3::new(movement.x * factor, 0.0, movement.z * factor);
    if cart.is_in_water() {
        new_movement * WATER_SLOWDOWN_FACTOR
    } else {
        new_movement
    }
}

/// Vanilla `AbstractMinecart`/`VehicleEntity` state and behavior shared by all carts.
///
/// Implementors only supply their synchronized-data plumbing and the handful of
/// variant-specific knobs (`default_display_block_state`, `max_speed`,
/// `apply_natural_slowdown`, `activate_minecart`, `drop_item`, `is_rideable`).
#[expect(
    dead_code,
    reason = "vanilla `AbstractMinecart.getDefaultDisplayBlockState`/`getDisplayBlockState`/`getRedstoneDirection`: every cart variant implements the default and the shared redstone helpers, but Steel has no client sync path reading them yet"
)]
pub(super) trait Minecart: Entity + Sized {
    /// Per-cart rail contact and rotation state.
    fn cart_state(&self) -> &SyncMutex<MinecartState>;

    /// Runs `f` against the shared `AbstractMinecart` synchronized-data layer.
    fn with_minecart_data<R>(&self, f: impl FnOnce(&AbstractMinecartEntityData) -> R) -> R;

    /// Runs `f` against the shared `AbstractMinecart` synchronized-data layer mutably.
    fn with_minecart_data_mut<R>(&self, f: impl FnOnce(&mut AbstractMinecartEntityData) -> R) -> R;

    /// Vanilla `getDefaultDisplayBlockState`.
    fn default_display_block_state(&self) -> BlockStateId;

    /// Vanilla `getDefaultDisplayOffset`.
    fn default_display_offset(&self) -> i32 {
        DEFAULT_DISPLAY_OFFSET
    }

    /// Vanilla `getDropItem`, used when the cart is destroyed.
    fn drop_item(&self) -> Option<ItemRef> {
        None
    }

    /// Vanilla `isRideable`.
    fn is_rideable(&self) -> bool {
        false
    }

    /// Vanilla `AbstractMinecart.isFurnace`; only the furnace cart overrides it.
    fn is_furnace(&self) -> bool {
        false
    }

    /// Vanilla `AbstractMinecart.push` for the behavior Steel always uses.
    ///
    /// Vanilla routes cart-to-cart contact through `pushOtherMinecart`; every other entity gets
    /// the generic impulse push. Emit this from the variant's `Entity::push_entity` override.
    fn minecart_push(&self, entity: &dyn Entity) {
        if entity.no_physics() || self.no_physics() || self.has_passenger(entity) {
            return;
        }

        let mut x = entity.position().x - self.position().x;
        let mut z = entity.position().z - self.position().z;
        let mut distance = x * x + z * z;
        if distance < 1.0e-4 {
            return;
        }

        distance = distance.sqrt();
        x /= distance;
        z /= distance;
        let power = (1.0 / distance).min(1.0);
        // Vanilla: `xa *= 0.1F; za *= 0.1F; xa *= 0.5; za *= 0.5`.
        x *= power * 0.05;
        z *= power * 0.05;

        if is_minecart_entity(entity) {
            self.push_other_minecart(entity, x, z);
        } else {
            self.push_impulse(DVec3::new(-x, 0.0, -z));
            entity.push_impulse(DVec3::new(x / 4.0, 0.0, z / 4.0));
        }
    }

    /// Vanilla `AbstractMinecart.pushOtherMinecart` for the old behavior, including the
    /// furnace asymmetries.
    fn push_other_minecart(&self, other: &dyn Entity, xa: f64, za: f64) {
        let xo = other.position().x - self.position().x;
        let zo = other.position().z - self.position().z;
        let dir = DVec3::new(xo, 0.0, zo).normalize();
        let (yaw, _) = self.rotation();
        let (sin, cos) = yaw.to_radians().sin_cos();
        let facing = DVec3::new(f64::from(cos), 0.0, f64::from(sin)).normalize();
        if dir.dot(facing).abs() < 0.8 {
            return;
        }

        let movement = self.velocity();
        let other_movement = other.velocity();
        let other_is_furnace = is_furnace_entity(other);
        let self_is_furnace = self.is_furnace();

        if other_is_furnace && !self_is_furnace {
            self.set_velocity(DVec3::new(movement.x * 0.2, movement.y, movement.z * 0.2));
            self.push_impulse(DVec3::new(
                other_movement.x - xa,
                0.0,
                other_movement.z - za,
            ));
            other.set_velocity(DVec3::new(
                other_movement.x * 0.95,
                other_movement.y,
                other_movement.z * 0.95,
            ));
        } else if !other_is_furnace && self_is_furnace {
            other.set_velocity(DVec3::new(
                other_movement.x * 0.2,
                other_movement.y,
                other_movement.z * 0.2,
            ));
            other.push_impulse(DVec3::new(movement.x + xa, 0.0, movement.z + za));
            self.set_velocity(DVec3::new(movement.x * 0.95, movement.y, movement.z * 0.95));
        } else {
            let xdd = (other_movement.x + movement.x) / 2.0;
            let zdd = (other_movement.z + movement.z) / 2.0;
            self.set_velocity(DVec3::new(movement.x * 0.2, movement.y, movement.z * 0.2));
            self.push_impulse(DVec3::new(xdd - xa, 0.0, zdd - za));
            other.set_velocity(DVec3::new(
                other_movement.x * 0.2,
                other_movement.y,
                other_movement.z * 0.2,
            ));
            other.push_impulse(DVec3::new(xdd + xa, 0.0, zdd + za));
        }
    }

    /// Vanilla `AbstractMinecart.getMaxSpeed` for the behaviour Steel always uses.
    fn max_speed(&self, world: &World) -> f64 {
        default_max_speed(self, world)
    }

    /// Vanilla `OldMinecartBehavior.getSlowdownFactor`.
    fn slowdown_factor(&self) -> f64 {
        if self.is_vehicle() { 0.997 } else { 0.96 }
    }

    /// Vanilla `AbstractMinecart.applyNaturalSlowdown`.
    fn apply_natural_slowdown(&self, movement: DVec3) -> DVec3 {
        base_natural_slowdown(self, movement)
    }

    /// Vanilla `AbstractMinecart.activateMinecart` for activator rails.
    fn activate_minecart(&self, _world: &Arc<World>, _pos: BlockPos, _powered: bool) {}

    /// Extra work performed after the cart's drop item has been spawned.
    fn on_minecart_destroyed(&self, _world: &World) {}

    /// Vanilla `AbstractMinecart.getDefaultGravity`.
    fn minecart_default_gravity(&self) -> f64 {
        if self.is_in_water() {
            WATER_GRAVITY
        } else {
            LAND_GRAVITY
        }
    }

    /// Whether the cart currently stands on a rail block.
    fn is_on_minecart_rails(&self) -> bool {
        self.cart_state().lock().on_rails
    }

    /// Sets rail contact, mirroring vanilla `setOnRails`.
    fn set_on_minecart_rails(&self, on_rails: bool) {
        self.cart_state().lock().on_rails = on_rails;
    }

    /// Whether the cart's rotation is flipped, mirroring vanilla `isFlipped`.
    fn is_flipped(&self) -> bool {
        self.cart_state().lock().flipped
    }

    /// Sets the flipped rotation flag, mirroring vanilla `setFlipped`.
    fn set_flipped(&self, flipped: bool) {
        self.cart_state().lock().flipped = flipped;
    }

    /// The custom display block state, when one was set.
    fn custom_display_block_state(&self) -> Option<BlockStateId> {
        self.with_minecart_data(|data| *data.id_custom_display_block.get())
    }

    /// Sets the custom display block state.
    fn set_custom_display_block_state(&self, state: Option<BlockStateId>) {
        self.with_minecart_data_mut(|data| data.id_custom_display_block.set(state));
    }

    /// Vanilla `getDisplayBlockState`, falling back to the cart's default.
    fn display_block_state(&self) -> BlockStateId {
        self.custom_display_block_state()
            .unwrap_or_else(|| self.default_display_block_state())
    }

    /// The synchronized display-block vertical offset.
    fn display_offset(&self) -> i32 {
        self.with_minecart_data(|data| *data.id_display_offset.get())
    }

    /// Sets the synchronized display-block vertical offset.
    fn set_display_offset(&self, offset: i32) {
        self.with_minecart_data_mut(|data| data.id_display_offset.set(offset));
    }

    /// Vanilla `VehicleEntity.getHurtTime`.
    fn hurt_time(&self) -> i32 {
        self.with_minecart_data(|data| *data.vehicle_entity.id_hurt.get())
    }

    /// Vanilla `VehicleEntity.setHurtTime`.
    fn set_hurt_time(&self, hurt_time: i32) {
        self.with_minecart_data_mut(|data| data.vehicle_entity.id_hurt.set(hurt_time));
    }

    /// Vanilla `VehicleEntity.getHurtDir`.
    fn hurt_dir(&self) -> i32 {
        self.with_minecart_data(|data| *data.vehicle_entity.id_hurtdir.get())
    }

    /// Vanilla `VehicleEntity.setHurtDir`.
    fn set_hurt_dir(&self, hurt_dir: i32) {
        self.with_minecart_data_mut(|data| data.vehicle_entity.id_hurtdir.set(hurt_dir));
    }

    /// Vanilla `VehicleEntity.getDamage`.
    fn hurt_damage(&self) -> f32 {
        self.with_minecart_data(|data| *data.vehicle_entity.id_damage.get())
    }

    /// Vanilla `VehicleEntity.setDamage`.
    fn set_hurt_damage(&self, damage: f32) {
        self.with_minecart_data_mut(|data| data.vehicle_entity.id_damage.set(damage));
    }

    /// Vanilla `VehicleEntity.hurtServer` wobble: the cart absorbs
    /// `amount * 10` damage and is destroyed once its accumulated damage passes
    /// `40.0`.
    ///
    /// Vanilla's `AbstractMinecart.animateHurt(float)` looks similar but is
    /// client-only (called from `ClientPacketListener` on the hurt packet) and
    /// scales the *existing* damage instead of the incoming amount, so it must
    /// not be used here.
    fn mark_minecart_hurt(&self, amount: f32) {
        self.set_hurt_dir(-self.hurt_dir());
        self.set_hurt_time(10);
        self.mark_hurt();
        self.set_hurt_damage(self.hurt_damage() + amount * 10.0);
    }

    /// Vanilla `AbstractMinecart.getCurrentBlockPosOrRailBelow` for the old behavior.
    fn current_block_pos_or_rail_below(&self) -> BlockPos {
        let Some(world) = self.level() else {
            return self.block_position();
        };
        let position = self.position();
        let xt = position.x.floor() as i32;
        let mut yt = position.y.floor() as i32;
        let zt = position.z.floor() as i32;
        if is_rail_state(world.get_block_state(BlockPos::new(xt, yt - 1, zt))) {
            yt -= 1;
        }
        BlockPos::new(xt, yt, zt)
    }

    /// Vanilla `AbstractMinecart.isRedstoneConductor`.
    fn is_redstone_conductor_at(&self, pos: BlockPos) -> bool {
        let Some(world) = self.level() else {
            return false;
        };
        let state = world.get_block_state(pos);
        SignalGetter::is_redstone_conductor(&*world, state, pos)
    }

    /// Vanilla `AbstractMinecart.getRedstoneDirection` for powered rails.
    fn redstone_direction(&self, pos: BlockPos) -> DVec3 {
        let Some(world) = self.level() else {
            return DVec3::ZERO;
        };
        let state = world.get_block_state(pos);
        if state.get_block() != &vanilla_blocks::POWERED_RAIL
            || !state.get_value(&BlockStateProperties::POWERED)
        {
            return DVec3::ZERO;
        }

        match rail_shape(state) {
            Some(RailShape::EastWest) => {
                if self.is_redstone_conductor_at(pos.west()) {
                    DVec3::new(1.0, 0.0, 0.0)
                } else if self.is_redstone_conductor_at(pos.east()) {
                    DVec3::new(-1.0, 0.0, 0.0)
                } else {
                    DVec3::ZERO
                }
            }
            Some(RailShape::NorthSouth) => {
                if self.is_redstone_conductor_at(pos.north()) {
                    DVec3::new(0.0, 0.0, 1.0)
                } else if self.is_redstone_conductor_at(pos.south()) {
                    DVec3::new(0.0, 0.0, -1.0)
                } else {
                    DVec3::ZERO
                }
            }
            _ => DVec3::ZERO,
        }
    }

    /// Saves the `AbstractMinecart` fields into the entity's type-specific NBT.
    fn save_minecart_data(&self, nbt: &mut NbtCompound) {
        if let Some(state) = self.custom_display_block_state() {
            nbt.insert(
                "DisplayState",
                NbtTag::Compound(block_state_nbt::save(state)),
            );
        }

        let offset = self.display_offset();
        if offset != self.default_display_offset() {
            nbt.insert("DisplayOffset", offset);
        }

        nbt.insert("FlippedRotation", i8::from(self.is_flipped()));
        nbt.insert("HasTicked", i8::from(self.is_first_tick()));
    }

    /// Loads the `AbstractMinecart` fields from the entity's type-specific NBT.
    fn load_minecart_data(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let display_state = nbt.compound("DisplayState").and_then(block_state_nbt::load);
        self.set_custom_display_block_state(display_state);
        self.set_display_offset(
            nbt.int("DisplayOffset")
                .unwrap_or_else(|| self.default_display_offset()),
        );
        self.set_flipped(nbt.byte("FlippedRotation").is_some_and(|value| value != 0));
        self.set_first_tick(nbt.byte("HasTicked").is_some_and(|value| value != 0));
    }

    /// Vanilla `AbstractMinecart.tick` plus `OldMinecartBehavior.tick` server branch.
    fn minecart_tick(self: Arc<Self>) {
        let hurt_time = self.hurt_time();
        if hurt_time > 0 {
            self.set_hurt_time(hurt_time - 1);
        }

        let damage = self.hurt_damage();
        if damage > 0.0 {
            self.set_hurt_damage(damage - 1.0);
        }

        self.check_below_world();
        self.base().compute_known_speed();
        self.handle_portal();
        if let Some(world) = self.level() {
            Arc::clone(&self).minecart_behavior_tick(&world);
        }
        let _ = self.refresh_fluid_contact();
        if self.is_in_lava() {
            self.ignite_for_ticks(LAVA_IGNITE_TICKS);
            self.lava_hurt();
            self.set_fall_distance(self.fall_distance() * 0.5);
        }

        self.set_first_tick(false);
    }

    /// Vanilla `OldMinecartBehavior.tick` server branch.
    fn minecart_behavior_tick(self: Arc<Self>, world: &Arc<World>) {
        self.apply_gravity();
        let rail_pos = self.current_block_pos_or_rail_below();
        let state = world.get_block_state(rail_pos);
        let on_rails = is_rail_state(state);
        self.set_on_minecart_rails(on_rails);

        if on_rails {
            Arc::clone(&self).move_along_track(world);
            if state.get_block() == &vanilla_blocks::ACTIVATOR_RAIL {
                self.activate_minecart(
                    world,
                    rail_pos,
                    state.get_value(&BlockStateProperties::POWERED),
                );
            }
        } else {
            Arc::clone(&self).come_off_track(world);
        }

        self.apply_effects_from_blocks();

        let old_position = self.old_position();
        let position = self.position();
        let (yaw, _) = self.rotation();
        let mut new_yaw = yaw;
        let x_diff = old_position.x - position.x;
        let z_diff = old_position.z - position.z;
        if x_diff * x_diff + z_diff * z_diff > 0.001 {
            new_yaw = (z_diff.atan2(x_diff) * RAD_TO_DEG_F64) as f32;
            if self.is_flipped() {
                new_yaw += 180.0;
            }
        }

        let old_yaw = self.base().old_rotation().0;
        let rot_diff = wrap_degrees(new_yaw - old_yaw);
        if !(-170.0..170.0).contains(&rot_diff) {
            new_yaw += 180.0;
            self.set_flipped(!self.is_flipped());
        }

        self.set_rotation((new_yaw % 360.0, 0.0));
        Arc::clone(&self).push_and_pickup_entities();
    }

    /// Vanilla `OldMinecartBehavior.moveAlongTrack`.
    #[expect(
        clippy::too_many_lines,
        reason = "vanilla `OldMinecartBehavior.moveAlongTrack` mirrors the Java method one-to-one"
    )]
    fn move_along_track(self: Arc<Self>, world: &Arc<World>) {
        let pos = self.current_block_pos_or_rail_below();
        let state = world.get_block_state(pos);
        let Some(shape) = rail_shape(state) else {
            return;
        };

        self.reset_fall_distance();
        let position = self.position();
        let old_pos = rail_position(world, position.x, position.y, position.z);
        let mut y = f64::from(pos.y());

        let mut power_track = false;
        let mut halt_track = false;
        if state.get_block() == &vanilla_blocks::POWERED_RAIL {
            power_track = state.get_value(&BlockStateProperties::POWERED);
            halt_track = !power_track;
        }

        let mut slide_speed = 0.007_812_5;
        if self.is_in_water() {
            slide_speed *= 0.2;
        }

        let movement = self.velocity();
        match shape {
            RailShape::AscendingEast => {
                self.set_velocity(movement + DVec3::new(-slide_speed, 0.0, 0.0));
            }
            RailShape::AscendingWest => {
                self.set_velocity(movement + DVec3::new(slide_speed, 0.0, 0.0));
            }
            RailShape::AscendingNorth => {
                self.set_velocity(movement + DVec3::new(0.0, 0.0, slide_speed));
            }
            RailShape::AscendingSouth => {
                self.set_velocity(movement + DVec3::new(0.0, 0.0, -slide_speed));
            }
            _ => {}
        }
        if shape.is_slope() {
            y += 1.0;
        }

        let (exit0, exit1) = exits(shape);
        let mut xd = f64::from(exit1.x - exit0.x);
        let mut zd = f64::from(exit1.z - exit0.z);
        let length = (xd * xd + zd * zd).sqrt();
        let movement = self.velocity();
        if movement.x * xd + movement.z * zd < 0.0 {
            xd = -xd;
            zd = -zd;
        }

        let horizontal = (movement.x * movement.x + movement.z * movement.z).sqrt();
        let pow = horizontal.min(2.0);
        self.set_velocity(DVec3::new(pow * xd / length, movement.y, pow * zd / length));

        if let Some(passenger) = self.first_passenger()
            && let Some(player) = passenger.as_player()
        {
            let move_intent = player.last_client_input().movement_input();
            let velocity = self.velocity();
            let horizontal_sqr = velocity.x * velocity.x + velocity.z * velocity.z;
            if move_intent.length_squared() > 0.0 && horizontal_sqr < 0.01 {
                self.set_velocity(
                    velocity + DVec3::new(move_intent.x * 0.001, 0.0, move_intent.z * 0.001),
                );
                halt_track = false;
            }
        }

        if halt_track {
            let velocity = self.velocity();
            let speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
            if speed < 0.03 {
                self.set_velocity(DVec3::ZERO);
            } else {
                self.set_velocity(DVec3::new(velocity.x * 0.5, 0.0, velocity.z * 0.5));
            }
        }

        let x0 = f64::from(pos.x()) + 0.5 + f64::from(exit0.x) * 0.5;
        let z0 = f64::from(pos.z()) + 0.5 + f64::from(exit0.z) * 0.5;
        let x1 = f64::from(pos.x()) + 0.5 + f64::from(exit1.x) * 0.5;
        let z1 = f64::from(pos.z()) + 0.5 + f64::from(exit1.z) * 0.5;
        let dx = x1 - x0;
        let dz = z1 - z0;
        let progress = if dx == 0.0 {
            position.z - f64::from(pos.z())
        } else if dz == 0.0 {
            position.x - f64::from(pos.x())
        } else {
            ((position.x - x0) * dx + (position.z - z0) * dz) * 2.0
        };

        let _ = self.try_set_position(DVec3::new(x0 + dx * progress, y, z0 + dz * progress));

        let scale = if self.is_vehicle() { 0.75 } else { 1.0 };
        let max_speed = self.max_speed(world);
        let movement = self.velocity();
        let _ = Arc::clone(&self).move_entity(
            MoverType::SelfMovement,
            DVec3::new(
                (scale * movement.x).clamp(-max_speed, max_speed),
                0.0,
                (scale * movement.z).clamp(-max_speed, max_speed),
            ),
        );

        let position = self.position();
        let block_x = position.x.floor() as i32 - pos.x();
        let block_z = position.z.floor() as i32 - pos.z();
        if exit0.y != 0 && block_x == exit0.x && block_z == exit0.z {
            let _ = self.try_set_position(DVec3::new(
                position.x,
                position.y + f64::from(exit0.y),
                position.z,
            ));
        } else if exit1.y != 0 && block_x == exit1.x && block_z == exit1.z {
            let _ = self.try_set_position(DVec3::new(
                position.x,
                position.y + f64::from(exit1.y),
                position.z,
            ));
        }

        let natural = self.apply_natural_slowdown(self.velocity());
        self.set_velocity(natural);

        let new_pos = rail_position(
            world,
            self.position().x,
            self.position().y,
            self.position().z,
        );
        if let (Some(old), Some(new)) = (old_pos, new_pos) {
            let speed = (old.y - new.y) * 0.05;
            let velocity = self.velocity();
            let horizontal = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
            if horizontal > 0.0 {
                let factor = (horizontal + speed) / horizontal;
                self.set_velocity(DVec3::new(
                    velocity.x * factor,
                    velocity.y,
                    velocity.z * factor,
                ));
            }
            let position = self.position();
            let _ = self.try_set_position(DVec3::new(position.x, new.y, position.z));
        }

        let xn = self.position().x.floor() as i32;
        let zn = self.position().z.floor() as i32;
        if xn != pos.x() || zn != pos.z() {
            let velocity = self.velocity();
            let horizontal = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
            self.set_velocity(DVec3::new(
                horizontal * f64::from(xn - pos.x()),
                velocity.y,
                horizontal * f64::from(zn - pos.z()),
            ));
        }

        if power_track {
            self.apply_powered_rail_boost(pos, shape);
        }
    }

    /// Vanilla `moveAlongTrack`'s powered-rail acceleration.
    fn apply_powered_rail_boost(&self, pos: BlockPos, shape: RailShape) {
        let velocity = self.velocity();
        let speed = (velocity.x * velocity.x + velocity.z * velocity.z).sqrt();
        if speed > 0.01 {
            self.set_velocity(
                velocity + DVec3::new(velocity.x / speed * 0.06, 0.0, velocity.z / speed * 0.06),
            );
            return;
        }

        let mut dx = velocity.x;
        let mut dz = velocity.z;
        match shape {
            RailShape::EastWest => {
                if self.is_redstone_conductor_at(pos.west()) {
                    dx = 0.02;
                } else if self.is_redstone_conductor_at(pos.east()) {
                    dx = -0.02;
                }
            }
            RailShape::NorthSouth => {
                if self.is_redstone_conductor_at(pos.north()) {
                    dz = 0.02;
                } else if self.is_redstone_conductor_at(pos.south()) {
                    dz = -0.02;
                }
            }
            _ => return,
        }

        self.set_velocity(DVec3::new(dx, velocity.y, dz));
    }

    /// Vanilla `AbstractMinecart.comeOffTrack`.
    fn come_off_track(self: Arc<Self>, world: &Arc<World>) {
        let max_speed = self.max_speed(world);
        let movement = self.velocity();
        self.set_velocity(DVec3::new(
            movement.x.clamp(-max_speed, max_speed),
            movement.y,
            movement.z.clamp(-max_speed, max_speed),
        ));
        if self.on_ground() {
            self.set_velocity(self.velocity() * 0.5);
        }

        let _ = Arc::clone(&self).move_entity(MoverType::SelfMovement, self.velocity());
        if !self.on_ground() {
            self.set_velocity(self.velocity() * AIR_DRAG);
        }
    }

    /// Vanilla `OldMinecartBehavior.pushAndPickupEntities`.
    fn push_and_pickup_entities(self: Arc<Self>) {
        let Some(world) = self.level() else {
            return;
        };
        let cart = self.as_entity_event_source();
        let cart_shared = Arc::clone(&self).into_shared_entity();
        let hitbox = self.bounding_box().inflate_xyz(0.2, 0.0, 0.2);
        let velocity = self.velocity();
        let moving = velocity.x * velocity.x + velocity.z * velocity.z >= 0.01;

        if self.is_rideable() && moving {
            let entities = world.get_entities_in_aabb_matching(&hitbox, |entity| {
                entity.id() != self.id() && is_pushable_by(entity, cart)
            });
            for entity in entities {
                if entity.as_player().is_none()
                    && !entity.is::<IronGolemEntity>()
                    && !is_minecart_entity(&*entity)
                    && !self.is_vehicle()
                    && !entity.is_passenger()
                {
                    entity.start_riding(&cart_shared);
                } else {
                    entity.push_entity(cart);
                }
            }
            return;
        }

        let entities =
            world.get_entities_in_aabb_matching(&hitbox, |entity| entity.id() != self.id());
        for entity in entities {
            if !self.has_passenger(&*entity) && entity.is_pushable() && is_minecart_entity(&*entity)
            {
                entity.push_entity(cart);
            }
        }
    }

    /// Vanilla `VehicleEntity.hurtServer`.
    fn minecart_hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        if self.is_removed() {
            return true;
        }

        if self.is_invulnerable_to_base(source) {
            return false;
        }

        self.mark_minecart_hurt(amount);
        self.game_event_with_source_entity(
            &vanilla_game_events::ENTITY_DAMAGE,
            source.causing_entity().map(|entity| &**entity),
        );
        let creative_player = source.is_creative_player();
        if (creative_player || self.hurt_damage() <= DESTROY_DAMAGE_THRESHOLD)
            && !self.should_source_destroy_minecart(source)
        {
            if creative_player {
                self.set_removed(RemovalReason::Discarded);
            }
        } else {
            self.minecart_destroy(world, source);
        }

        true
    }

    /// Vanilla `VehicleEntity.shouldSourceDestroy`; only the TNT cart overrides it.
    fn should_source_destroy_minecart(&self, _source: &DamageSource) -> bool {
        false
    }

    /// Vanilla `VehicleEntity.destroy(ServerLevel, DamageSource)`.
    fn minecart_destroy(&self, world: &World, _source: &DamageSource) {
        self.destroy_with_drop(world, self.drop_item());
    }

    /// Vanilla `VehicleEntity.destroy(ServerLevel, Item)`.
    fn destroy_with_drop(&self, world: &World, drop_item: Option<ItemRef>) {
        self.kill(world);
        if world.get_game_rule(&vanilla_game_rules::ENTITY_DROPS)
            && let Some(item) = drop_item
        {
            let mut stack = ItemStack::new(item);
            if let Some(name) = self.custom_name() {
                stack.set(vanilla_components::CUSTOM_NAME, name);
            }
            self.spawn_at_location(stack, 0.0);
        }
        self.on_minecart_destroyed(world);
    }
}

/// Vanilla `AbstractMinecart.getRelativePortalPosition` for carts.
pub(super) fn minecart_relative_portal_position(
    entity: &dyn Entity,
    axis: Axis,
    portal_area: FoundRectangle,
) -> DVec3 {
    reset_forward_direction_of_relative_portal_position(PortalShape::get_relative_position(
        portal_area,
        axis,
        entity.position(),
        entity.dimensions_for_pose(entity.pose()),
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Weak};

    use steel_registry::{init_vanilla_registry, vanilla_entities, vanilla_game_rules};

    use super::*;
    use crate::entity::SharedEntity;
    use crate::test_support::fresh_test_world;

    fn plain_cart(world: Weak<World>) -> MinecartEntity {
        MinecartEntity::new(
            &vanilla_entities::MINECART,
            1,
            DVec3::new(8.5, 64.0, 8.5),
            world,
        )
    }

    #[test]
    fn minecart_damage_absorbs_ten_times_the_incoming_amount() {
        let cart = plain_cart(Weak::new());

        // Vanilla `VehicleEntity.hurtServer`: `setDamage(getDamage() + amount * 10.0f)`.
        cart.mark_minecart_hurt(5.0);
        assert_eq!(cart.hurt_damage(), 50.0);
        assert_eq!(cart.hurt_time(), 10);

        cart.mark_minecart_hurt(0.5);
        assert_eq!(cart.hurt_damage(), 55.0);
    }

    #[test]
    fn minecart_max_speed_follows_old_behavior_not_the_improvements_rule() {
        let world_fixture = fresh_test_world("minecart_old_behavior_max_speed");
        let world = &world_fixture.world;
        world.set_game_rule(&vanilla_game_rules::MAX_MINECART_SPEED, 20);

        let cart = plain_cart(Arc::downgrade(world));

        // Vanilla reads `max_minecart_speed` only from the experimental
        // `NewMinecartBehavior`; the stable behavior stays at 0.4 on land.
        assert_eq!(default_max_speed(&cart, world), MAX_SPEED_ON_LAND);
        assert!(!cart.is_in_water());
    }

    #[test]
    fn chest_carts_count_as_minecarts_for_collisions_and_boarding() {
        // Vanilla's cart checks are `instanceof AbstractMinecart`: `push_and_pickup_entities`
        // pushes a chest cart instead of trying to board it, and two such carts collide
        // through `push_other_minecart` instead of the generic impulse push.
        let chest = ChestMinecartEntity::new(
            &vanilla_entities::CHEST_MINECART,
            3,
            DVec3::new(8.5, 64.0, 8.5),
            Weak::new(),
        );
        assert!(is_minecart_entity(&chest));
    }

    #[test]
    fn cart_push_into_a_furnace_cart_damps_the_pusher_like_vanilla() {
        // Vanilla `AbstractMinecart.pushOtherMinecart`: with `xa = 0.05` for a cart half a block
        // away and a yaw pointing at it, the normal cart is damped to 0.2 and receives the
        // furnace cart's movement minus `xa`, while the furnace cart keeps 0.95 of its movement.
        let cart = plain_cart(Weak::new());
        let furnace = Arc::new(FurnaceMinecartEntity::new(
            &vanilla_entities::FURNACE_MINECART,
            2,
            DVec3::new(9.0, 64.0, 8.5),
            Weak::new(),
        ));
        let furnace_shared: SharedEntity = Arc::clone(&furnace) as SharedEntity;

        cart.set_velocity(DVec3::new(0.4, 0.0, 0.0));
        furnace.set_velocity(DVec3::new(0.3, 0.0, 0.0));
        cart.push_entity(&*furnace_shared);

        assert!((cart.velocity().x - 0.33).abs() < 1.0e-12);
        assert!((furnace.velocity().x - 0.285).abs() < 1.0e-12);
    }

    #[test]
    fn cart_push_is_skipped_when_the_other_cart_is_not_in_front() {
        // The `|dir · facing| < 0.8` guard: a cart to the side is ignored entirely.
        let cart = plain_cart(Weak::new());
        let other = Arc::new(MinecartEntity::new(
            &vanilla_entities::MINECART,
            2,
            DVec3::new(8.5, 64.0, 9.0),
            Weak::new(),
        ));
        let other_shared: SharedEntity = Arc::clone(&other) as SharedEntity;

        cart.set_velocity(DVec3::new(0.4, 0.0, 0.0));
        other.set_velocity(DVec3::new(0.4, 0.0, 0.0));
        cart.push_entity(&*other_shared);

        assert_eq!(cart.velocity(), DVec3::new(0.4, 0.0, 0.0));
        assert_eq!(other.velocity(), DVec3::new(0.4, 0.0, 0.0));
    }

    #[test]
    fn cart_push_splits_movement_evenly_between_two_normal_carts() {
        // Vanilla's final `pushOtherMinecart` branch: `xdd = (other + own) / 2`, both carts
        // damped to 0.2 and pushed apart by `xa`.
        init_vanilla_registry();
        let cart = plain_cart(Weak::new());
        let other = Arc::new(MinecartEntity::new(
            &vanilla_entities::MINECART,
            2,
            DVec3::new(9.0, 64.0, 8.5),
            Weak::new(),
        ));
        let other_shared: SharedEntity = Arc::clone(&other) as SharedEntity;

        cart.set_velocity(DVec3::new(0.4, 0.0, 0.0));
        other.set_velocity(DVec3::new(0.2, 0.0, 0.0));
        cart.push_entity(&*other_shared);

        // own: 0.4 * 0.2 = 0.08, impulse (0.3 - 0.05) = 0.25 -> 0.33
        assert!((cart.velocity().x - 0.33).abs() < 1.0e-12);
        // other: 0.2 * 0.2 = 0.04, impulse (0.3 + 0.05) = 0.35 -> 0.39
        assert!((other.velocity().x - 0.39).abs() < 1.0e-12);
    }
}
