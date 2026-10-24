//! TNT minecart entity (`net.minecraft.world.entity.vehicle.minecart.MinecartTNT`).
//!
//! Detonates on impact, fire, explosion damage, a powered activator rail, or a
//! fall of three blocks or more. The fuse, ignition bookkeeping and the blast itself
//! (vanilla power `explosionPowerBase + explosionSpeedFactor * random * 1.5 * speed`,
//! `Level.ExplosionInteraction.TNT`, with the primed-rail exemption) are complete and
//! end with the cart discarded.
//!
//! Deliberate difference: vanilla switches to the experimental
//! `NewMinecartBehavior` when `minecart_improvements` is enabled; Steel always
//! uses the stable `OldMinecartBehavior` physics from [`super::minecart_base`].

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::fluid::FluidState;
use steel_registry::items::ItemRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_damage_type_tags::DamageTypeTag;
use steel_registry::vanilla_entity_data::{AbstractMinecartEntityData, TntMinecartEntityData};
use steel_registry::{
    sound_events, vanilla_blocks, vanilla_damage_types, vanilla_game_rules, vanilla_items,
    vanilla_particle_types,
};
use steel_utils::axis::Axis;
use steel_utils::block_util::FoundRectangle;
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockPos, BlockStateId, DowncastType, DowncastTypeKey};

use crate::entity::damage::DamageSource;
use crate::entity::{Entity, EntityBase, EntityBaseLoad, EntitySyncedData, RemovalReason};
use crate::world::World;
use crate::world::explosion::{
    ExplosionDamageCalculator, ExplosionInfo, ExplosionInteraction, base_block_explosion_resistance,
};

use super::minecart_base::{self, Minecart, MinecartState};

/// Vanilla `MinecartTNT` default fuse length.
const DEFAULT_FUSE: i32 = 80;
/// Vanilla `MinecartTNT.DEFAULT_EXPLOSION_POWER_BASE`.
const DEFAULT_EXPLOSION_POWER_BASE: f32 = 4.0;
/// Vanilla `MinecartTNT.DEFAULT_EXPLOSION_SPEED_FACTOR`.
const DEFAULT_EXPLOSION_SPEED_FACTOR: f32 = 1.0;
/// Vanilla clamps both explosion fields to this range on load.
const EXPLOSION_FIELD_MAX: f32 = 128.0;
/// Vanilla `MinecartTNT.explode` speed cap.
const MAX_EXPLOSION_SPEED: f64 = 5.0;
/// Vanilla `MinecartTNT.tick` horizontal-collision detonation threshold.
const COLLISION_SPEED_SQR_THRESHOLD: f64 = 0.01;
/// Vanilla `MinecartTNT.causeFallDamage` minimum fall distance.
const FALL_DETONATION_DISTANCE: f64 = 3.0;
/// Vanilla short fuse: `random.nextInt(20) + random.nextInt(20)`.
const SHORT_FUSE_BOUND: i32 = 20;

/// A TNT minecart.
#[entity_behavior(class = "MinecartTNT")]
pub struct TntMinecartEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    entity_data: SyncMutex<TntMinecartEntityData>,
    state: SyncMutex<MinecartState>,
    fuse: SyncMutex<TntState>,
}

/// Vanilla `MinecartTNT` fuse and explosion tuning (plain fields, not synced).
#[derive(Clone)]
struct TntState {
    /// Fuse ticks; `-1` when not primed.
    fuse: i32,
    /// Vanilla `explosionPowerBase`.
    explosion_power_base: f32,
    /// Vanilla `explosionSpeedFactor`.
    explosion_speed_factor: f32,
    /// Vanilla `ignitionSource`, absent until the cart is ignited.
    ignition_source: Option<DamageSource>,
}

impl TntState {
    const fn new() -> Self {
        Self {
            fuse: -1,
            explosion_power_base: DEFAULT_EXPLOSION_POWER_BASE,
            explosion_speed_factor: DEFAULT_EXPLOSION_SPEED_FACTOR,
            ignition_source: None,
        }
    }
}

// SAFETY: This key is owned by Steel and uniquely identifies `TntMinecartEntity`.
unsafe impl DowncastType for TntMinecartEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/tnt_minecart");
}

/// Vanilla `MinecartTNT`'s `getBlockExplosionResistance`/`shouldBlockExplode` overrides as a
/// `World::explode_full` calculator: while the cart is primed, rails and the blocks directly
/// below them (the blocks whose upper neighbour is a rail) are exempt from its own blast.
struct TntMinecartExplosionDamageCalculator {
    /// Whether the exploding cart's fuse had started, vanilla `isPrimed()`.
    primed: bool,
}

impl TntMinecartExplosionDamageCalculator {
    /// Vanilla `!isPrimed() || !block.is(BlockTags.RAILS) && !level.getBlockState(pos.above()).is(BlockTags.RAILS)`.
    fn rail_exempt(&self, world: &World, pos: BlockPos, state: BlockStateId) -> bool {
        self.primed
            && (minecart_base::is_rail_state(state)
                || minecart_base::is_rail_state(world.get_block_state(pos.above())))
    }
}

impl ExplosionDamageCalculator for TntMinecartExplosionDamageCalculator {
    fn block_explosion_resistance(
        &self,
        _explosion: &ExplosionInfo<'_>,
        world: &World,
        pos: BlockPos,
        state: BlockStateId,
        fluid: FluidState,
    ) -> Option<f32> {
        if self.rail_exempt(world, pos, state) {
            return Some(0.0);
        }
        base_block_explosion_resistance(state, fluid)
    }

    fn should_block_explode(
        &self,
        _explosion: &ExplosionInfo<'_>,
        world: &World,
        pos: BlockPos,
        state: BlockStateId,
        _power: f32,
    ) -> bool {
        !self.rail_exempt(world, pos, state)
    }
}

impl TntMinecartEntity {
    /// Creates a new TNT minecart entity.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(TntMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            fuse: SyncMutex::new(TntState::new()),
        }
    }

    /// Creates a TNT minecart entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(TntMinecartEntityData::new()),
            state: SyncMutex::new(MinecartState::new()),
            fuse: SyncMutex::new(TntState::new()),
        }
    }

    /// Vanilla `getFuse`.
    #[must_use]
    pub fn fuse(&self) -> i32 {
        self.fuse.lock().fuse
    }

    /// Sets the fuse, vanilla `fuse` assignment.
    pub fn set_fuse(&self, fuse: i32) {
        self.fuse.lock().fuse = fuse;
    }

    /// Vanilla `isPrimed`.
    #[must_use]
    pub fn is_primed(&self) -> bool {
        self.fuse() > -1
    }

    /// Vanilla `explosionPowerBase`.
    #[must_use]
    pub fn explosion_power_base(&self) -> f32 {
        self.fuse.lock().explosion_power_base
    }

    /// Vanilla `explosionSpeedFactor`.
    #[must_use]
    pub fn explosion_speed_factor(&self) -> f32 {
        self.fuse.lock().explosion_speed_factor
    }

    /// Vanilla `getIgnitionSource`.
    #[must_use]
    pub fn ignition_source(&self) -> Option<DamageSource> {
        self.fuse.lock().ignition_source.clone()
    }

    /// Vanilla `primeFuse`.
    pub fn prime_fuse(&self, world: &World, source: Option<&DamageSource>) {
        if !world.get_game_rule(&vanilla_game_rules::TNT_EXPLODES) {
            return;
        }

        {
            let mut state = self.fuse.lock();
            state.fuse = DEFAULT_FUSE;
            if let Some(source) = source
                && state.ignition_source.is_none()
            {
                let mut ignition = DamageSource::environment(source.damage_type);
                if let Some(causing) = source.causing_entity() {
                    ignition = ignition.with_causing_entity(Arc::clone(causing));
                }
                state.ignition_source = Some(ignition);
            }
        }

        self.broadcast_entity_event(EntityStatus::TntPrime);
        // Vanilla `MinecartTNT.primeFuse` skips the primed sound for silent carts.
        if !self.is_silent() {
            self.play_sound(&sound_events::ENTITY_TNT_PRIMED, 1.0, 1.0);
        }
    }

    /// Vanilla `MinecartTNT.explode`.
    ///
    /// The blast is dispatched through `World::explode_full` with `MinecartTNT`'s own
    /// `ExplosionDamageCalculator`: while the cart is primed, rails and the blocks directly
    /// below them neither detonate nor contribute their blast resistance.
    fn explode(&self, source: Option<&DamageSource>, speed_sqr: f64) {
        let Some(world) = self.level() else {
            return;
        };

        if !world.get_game_rule(&vanilla_game_rules::TNT_EXPLODES) {
            if self.is_primed() {
                self.set_removed(RemovalReason::Discarded);
            }
            return;
        }

        // Vanilla computes the power in double and casts the whole expression to float.
        let speed = speed_sqr.sqrt().min(MAX_EXPLOSION_SPEED);
        let power = (f64::from(self.explosion_power_base())
            + f64::from(self.explosion_speed_factor()) * rand::random::<f64>() * 1.5 * speed)
            as f32;
        let calculator = TntMinecartExplosionDamageCalculator {
            primed: self.is_primed(),
        };
        let position = self.position();
        world.explode_full(
            world.get_entity_by_id(self.id()),
            source.cloned(),
            Some(&calculator),
            position.x,
            position.y,
            position.z,
            power,
            false,
            ExplosionInteraction::Tnt,
        );
        self.set_removed(RemovalReason::Discarded);
    }

    /// Vanilla `MinecartTNT.damageSourceIgnitesTnt`.
    ///
    /// Vanilla tests `instanceof Projectile`, which covers more than abstract
    /// arrows (for example a burning snowball or fireball still ignites TNT).
    fn damage_source_ignites_tnt(source: &DamageSource) -> bool {
        if let Some(direct) = source.direct_entity()
            && direct.as_projectile().is_some()
        {
            return direct.is_on_fire();
        }

        source.is(&DamageTypeTag::IS_FIRE) || source.is(&DamageTypeTag::IS_EXPLOSION)
    }

    /// Vanilla `MinecartTNT.hurtServer` projectile ignition.
    fn ignite_from_projectile(&self, source: &DamageSource) {
        let Some(direct) = source.direct_entity() else {
            return;
        };
        if !direct.is_abstract_arrow() || !direct.is_on_fire() {
            return;
        }

        let speed_sqr = direct.velocity().length_squared();
        let ignition = match self
            .level()
            .and_then(|world| world.get_entity_by_id(self.id()))
        {
            Some(entity) => DamageSource::direct(&vanilla_damage_types::EXPLOSION, entity),
            None => DamageSource::environment(&vanilla_damage_types::EXPLOSION),
        };
        let ignition = source.causing_entity().map_or(ignition.clone(), |causing| {
            ignition.clone().with_causing_entity(Arc::clone(causing))
        });
        self.explode(Some(&ignition), speed_sqr);
    }
}

impl Minecart for TntMinecartEntity {
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
        vanilla_blocks::TNT.default_state()
    }

    fn drop_item(&self) -> Option<ItemRef> {
        Some(&vanilla_items::TNT_MINECART)
    }

    fn activate_minecart(&self, world: &Arc<World>, _pos: BlockPos, powered: bool) {
        // Vanilla `MinecartTNT.activateMinecart`: a powered activator rail primes.
        if powered && self.fuse() < 0 {
            self.prime_fuse(world, None);
        }
    }

    fn should_source_destroy_minecart(&self, source: &DamageSource) -> bool {
        Self::damage_source_ignites_tnt(source)
    }

    fn minecart_destroy(&self, world: &World, source: &DamageSource) {
        // Vanilla `MinecartTNT.destroy(ServerLevel, DamageSource)`.
        let speed_sqr = {
            let velocity = self.velocity();
            velocity.x * velocity.x + velocity.z * velocity.z
        };
        if !Self::damage_source_ignites_tnt(source) && speed_sqr < COLLISION_SPEED_SQR_THRESHOLD {
            self.destroy_with_drop(world, self.drop_item());
        } else if self.fuse() < 0 {
            self.prime_fuse(world, Some(source));
            self.set_fuse(
                rand::random_range(0..SHORT_FUSE_BOUND) + rand::random_range(0..SHORT_FUSE_BOUND),
            );
        }
    }
}

impl Entity for TntMinecartEntity {
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

        let fuse = self.fuse();
        if fuse > 0 {
            self.set_fuse(fuse - 1);
            if let Some(world) = self.level() {
                let position = self.position();
                world.send_particles(
                    ParticleData::simple(&vanilla_particle_types::SMOKE),
                    DVec3::new(position.x, position.y + 0.5, position.z),
                    1,
                    DVec3::ZERO,
                    0.0,
                );
            }
        } else if fuse == 0 {
            let speed_sqr = {
                let velocity = self.velocity();
                velocity.x * velocity.x + velocity.z * velocity.z
            };
            let ignition = self.ignition_source();
            self.explode(ignition.as_ref(), speed_sqr);
        }

        if self.horizontal_collision() {
            let speed_sqr = {
                let velocity = self.velocity();
                velocity.x * velocity.x + velocity.z * velocity.z
            };
            if speed_sqr >= COLLISION_SPEED_SQR_THRESHOLD {
                let ignition = self.ignition_source();
                self.explode(ignition.as_ref(), speed_sqr);
            }
        }
    }

    fn hurt(&self, world: &World, source: &DamageSource, amount: f32) -> bool {
        self.ignite_from_projectile(source);
        self.minecart_hurt(world, source, amount)
    }

    fn cause_fall_damage(
        self: Arc<Self>,
        fall_distance: f64,
        damage_modifier: f32,
        source: &DamageSource,
    ) -> bool {
        if fall_distance >= FALL_DETONATION_DISTANCE {
            let power = fall_distance / 10.0;
            let ignition = self.ignition_source();
            self.explode(ignition.as_ref(), power * power);
        }

        // Vanilla `Entity.causeFallDamage` for a non-living cart only propagates to
        // passengers and reports the damage as unhandled.
        self.propagate_fall_to_passengers(fall_distance, damage_modifier, source);
        false
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_minecart_data(nbt);
        let state = self.fuse.lock();
        nbt.insert("fuse", state.fuse);
        if state.explosion_power_base != DEFAULT_EXPLOSION_POWER_BASE {
            nbt.insert("explosion_power", state.explosion_power_base);
        }
        if state.explosion_speed_factor != DEFAULT_EXPLOSION_SPEED_FACTOR {
            nbt.insert("explosion_speed_factor", state.explosion_speed_factor);
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_minecart_data(nbt);
        let mut state = self.fuse.lock();
        state.fuse = nbt.int("fuse").unwrap_or(-1);
        state.explosion_power_base = nbt
            .float("explosion_power")
            .unwrap_or(DEFAULT_EXPLOSION_POWER_BASE)
            .clamp(0.0, EXPLOSION_FIELD_MAX);
        state.explosion_speed_factor = nbt
            .float("explosion_speed_factor")
            .unwrap_or(DEFAULT_EXPLOSION_SPEED_FACTOR)
            .clamp(0.0, EXPLOSION_FIELD_MAX);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Weak;

    use steel_registry::vanilla_game_rules;
    use steel_registry::{init_vanilla_registry, vanilla_blocks, vanilla_entities};
    use steel_utils::types::UpdateFlags;
    use steel_utils::{BlockPos, ChunkPos};

    use crate::entity::entities::objects::SnowballEntity;
    use crate::entity::{Entity, SharedEntity, next_entity_id};
    use crate::test_support::{fresh_test_world, insert_ready_full_chunk};
    use crate::world::LevelReader as _;

    use super::*;
    use steel_registry::blocks::block_state_ext::BlockStateExt as _;

    fn direct_source(entity: SharedEntity) -> DamageSource {
        DamageSource::direct(&vanilla_damage_types::GENERIC, entity)
    }

    #[test]
    fn burning_non_arrow_projectiles_ignite_tnt() {
        // Vanilla `MinecartTNT.damageSourceIgnitesTnt` tests `instanceof
        // Projectile`, so any burning projectile ignites TNT — not just arrows.
        let snowball: SharedEntity = Arc::new(SnowballEntity::new(
            &vanilla_entities::SNOWBALL,
            1,
            DVec3::ZERO,
            Weak::new(),
        ));
        snowball.ignite_for_ticks(100);
        assert!(snowball.is_on_fire());
        assert!(TntMinecartEntity::damage_source_ignites_tnt(
            &direct_source(snowball)
        ));
    }

    #[test]
    fn cold_projectile_with_generic_damage_does_not_ignite() {
        let snowball: SharedEntity = Arc::new(SnowballEntity::new(
            &vanilla_entities::SNOWBALL,
            2,
            DVec3::ZERO,
            Weak::new(),
        ));
        // GENERIC carries neither the `is_fire` nor the `is_explosion` tag, so a
        // cold snowball must not ignite the cart.
        assert!(!TntMinecartEntity::damage_source_ignites_tnt(
            &direct_source(snowball)
        ));
    }

    #[test]
    fn powered_activator_rail_primes_an_eighty_tick_fuse_once() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("tnt_minecart_activator_prime");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(DVec3::ZERO));
        let cart = TntMinecartEntity::new(
            &vanilla_entities::TNT_MINECART,
            next_entity_id(),
            DVec3::ZERO,
            Arc::downgrade(world),
        );
        let pos = BlockPos::new(0, 0, 0);

        // Vanilla `activateMinecart`: an unpowered rail does nothing.
        cart.activate_minecart(world, pos, false);
        assert!(!cart.is_primed());

        cart.activate_minecart(world, pos, true);
        assert_eq!(cart.fuse(), DEFAULT_FUSE);
        assert!(cart.is_primed());

        // Vanilla `activateMinecart` only primes while `fuse < 0`.
        cart.set_fuse(10);
        cart.activate_minecart(world, pos, true);
        assert_eq!(cart.fuse(), 10);
    }

    #[test]
    fn priming_respects_the_tnt_explodes_rule() {
        init_vanilla_registry();
        let world_fixture = fresh_test_world("tnt_minecart_rule_gate");
        let world = &world_fixture.world;
        world.set_game_rule(&vanilla_game_rules::TNT_EXPLODES, false);

        let cart = TntMinecartEntity::new(
            &vanilla_entities::TNT_MINECART,
            next_entity_id(),
            DVec3::ZERO,
            Arc::downgrade(world),
        );
        cart.prime_fuse(world, None);
        assert!(!cart.is_primed());
    }

    #[test]
    fn primed_explosion_keeps_rails_and_their_support_blocks() {
        // Vanilla `MinecartTNT`'s explosion calculator exempts a block when it is a rail or
        // when the block above it is a rail — i.e. rails and the blocks they stand on — and
        // it also zeroes their blast resistance. `speedSqr = 0` pins the power to exactly
        // 4.0, and the exempt blocks sit next to the centre, so the outcome is stable.
        init_vanilla_registry();
        let world_fixture = fresh_test_world("tnt_minecart_rail_exemption");
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::new(0, 0));

        let rail_pos = BlockPos::new(0, 64, 0);
        let support_pos = rail_pos.below();
        let above_pos = rail_pos.above();
        // Vanilla rails need a sturdy block below them (`BaseRailBlock.canSurvive`) and are
        // dropped by the next neighbor change otherwise.
        assert!(world.set_block(
            support_pos,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL
        ));
        assert!(world.set_block(
            rail_pos,
            vanilla_blocks::RAIL.default_state(),
            UpdateFlags::UPDATE_ALL
        ));
        // The block above the rail is *not* exempt: vanilla only looks upwards from each
        // candidate position, so this stone is a control that the blast reaches the area.
        assert!(world.set_block(
            above_pos,
            vanilla_blocks::STONE.default_state(),
            UpdateFlags::UPDATE_ALL
        ));

        let cart = TntMinecartEntity::new(
            &vanilla_entities::TNT_MINECART,
            next_entity_id(),
            DVec3::new(0.5, 64.0625, 0.5),
            Arc::downgrade(world),
        );
        cart.set_fuse(DEFAULT_FUSE);
        cart.explode(None, 0.0);

        assert!(
            cart.is_removed(),
            "vanilla discards the cart after the blast"
        );
        assert_eq!(
            world.get_block_state(rail_pos).get_block(),
            &vanilla_blocks::RAIL,
            "a primed cart's blast does not destroy rails"
        );
        assert_eq!(
            world.get_block_state(support_pos).get_block(),
            &vanilla_blocks::STONE,
            "blocks whose upper neighbour is a rail are exempt too"
        );
        assert_eq!(
            world.get_block_state(above_pos).get_block(),
            &vanilla_blocks::AIR,
            "the exemption must not cover blocks above a rail"
        );
    }

    #[test]
    fn fuse_end_discards_a_primed_cart_when_explosions_are_off() {
        // Vanilla `explode`: with `tnt_explodes` disabled and the cart primed, the blast is
        // skipped and the cart is discarded.
        init_vanilla_registry();
        let world_fixture = fresh_test_world("tnt_minecart_fuse_end");
        let world = &world_fixture.world;
        world.set_game_rule(&vanilla_game_rules::TNT_EXPLODES, false);
        insert_ready_full_chunk(world, ChunkPos::from_entity_pos(DVec3::ZERO));

        let cart = Arc::new(TntMinecartEntity::new(
            &vanilla_entities::TNT_MINECART,
            next_entity_id(),
            DVec3::ZERO,
            Arc::downgrade(world),
        ));
        world
            .try_add_entity(Arc::clone(&cart) as SharedEntity)
            .expect("cart should attach to the loaded chunk");
        cart.set_fuse(1);
        Arc::clone(&cart).tick();
        assert_eq!(cart.fuse(), 0);
        Arc::clone(&cart).tick();
        assert!(cart.is_removed());
    }
}
