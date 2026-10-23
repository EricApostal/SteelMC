//! Primed TNT entity implementation.
//!
//! Mirrors vanilla `PrimedTnt`: the entity carries the primed fuse countdown and the
//! block state it renders as, falls under vanilla gravity, and detonates when the fuse
//! expires.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_macros::entity_behavior;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::particle_type::ParticleData;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_entity_data::PrimedTntEntityData;
use steel_registry::vanilla_game_rules::TNT_EXPLODES;
use steel_registry::vanilla_particle_types::SMOKE;
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockStateId, DowncastType, DowncastTypeKey};

use crate::block_entity::block_state_nbt;
use crate::entity::damage::DamageSource;
use crate::entity::{
    DEFAULT_AIR_DRAG, Entity, EntityBase, EntityBaseLoad, EntityBaseState, EntityMovementEmission,
    EntityReference, EntitySyncedData, RemovalReason, SharedEntity,
};
use crate::physics::MoverType;
use crate::world::World;
use crate::world::explosion::ExplosionInteraction;

/// Vanilla `PrimedTnt.DEFAULT_FUSE_TIME`.
const DEFAULT_FUSE_TIME: i16 = 80;

/// Vanilla `PrimedTnt.DEFAULT_EXPLOSION_POWER`.
const DEFAULT_EXPLOSION_POWER: f32 = 4.0;

/// Vanilla `PrimedTnt.DEFAULT_GRAVITY`.
const DEFAULT_GRAVITY: f64 = 0.04;

/// Maximum explosion power accepted from saved data (vanilla clamps to `0.0..=128.0`).
const MAX_EXPLOSION_POWER: f32 = 128.0;

/// Vanilla `PrimedTnt` state that is not synchronized to clients.
struct PrimedTntState {
    /// Vanilla `PrimedTnt.owner`, used for explosion attribution.
    owner: Option<EntityReference>,
    /// Vanilla `PrimedTnt.usedPortal`, set when the entity is teleported through a portal.
    used_portal: bool,
    /// Vanilla `PrimedTnt.explosionPower`.
    explosion_power: f32,
}

impl PrimedTntState {
    const fn new() -> Self {
        Self {
            owner: None,
            used_portal: false,
            explosion_power: DEFAULT_EXPLOSION_POWER,
        }
    }
}

/// A primed TNT entity.
///
/// Mirrors vanilla's `PrimedTnt` behavior:
/// - Falls with gravity (0.04 per tick) and bounces on landing
/// - Counts down its synchronized `fuse` and removes itself when it reaches zero
/// - Persists the carried `block_state` and the `explosion_power` override
#[entity_behavior(class = "PrimedTnt")]
pub struct PrimedTntEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,

    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,

    /// Synchronized `fuse` and `block_state` entity data.
    entity_data: SyncMutex<PrimedTntEntityData>,

    /// Unsynchronized vanilla `PrimedTnt` state.
    state: SyncMutex<PrimedTntState>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `PrimedTntEntity`.
unsafe impl DowncastType for PrimedTntEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/primed_tnt");
}

impl PrimedTntEntity {
    /// Creates a primed TNT entity with a full default fuse.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(PrimedTntEntityData::new()),
            state: SyncMutex::new(PrimedTntState::new()),
        }
    }

    /// Primes a TNT entity the way vanilla's `PrimedTnt(level, x, y, z, owner)` does:
    /// a randomized upward kick, an 80 tick fuse, and the given explosion owner.
    ///
    /// # Arguments
    /// * `entity_type` - The vanilla entity type registered for this implementation.
    /// * `id` - The runtime entity id.
    /// * `position` - The spawn position.
    /// * `world` - The world this entity belongs to.
    /// * `owner` - The living entity that primed the TNT, if any.
    #[must_use]
    pub fn primed(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        world: Weak<World>,
        owner: Option<&SharedEntity>,
    ) -> Self {
        let rotation = rand::random::<f64>() * f64::consts::TAU;
        let velocity = DVec3::new(-rotation.sin() * 0.02, 0.2, -rotation.cos() * 0.02);

        let mut state = PrimedTntState::new();
        state.owner = owner.map(EntityReference::from_entity);

        Self {
            base: EntityBase::new_with_state(
                id,
                EntityBaseState::new(position, entity_type.dimensions)
                    .with_velocity(velocity)
                    .with_old_position(position),
                world,
            ),
            entity_type,
            entity_data: SyncMutex::new(PrimedTntEntityData::new()),
            state: SyncMutex::new(state),
        }
    }

    /// Creates a primed TNT entity from saved data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(PrimedTntEntityData::new()),
            state: SyncMutex::new(PrimedTntState::new()),
        }
    }

    /// Sets the remaining fuse ticks (vanilla `PrimedTnt.setFuse`).
    pub fn set_fuse(&self, fuse: i32) {
        self.entity_data.lock().fuse.set(fuse);
    }

    /// Returns the remaining fuse ticks (vanilla `PrimedTnt.getFuse`).
    #[must_use]
    pub fn fuse(&self) -> i32 {
        *self.entity_data.lock().fuse.get()
    }

    /// Sets the block state this primed TNT renders as (vanilla `PrimedTnt.setBlockState`).
    pub fn set_block_state(&self, block_state: BlockStateId) {
        self.entity_data.lock().block_state.set(block_state);
    }

    /// Returns the block state this primed TNT renders as (vanilla `PrimedTnt.getBlockState`).
    #[must_use]
    pub fn block_state(&self) -> BlockStateId {
        *self.entity_data.lock().block_state.get()
    }

    /// Sets the explosion power used when the fuse expires (vanilla `PrimedTnt.explosionPower`).
    pub fn set_explosion_power(&self, power: f32) {
        self.state.lock().explosion_power = power.clamp(0.0, MAX_EXPLOSION_POWER);
    }

    /// Returns the explosion power used when the fuse expires.
    #[must_use]
    pub fn explosion_power(&self) -> f32 {
        self.state.lock().explosion_power
    }

    /// Marks that this entity traveled through a portal (vanilla `PrimedTnt.setUsedPortal`).
    pub fn set_used_portal(&self, used_portal: bool) {
        self.state.lock().used_portal = used_portal;
    }

    /// Returns whether this entity traveled through a portal before detonating.
    #[must_use]
    pub fn used_portal(&self) -> bool {
        self.state.lock().used_portal
    }

    /// Vanilla `PrimedTnt.getOwner`.
    #[must_use]
    pub fn get_owner(&self) -> Option<SharedEntity> {
        let world = self.level()?;
        let owner = self.state.lock().owner.clone()?;
        owner.get_entity(&world)
    }

    /// Retunes a randomized fuse the way vanilla `PrimedTnt.getRandomShortFuse` does.
    ///
    /// Used when a primed TNT is created by an explosion rather than by ignition.
    ///
    /// # Arguments
    /// * `fuse` - The base fuse from the exploding TNT.
    #[must_use]
    pub fn random_short_fuse(fuse: i32) -> i32 {
        rand::random_range(0..(fuse / 4).max(1)) + fuse / 8
    }

    /// Vanilla `PrimedTnt.explode`.
    ///
    /// Vanilla passes `Explosion.getDefaultDamageSource(level, this)` and, when the TNT
    /// traveled through a portal, `USED_PORTAL_DAMAGE_CALCULATOR` (which exempts nether
    /// portal blocks from the blast). Steel's [`World::explode_full`] derives the default
    /// damage source from the source entity, matching the first argument.
    ///
    /// # Arguments
    /// * `world` - The level the TNT detonated in.
    fn explode(self: &Arc<Self>, world: &Arc<World>) {
        if !world.get_game_rule(&TNT_EXPLODES) {
            return;
        }

        // TODO: Pass a nether-portal-exempting `ExplosionDamageCalculator` when
        // `used_portal` is set (vanilla `USED_PORTAL_DAMAGE_CALCULATOR`). Steel's
        // `ExplosionDamageCalculator` trait exists but has no portal-exempt impl yet.
        let source: SharedEntity = Arc::clone(self) as SharedEntity;
        let position = self.position();
        world.explode_full(
            Some(source),
            None,
            None,
            position.x,
            position.y + 0.0625,
            position.z,
            self.explosion_power(),
            false,
            ExplosionInteraction::Tnt,
        );
    }
}

impl Entity for PrimedTntEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        // Vanilla `PrimedTnt.tick` does not call `super.tick()`; it handles the portal
        // itself and never runs the shared base tick (no fire tick, no leash tick).
        self.handle_portal();
        self.apply_gravity();
        let _ = Arc::clone(&self).move_entity(MoverType::SelfMovement, self.velocity());
        self.apply_effects_from_blocks();

        let mut velocity = self.velocity() * DEFAULT_AIR_DRAG;
        if self.on_ground() {
            velocity = DVec3::new(velocity.x * 0.7, velocity.y * -0.5, velocity.z * 0.7);
        }
        self.set_velocity(velocity);

        let fuse = self.fuse() - 1;
        self.set_fuse(fuse);
        if fuse <= 0 {
            self.set_removed(RemovalReason::Discarded);
            if let Some(world) = self.level() {
                self.explode(&world);
            }
        } else if let Some(world) = self.level() {
            world.send_particles(
                ParticleData::simple(&SMOKE),
                DVec3::new(
                    self.position().x,
                    self.position().y + 0.5,
                    self.position().z,
                ),
                0,
                DVec3::ZERO,
                0.0,
            );
        }
    }

    fn get_default_gravity(&self) -> f64 {
        DEFAULT_GRAVITY
    }

    fn movement_emission(&self) -> EntityMovementEmission {
        EntityMovementEmission::None
    }

    fn is_pickable(&self) -> bool {
        !self.is_removed()
    }

    fn blocks_building(&self) -> bool {
        true
    }

    fn attackable(&self) -> bool {
        false
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn hurt(&self, _world: &World, _source: &DamageSource, _amount: f32) -> bool {
        // Vanilla `PrimedTnt.hurtServer` is final and always ignores damage.
        false
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        // Vanilla `PrimedTnt.restoreFrom` copies the explosion owner.
        self.state.lock().owner = Some(EntityReference::from_entity(owner));
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        // Match vanilla's `PrimedTnt.addAdditionalSaveData`.
        nbt.insert("fuse", self.fuse() as i16);
        nbt.insert(
            "block_state",
            NbtTag::Compound(block_state_nbt::save(self.block_state())),
        );

        if self.explosion_power() != DEFAULT_EXPLOSION_POWER {
            nbt.insert("explosion_power", self.explosion_power());
        }

        let state = self.state.lock();
        if let Some(owner) = &state.owner {
            owner.store(nbt, "owner");
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        // Match vanilla's `PrimedTnt.readAdditionalSaveData`.
        self.set_fuse(i32::from(nbt.short("fuse").unwrap_or(DEFAULT_FUSE_TIME)));
        let block_state = nbt
            .compound("block_state")
            .and_then(block_state_nbt::load)
            .unwrap_or_else(|| vanilla_blocks::TNT.default_state());
        self.set_block_state(block_state);

        let mut state = self.state.lock();
        state.explosion_power = nbt
            .float("explosion_power")
            .unwrap_or(DEFAULT_EXPLOSION_POWER)
            .clamp(0.0, MAX_EXPLOSION_POWER);
        state.owner = EntityReference::read(&nbt, "owner");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use steel_registry::vanilla_entities;

    #[test]
    fn primed_tnt_defaults_match_vanilla() {
        let tnt = PrimedTntEntity::new(&vanilla_entities::TNT, 1, DVec3::ZERO, Weak::new());

        assert_eq!(tnt.fuse(), i32::from(DEFAULT_FUSE_TIME));
        assert_eq!(tnt.block_state(), vanilla_blocks::TNT.default_state());
        assert_eq!(tnt.explosion_power(), DEFAULT_EXPLOSION_POWER);
        assert!(!tnt.used_portal());
    }

    #[test]
    fn primed_tnt_saves_only_non_default_explosion_power() {
        let tnt = PrimedTntEntity::new(&vanilla_entities::TNT, 1, DVec3::ZERO, Weak::new());
        tnt.set_fuse(20);

        let mut nbt = NbtCompound::new();
        tnt.save_additional(&mut nbt);
        assert_eq!(nbt.short("fuse"), Some(20));
        assert!(nbt.compound("block_state").is_some());
        assert_eq!(nbt.float("explosion_power"), None);

        tnt.set_explosion_power(8.0);
        let mut nbt = NbtCompound::new();
        tnt.save_additional(&mut nbt);
        assert_eq!(nbt.float("explosion_power"), Some(8.0));
    }

    #[test]
    fn primed_tnt_clamps_explosion_power_from_saved_data() {
        use std::io::Cursor;

        let tnt = PrimedTntEntity::new(&vanilla_entities::TNT, 1, DVec3::ZERO, Weak::new());
        let mut nbt = NbtCompound::new();
        nbt.insert("fuse", 40i16);
        nbt.insert("explosion_power", 1000.0f32);

        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let Ok(borrowed) = simdnbt::borrow::read_compound(&mut Cursor::new(bytes.as_slice()))
        else {
            panic!("saved primed TNT NBT should reborrow");
        };
        tnt.load_additional((&borrowed).into());

        assert_eq!(tnt.fuse(), 40);
        assert_eq!(tnt.explosion_power(), MAX_EXPLOSION_POWER);
        assert_eq!(tnt.block_state(), vanilla_blocks::TNT.default_state());
    }

    #[test]
    fn primed_tnt_ignition_uses_vanilla_fuse_and_kick() {
        let tnt =
            PrimedTntEntity::primed(&vanilla_entities::TNT, 1, DVec3::ZERO, Weak::new(), None);

        assert_eq!(tnt.fuse(), i32::from(DEFAULT_FUSE_TIME));
        assert!(tnt.velocity().length() > 0.0);
    }
}
