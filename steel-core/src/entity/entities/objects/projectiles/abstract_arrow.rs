//! Vanilla `AbstractArrow` — shared arrow flight, sticking, pickup and damage.
//!
//! Mirrors `AbstractArrow` for `Arrow`, `SpectralArrow` and `ThrownTrident`.
//! Non-synced fields live in [`AbstractArrowBase`]; the synced `in_ground`,
//! `pierce_level` and `id_flags` layer stays in each entity's generated
//! `*EntityData` and is reached through [`AbstractArrow::with_arrow_data`].
//! Subclasses only add their own effects (`do_post_hurt_effects`), sounds and
//! pickup items.

use std::ptr;
use std::str::FromStr;
use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::ToNbtTag;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtTag};
use steel_protocol::packets::game::{CGameEvent, GameEventType};
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::data_components::vanilla_components::INTANGIBLE_PROJECTILE;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::AbstractArrowEntityData;
use steel_registry::vanilla_entity_type_tags::EntityTypeTag;
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt as _, sound_events, vanilla_damage_types,
    vanilla_entities, vanilla_items,
};
use steel_utils::locks::SyncMutex;
use steel_utils::{BlockStateId, Identifier, WorldAabb};

use crate::block_entity::block_state_nbt;
use crate::enchantment_helper::{self, EnchantmentDamageContext, EnchantmentPostAttackContext};
use crate::entity::damage::DamageSource;
use crate::entity::entities::ArrowEntity;
use crate::entity::projectile::triangle_random;
use crate::entity::{
    Entity, EntityReference, LivingEntity, Projectile, ProjectileDeflection, ProjectileHit,
    RemovalReason, SharedEntity, next_entity_id,
};
use crate::inventory::container::Container as _;
use crate::physics::{WorldCollisionProvider, has_block_collision};
use crate::player::Player;
use crate::world::{ClipHitResult, World};

/// Vanilla `AbstractArrow.ARROW_BASE_DAMAGE`.
pub const ARROW_BASE_DAMAGE: f64 = 2.0;

/// Vanilla `AbstractArrow` spawn offset below the shooter's eyes (the
/// `LivingEntity` constructor spawns at `mob.getEyeY() - 0.1F`). Shared by the
/// player weapon and mob-arrow factories.
pub const ARROW_SPAWN_EYE_OFFSET: f64 = 0.1;

/// Vanilla `AbstractArrow.SHAKE_TIME`.
const SHAKE_TIME: i32 = 7;

/// Vanilla `AbstractArrow.WATER_INERTIA`.
const WATER_INERTIA: f32 = 0.6;

/// Vanilla `AbstractArrow.INERTIA`.
const AIR_INERTIA: f32 = 0.99;

/// Vanilla `AbstractArrow.tickDespawn` lifespan, in ticks.
const DESPAWN_TICKS: i32 = 1200;

/// Vanilla `AbstractArrow.FLAG_CRIT`.
const FLAG_CRIT: i8 = 1;

/// Vanilla `AbstractArrow.FLAG_NOPHYSICS`.
const FLAG_NO_PHYSICS: i8 = 2;

/// Vanilla `AbstractArrow.Pickup`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Pickup {
    /// Cannot be picked up.
    #[default]
    Disallowed,
    /// Any player may pick this arrow up.
    Allowed,
    /// Only players with infinite materials may pick this arrow up.
    CreativeOnly,
}

impl Pickup {
    /// Vanilla `Pickup.byOrdinal`: out-of-range values fall back to `DISALLOWED`.
    #[must_use]
    pub const fn by_ordinal(ordinal: i32) -> Self {
        match ordinal {
            1 => Self::Allowed,
            2 => Self::CreativeOnly,
            _ => Self::Disallowed,
        }
    }

    /// Returns the byte vanilla `Pickup.LEGACY_CODEC` stores in NBT.
    #[must_use]
    pub const fn legacy_byte(self) -> i8 {
        self as i8
    }
}

/// Non-synced `AbstractArrow` fields.
///
/// Each field is behind its own lock so no arrow method ever holds one lock
/// while calling back into entity or world code.
pub struct AbstractArrowBase {
    last_state: SyncMutex<Option<BlockStateId>>,
    in_ground_time: SyncMutex<i32>,
    pickup: SyncMutex<Pickup>,
    shake_time: SyncMutex<i32>,
    life: SyncMutex<i32>,
    base_damage: SyncMutex<f64>,
    sound_event: SyncMutex<SoundEventRef>,
    piercing_ignore_entity_ids: SyncMutex<Vec<i32>>,
    pierced_and_killed_entities: SyncMutex<Vec<SharedEntity>>,
    pickup_item_stack: SyncMutex<ItemStack>,
    fired_from_weapon: SyncMutex<Option<ItemStack>>,
}

impl AbstractArrowBase {
    /// Creates the shared state for an arrow whose default pickup stack and
    /// hit-ground sound are supplied by the concrete entity.
    #[must_use]
    pub fn new(pickup_item_stack: ItemStack, default_sound_event: SoundEventRef) -> Self {
        Self {
            last_state: SyncMutex::new(None),
            in_ground_time: SyncMutex::new(0),
            pickup: SyncMutex::new(Pickup::Disallowed),
            shake_time: SyncMutex::new(0),
            life: SyncMutex::new(0),
            base_damage: SyncMutex::new(ARROW_BASE_DAMAGE),
            sound_event: SyncMutex::new(default_sound_event),
            piercing_ignore_entity_ids: SyncMutex::new(Vec::new()),
            pierced_and_killed_entities: SyncMutex::new(Vec::new()),
            pickup_item_stack: SyncMutex::new(pickup_item_stack),
            fired_from_weapon: SyncMutex::new(None),
        }
    }

    /// Returns the block state this arrow last hit while stuck in a block.
    #[must_use]
    pub fn last_state(&self) -> Option<BlockStateId> {
        *self.last_state.lock()
    }

    /// Stores the block state this arrow hit.
    pub fn set_last_state(&self, state: Option<BlockStateId>) {
        *self.last_state.lock() = state;
    }

    /// Returns how many consecutive ticks this arrow has been in the ground.
    #[must_use]
    pub fn in_ground_time(&self) -> i32 {
        *self.in_ground_time.lock()
    }

    /// Advances the in-ground tick counter and returns the new value.
    pub fn increment_in_ground_time(&self) -> i32 {
        let mut time = self.in_ground_time.lock();
        *time += 1;
        *time
    }

    /// Resets the in-ground tick counter.
    pub fn reset_in_ground_time(&self) {
        *self.in_ground_time.lock() = 0;
    }

    /// Returns this arrow's pickup rule.
    #[must_use]
    pub fn pickup(&self) -> Pickup {
        *self.pickup.lock()
    }

    /// Sets this arrow's pickup rule.
    pub fn set_pickup(&self, pickup: Pickup) {
        *self.pickup.lock() = pickup;
    }

    /// Returns the remaining shake animation ticks.
    #[must_use]
    pub fn shake_time(&self) -> i32 {
        *self.shake_time.lock()
    }

    /// Sets the shake animation timer.
    pub fn set_shake_time(&self, shake_time: i32) {
        *self.shake_time.lock() = shake_time;
    }

    /// Returns the in-ground despawn timer.
    #[must_use]
    pub fn life(&self) -> i32 {
        *self.life.lock()
    }

    /// Sets the in-ground despawn timer.
    pub fn set_life(&self, life: i32) {
        *self.life.lock() = life;
    }

    /// Advances the in-ground despawn timer and returns the new value.
    pub fn increment_life(&self) -> i32 {
        let mut life = self.life.lock();
        *life += 1;
        *life
    }

    /// Returns the arrow's base damage (vanilla `baseDamage`).
    #[must_use]
    pub fn base_damage(&self) -> f64 {
        *self.base_damage.lock()
    }

    /// Sets the arrow's base damage.
    pub fn set_base_damage(&self, base_damage: f64) {
        *self.base_damage.lock() = base_damage;
    }

    /// Returns the sound played when this arrow hits an entity or a block.
    #[must_use]
    pub fn sound_event(&self) -> SoundEventRef {
        *self.sound_event.lock()
    }

    /// Replaces the hit sound (crossbows use `crossbow.hit`, hits in the ground
    /// switch to `arrow.hit`).
    pub fn set_sound_event(&self, sound_event: SoundEventRef) {
        *self.sound_event.lock() = sound_event;
    }

    /// Returns whether `entity_id` was already pierced by this arrow.
    #[must_use]
    pub fn is_piercing_ignored(&self, entity_id: i32) -> bool {
        self.piercing_ignore_entity_ids.lock().contains(&entity_id)
    }

    /// Returns how many entities this arrow has pierced so far.
    #[must_use]
    pub fn piercing_ignore_count(&self) -> usize {
        self.piercing_ignore_entity_ids.lock().len()
    }

    /// Records a pierced entity.
    pub fn add_piercing_ignore(&self, entity_id: i32) {
        let mut ids = self.piercing_ignore_entity_ids.lock();
        if !ids.contains(&entity_id) {
            ids.push(entity_id);
        }
    }

    /// Returns a snapshot of the entities this arrow pierced and killed.
    #[must_use]
    pub fn pierced_and_killed_entities(&self) -> Vec<SharedEntity> {
        self.pierced_and_killed_entities.lock().clone()
    }

    /// Records an entity this arrow pierced and killed.
    pub fn add_pierced_and_killed_entity(&self, entity: SharedEntity) {
        self.pierced_and_killed_entities.lock().push(entity);
    }

    /// Clears the pierced entity bookkeeping (vanilla `resetPiercedEntities`).
    pub fn reset_pierced_entities(&self) {
        self.piercing_ignore_entity_ids.lock().clear();
        self.pierced_and_killed_entities.lock().clear();
    }

    /// Returns the stack this arrow drops and renders when picked up.
    #[must_use]
    pub fn pickup_item_stack(&self) -> ItemStack {
        self.pickup_item_stack.lock().clone()
    }

    /// Stores the stack this arrow drops and renders when picked up.
    pub fn set_pickup_item_stack(&self, stack: ItemStack) {
        *self.pickup_item_stack.lock() = stack;
    }

    /// Returns the weapon this arrow was fired from.
    #[must_use]
    pub fn fired_from_weapon(&self) -> Option<ItemStack> {
        self.fired_from_weapon.lock().clone()
    }

    /// Stores the weapon this arrow was fired from.
    pub fn set_fired_from_weapon(&self, weapon: Option<ItemStack>) {
        *self.fired_from_weapon.lock() = weapon;
    }
}

/// Vanilla `Mth.lerp(0.2, rotO, rot)` with the shortest-arc wrap vanilla uses
/// for projectile rotation.
fn lerp_rotation(mut rot_old: f32, rot: f32) -> f32 {
    use steel_math::{DEGREE_180, DEGREE_360};
    while rot - rot_old < -DEGREE_180 {
        rot_old -= DEGREE_360;
    }
    while rot - rot_old >= DEGREE_180 {
        rot_old += DEGREE_360;
    }
    rot_old + 0.2 * (rot - rot_old)
}

/// Vanilla `AbstractArrow` behavior shared by arrows and thrown tridents.
///
/// Concrete entities forward `Projectile::on_hit_entity` / `on_hit_block` and
/// their `Entity::tick` into this trait, which keeps the vanilla dispatch
/// (`super.onHitEntity()`, `super.tick()`) intact.
pub trait AbstractArrow: Projectile + Sized {
    /// Returns the shared non-synced arrow state.
    fn abstract_arrow_base(&self) -> &AbstractArrowBase;

    /// Runs `f` with this entity's synced `AbstractArrow` data layer.
    fn with_arrow_data<R>(&self, f: impl FnOnce(&mut AbstractArrowEntityData) -> R) -> R;

    /// Returns the item this arrow becomes when picked up (vanilla
    /// `getDefaultPickupItem`).
    fn get_default_pickup_item(&self) -> ItemRef;

    /// Returns the sound used when this arrow hits the ground.
    fn get_default_hit_ground_sound_event(&self) -> SoundEventRef {
        &sound_events::ENTITY_ARROW_HIT
    }

    /// Vanilla `getWaterInertia`; a thrown trident keeps almost all its speed.
    fn get_water_inertia(&self) -> f32 {
        WATER_INERTIA
    }

    /// Vanilla `getWeaponItem`: the weapon enchantments applied on hit.
    fn get_weapon_item(&self) -> Option<ItemStack> {
        self.abstract_arrow_base().fired_from_weapon()
    }

    /// Applies subclass effects after a successful hit (potion effects,
    /// glowing, …).
    fn do_post_hurt_effects(&self, _mob: &SharedEntity) {}

    /// Vanilla `AbstractArrow.isCritArrow`.
    fn is_crit_arrow(&self) -> bool {
        self.arrow_flag(FLAG_CRIT)
    }

    /// Vanilla `AbstractArrow.setCritArrow`.
    fn set_crit_arrow(&self, crit: bool) {
        self.set_arrow_flag(FLAG_CRIT, crit);
    }

    /// Vanilla `AbstractArrow.getPierceLevel`.
    fn get_pierce_level(&self) -> i8 {
        self.with_arrow_data(|data| *data.pierce_level.get())
    }

    /// Vanilla `AbstractArrow.setPierceLevel`.
    fn set_pierce_level(&self, pierce_level: i8) {
        self.with_arrow_data(|data| data.pierce_level.set(pierce_level));
    }

    /// Vanilla `AbstractArrow.isInGround`.
    fn is_in_ground(&self) -> bool {
        self.with_arrow_data(|data| *data.in_ground.get())
    }

    /// Vanilla `AbstractArrow.setInGround`, including the shake animation that
    /// `onSyncedDataUpdated` starts on the client.
    fn set_in_ground(&self, in_ground: bool) {
        let changed = self.with_arrow_data(|data| {
            let changed = *data.in_ground.get() != in_ground;
            data.in_ground.set(in_ground);
            changed
        });
        if changed
            && in_ground
            && !self.is_first_tick()
            && self.abstract_arrow_base().shake_time() <= 0
        {
            self.abstract_arrow_base().set_shake_time(SHAKE_TIME);
        }
    }

    /// Vanilla `AbstractArrow.setNoPhysics`: mirrors the flag to clients.
    fn set_arrow_no_physics(&self, no_physics: bool) {
        self.set_no_physics(no_physics);
        self.set_arrow_flag(FLAG_NO_PHYSICS, no_physics);
    }

    /// Vanilla `AbstractArrow.isNoPhysics`. Steel is server-only, so the server
    /// field is authoritative and the synced flag exists for clients.
    fn arrow_no_physics(&self) -> bool {
        self.no_physics()
    }

    /// Vanilla `AbstractArrow.getBaseDamage`.
    fn get_base_damage(&self) -> f64 {
        self.abstract_arrow_base().base_damage()
    }

    /// Vanilla `AbstractArrow.setBaseDamage`.
    fn set_base_damage(&self, base_damage: f64) {
        self.abstract_arrow_base().set_base_damage(base_damage);
    }

    /// Vanilla `AbstractArrow.setBaseDamageFromMob`: mob-fired arrows scale
    /// with the shooter's charge and the world difficulty.
    fn set_base_damage_from_mob(&self, power: f32) {
        let difficulty = self
            .level()
            .map_or(0.0, |world| f64::from(world.difficulty() as u8));
        self.set_base_damage(f64::from(power) * 2.0 + triangle_random(difficulty * 0.11, 0.574_25));
    }

    /// Vanilla `AbstractArrow.getPickupItem`.
    fn get_pickup_item(&self) -> ItemStack {
        self.abstract_arrow_base().pickup_item_stack()
    }

    /// Vanilla `AbstractArrow.getPickupItemStackOrigin`.
    fn get_pickup_item_stack_origin(&self) -> ItemStack {
        self.abstract_arrow_base().pickup_item_stack()
    }

    /// Vanilla `AbstractArrow.setPickupItemStack`: an empty stack falls back to
    /// the concrete arrow's default pickup item.
    fn set_pickup_item_stack(&self, stack: ItemStack) {
        let stack = if stack.is_empty() {
            ItemStack::new(self.get_default_pickup_item())
        } else {
            stack
        };
        self.abstract_arrow_base().set_pickup_item_stack(stack);
    }

    /// Vanilla `AbstractArrow.tickDespawn`.
    fn tick_despawn(&self) {
        if self.abstract_arrow_base().increment_life() >= DESPAWN_TICKS {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    /// Vanilla `AbstractArrow.shouldFall`: a stuck arrow starts falling again
    /// once the block that held it is gone.
    fn should_fall(&self) -> bool {
        if !self.is_in_ground() {
            return false;
        }
        let Some(world) = self.level() else {
            return false;
        };
        let position = self.position();
        let bounds = WorldAabb::new(
            position.x, position.y, position.z, position.x, position.y, position.z,
        )
        .inflate(0.06);
        !has_block_collision(
            &WorldCollisionProvider::for_entity(&world, self.as_entity_event_source()),
            bounds,
        )
    }

    /// Vanilla `AbstractArrow.startFalling`.
    fn start_falling(&self) {
        self.set_in_ground(false);
        let velocity = self.velocity();
        self.set_velocity(DVec3::new(
            velocity.x * f64::from(rand::random::<f32>() * 0.2),
            velocity.y * f64::from(rand::random::<f32>() * 0.2),
            velocity.z * f64::from(rand::random::<f32>() * 0.2),
        ));
        self.abstract_arrow_base().set_life(0);
    }

    /// Applies the `AbstractArrow` constructor body: pickup stack (including the
    /// `INTANGIBLE_PROJECTILE` creative-only rule), weapon piercing and owner
    /// pickup promotion.
    fn init_arrow(
        &self,
        pickup_item_stack: ItemStack,
        fired_from_weapon: Option<&ItemStack>,
        owner: Option<&SharedEntity>,
    ) {
        let intangible = pickup_item_stack.has(INTANGIBLE_PROJECTILE);
        self.set_pickup_item_stack(pickup_item_stack);
        if intangible {
            self.abstract_arrow_base().set_pickup(Pickup::CreativeOnly);
        }

        if let Some(weapon) = fired_from_weapon {
            if weapon.is_empty() {
                log::warn!(
                    "invalid empty weapon firing arrow entity {}; ignoring the weapon",
                    self.id()
                );
            } else {
                self.abstract_arrow_base()
                    .set_fired_from_weapon(Some(weapon.clone()));
                let pierce_level = piercing_count(weapon);
                if pierce_level > 0 {
                    self.set_pierce_level(pierce_level as i8);
                }
            }
        }

        self.set_arrow_owner(owner);
    }

    /// Vanilla `AbstractArrow.setOwner` pickup promotion.
    fn set_arrow_owner(&self, owner: Option<&SharedEntity>) {
        self.set_owner_entity(owner);
        let Some(owner) = owner else {
            return;
        };
        if owner.as_player().is_some() && self.abstract_arrow_base().pickup() == Pickup::Disallowed
        {
            self.abstract_arrow_base().set_pickup(Pickup::Allowed);
        } else if owner.entity_type() == &vanilla_entities::OMINOUS_ITEM_SPAWNER {
            self.abstract_arrow_base().set_pickup(Pickup::Disallowed);
        }
    }

    /// Vanilla `AbstractArrow.tick`.
    fn abstract_arrow_tick(self: Arc<Self>) {
        let Some(world) = self.level() else {
            return;
        };

        // Vanilla `Entity.setOldPosAndRot()` runs before the entity tick; Steel
        // projectiles snapshot it at the top of their tick.
        self.set_old_position_to_current();
        self.base().set_old_rotation_to_current();

        let physics_enabled = !self.arrow_no_physics();
        let movement = self.velocity();
        let block_pos = self.block_position();
        let block_state = world.get_block_state(block_pos);

        if !block_state.is_air() && physics_enabled {
            let shape = block_state.get_collision_shape_at(block_pos);
            if !shape.is_empty() {
                let position = self.position();
                let offset = DVec3::new(
                    f64::from(block_pos.x()),
                    f64::from(block_pos.y()),
                    f64::from(block_pos.z()),
                );
                for aabb in shape.iter() {
                    if aabb.translate(offset).contains(position) {
                        self.set_velocity(DVec3::ZERO);
                        self.set_in_ground(true);
                        break;
                    }
                }
            }
        }

        let shake_time = self.abstract_arrow_base().shake_time();
        if shake_time > 0 {
            self.abstract_arrow_base().set_shake_time(shake_time - 1);
        }

        if self.is_in_water_or_rain() {
            self.clear_fire();
        }

        if self.is_in_ground() && physics_enabled {
            let base = self.abstract_arrow_base();
            if base.last_state() != Some(block_state) && self.should_fall() {
                self.start_falling();
            } else {
                self.tick_despawn();
            }
            base.increment_in_ground_time();
            if self.is_alive() {
                self.apply_effects_from_blocks();
            }
            // Vanilla mirrors the fire flag into the shared entity flags for
            // clients here; Steel syncs fire/freeze through entity data.
        } else {
            self.abstract_arrow_base().reset_in_ground_time();
            let original_position = self.position();
            if self.is_in_water() {
                let inertia = f64::from(self.get_water_inertia());
                self.set_velocity(self.velocity() * inertia);
                // VANILLA CLIENT-LOCAL: the trailing bubble particles are
                // spawned by clients in `AbstractArrow.tick`.
            }

            if self.is_crit_arrow() {
                // VANILLA CLIENT-LOCAL: the four crit particles are spawned by
                // clients in `AbstractArrow.tick`.
            }

            let yaw = if physics_enabled {
                movement.x.atan2(movement.z).to_degrees() as f32
            } else {
                (-movement.x).atan2(-movement.z).to_degrees() as f32
            };
            let horizontal = (movement.x * movement.x + movement.z * movement.z).sqrt();
            let pitch = movement.y.atan2(horizontal).to_degrees() as f32;
            let (current_yaw, current_pitch) = self.rotation();
            self.set_rotation((
                lerp_rotation(current_yaw, yaw),
                lerp_rotation(current_pitch, pitch),
            ));

            self.check_left_owner();
            if physics_enabled {
                Arc::clone(&self).step_move_and_hit();
            } else {
                if let Err(error) = self.try_set_position(original_position + movement) {
                    log::debug!(
                        "failed to advance arrow {} without physics: {error}",
                        self.id()
                    );
                    self.set_removed(RemovalReason::Discarded);
                    return;
                }
                self.apply_effects_from_blocks();
            }

            if !self.is_in_water() {
                self.set_velocity(self.velocity() * f64::from(AIR_INERTIA));
            }

            if physics_enabled && !self.is_in_ground() {
                self.apply_gravity();
            }

            self.projectile_base_tick();
        }
    }

    /// Vanilla `AbstractArrow.stepMoveAndHit`.
    ///
    /// Vanilla clips the block hit once and sorts every entity hit along the
    /// move vector. Taking the nearest remaining hit repeatedly visits the same
    /// hits in the same order, because pierced entities are excluded by
    /// `can_hit_entity` and the recomputed block clip stops at the same block
    /// face.
    ///
    /// A clip that hits nothing still reports the end of the move vector in
    /// vanilla (`BlockHitResult.miss(to, …)`), so a clean miss advances the
    /// arrow by its whole velocity before the loop ends.
    fn step_move_and_hit(self: Arc<Self>) {
        while self.is_alive() {
            let from = self.position();
            let hit = self.get_hit_result_on_move_vector();
            let next = match &hit {
                Some(hit) => hit.location(),
                None => from + self.velocity(),
            };
            if let Err(error) = self.try_set_position(next) {
                log::debug!("failed to advance arrow {}: {error}", self.id());
                self.set_removed(RemovalReason::Discarded);
                return;
            }
            self.apply_effects_from_blocks_between(from, next);

            let Some(hit) = hit else {
                break;
            };
            match hit {
                ProjectileHit::Entity(_) => {
                    let deflection = Arc::clone(&self).hit_target_or_deflect_self(&hit);
                    if self.get_pierce_level() > 0 && deflection == ProjectileDeflection::None {
                        continue;
                    }
                    break;
                }
                ProjectileHit::Block { .. } => {
                    if self.is_alive() {
                        Arc::clone(&self).hit_target_or_deflect_self(&hit);
                    }
                    break;
                }
            }
        }
    }

    /// Vanilla `AbstractArrow.onHitEntity`.
    fn abstract_arrow_on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        let speed = self.velocity().length();
        let owner = self.get_owner();
        let this: SharedEntity = Arc::clone(&self) as SharedEntity;
        let mut damage_source =
            DamageSource::environment(&vanilla_damage_types::ARROW).with_direct_entity(this);
        damage_source = match &owner {
            Some(owner) => damage_source.with_causing_entity(Arc::clone(owner)),
            None => damage_source.with_causing_entity(Arc::clone(&self) as SharedEntity),
        };

        let mut arrow_damage = self.get_base_damage();
        if let Some(weapon) = self.get_weapon_item() {
            let context =
                EnchantmentDamageContext::from_damage_source(entity.entity_type(), &damage_source);
            arrow_damage = f64::from(enchantment_helper::modify_damage(
                &weapon,
                &context,
                arrow_damage as f32,
            ));
        }

        let mut damage = (speed * arrow_damage)
            .clamp(0.0, f64::from(i32::MAX))
            .ceil() as i32;

        if self.get_pierce_level() > 0 {
            if self.abstract_arrow_base().piercing_ignore_count()
                > usize::from(self.get_pierce_level().max(0) as u8)
            {
                self.set_removed(RemovalReason::Discarded);
                return;
            }
            self.abstract_arrow_base().add_piercing_ignore(entity.id());
        }

        if self.is_crit_arrow() {
            let bonus = rand::random_range(0..(damage / 2 + 2));
            damage = bonus.saturating_add(damage);
        }

        if let Some(living_owner) = owner.as_ref().and_then(|owner| owner.as_living_entity()) {
            living_owner.set_last_hurt_mob(Some(entity));
        }

        let remaining_fire_ticks = entity.remaining_fire_ticks();
        if self.is_on_fire() {
            entity.ignite_for_ticks(100);
        }

        let Some(world) = self.level() else {
            return;
        };

        if entity.hurt(&world, &damage_source, damage as f32) {
            if let Some(mob) = entity.as_living_entity() {
                // TODO: Increment vanilla `LivingEntity.arrowCount` once Steel
                // syncs it; the render-only counter has no entity-data field.
                self.do_knockback(mob, &damage_source);
                if let Some(weapon) = self.get_weapon_item() {
                    let context = EnchantmentPostAttackContext::new(entity, &damage_source);
                    enchantment_helper::do_post_attack_effects_with_item_source(
                        &world,
                        entity.as_ref(),
                        &weapon,
                        &context,
                    );
                }
                self.do_post_hurt_effects(entity);

                if let Some(victim_player) = entity.as_player()
                    && let Some(owner_player) = owner.as_ref().and_then(|owner| owner.as_player())
                    && !self.is_silent()
                    && !ptr::eq(victim_player, owner_player)
                {
                    owner_player.send_packet(CGameEvent {
                        event: GameEventType::PlayArrowHitSound,
                        data: 0.0,
                    });
                }

                if !entity.is_alive() {
                    self.abstract_arrow_base()
                        .add_pierced_and_killed_entity(Arc::clone(entity));
                }
                // TODO: Trigger `CriteriaTriggers.KILLED_BY_ARROW` once the
                // advancement criteria foundation exists.
            }

            self.play_sound(
                self.abstract_arrow_base().sound_event(),
                1.0,
                1.2 / (rand::random::<f32>() * 0.2 + 0.9),
            );
            if self.get_pierce_level() <= 0 {
                self.set_removed(RemovalReason::Discarded);
            }
        } else {
            // Vanilla `Entity.projectileReceivesSideEffectsOnHit` is true for
            // every entity type, so the side-effect branch is always taken.
            entity.set_remaining_fire_ticks(remaining_fire_ticks);
            let owner_reference = self.owner_uuid().map(EntityReference::from_uuid);
            self.deflect(
                ProjectileDeflection::Reverse,
                Some(entity.as_ref()),
                owner_reference,
                false,
            );
            if self.velocity().length_squared() < 1.0e-7 {
                if self.abstract_arrow_base().pickup() == Pickup::Allowed {
                    self.spawn_at_location(self.get_pickup_item(), 0.1);
                }
                self.set_removed(RemovalReason::Discarded);
            }
        }
    }

    /// Vanilla `AbstractArrow.doKnockback`.
    fn do_knockback(&self, mob: &dyn LivingEntity, damage_source: &DamageSource) {
        let Some(weapon) = self.get_weapon_item() else {
            return;
        };
        let context =
            EnchantmentDamageContext::from_damage_source(mob.entity_type(), damage_source);
        let knockback = enchantment_helper::modify_knockback(&weapon, &context, 0.0);
        if knockback <= 0.0 {
            return;
        }
        let resistance = (1.0 - mob.knockback_resistance()).max(0.0);
        let velocity = self.velocity();
        let horizontal = DVec3::new(velocity.x, 0.0, velocity.z);
        if horizontal.length_squared() <= 0.0 {
            return;
        }
        let push = horizontal.normalize() * f64::from(knockback * 0.6) * resistance;
        if push.length_squared() > 0.0 {
            mob.push_impulse(DVec3::new(push.x, 0.1, push.z));
        }
    }

    /// Vanilla `AbstractArrow.onHitBlock`.
    fn abstract_arrow_on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        let Some(world) = self.level() else {
            return;
        };
        self.abstract_arrow_base()
            .set_last_state(Some(world.get_block_state(hit.block_pos)));
        Arc::clone(&self).projectile_on_hit_block(hit);

        // TODO: Run vanilla `hitBlockEnchantmentEffects` once
        // `enchantment_helper` exposes an `on_hit_block` fold; no bow, crossbow
        // or trident enchantment in vanilla data has block-hit effects today.

        let movement = self.velocity();
        let offset = DVec3::new(
            movement.x.signum(),
            movement.y.signum(),
            movement.z.signum(),
        ) * 0.05;
        if let Err(error) = self.try_set_position(self.position() - offset) {
            log::debug!("failed to settle stuck arrow {}: {error}", self.id());
        }
        self.set_velocity(DVec3::ZERO);
        self.play_sound(
            self.abstract_arrow_base().sound_event(),
            1.0,
            1.2 / (rand::random::<f32>() * 0.2 + 0.9),
        );
        self.set_in_ground(true);
        self.abstract_arrow_base().set_shake_time(SHAKE_TIME);
        self.set_crit_arrow(false);
        self.set_pierce_level(0);
        self.abstract_arrow_base()
            .set_sound_event(&sound_events::ENTITY_ARROW_HIT);
        self.abstract_arrow_base().reset_pierced_entities();
    }

    /// Vanilla `AbstractArrow.playerTouch`.
    fn arrow_player_touch(self: Arc<Self>, player: &Arc<Player>) {
        if (self.is_in_ground() || self.arrow_no_physics())
            && self.abstract_arrow_base().shake_time() <= 0
            && self.try_pickup(player)
        {
            self.set_removed(RemovalReason::Discarded);
        }
    }

    /// Vanilla `AbstractArrow.tryPickup`.
    fn try_pickup(&self, player: &Arc<Player>) -> bool {
        match self.abstract_arrow_base().pickup() {
            Pickup::Disallowed => false,
            Pickup::Allowed => {
                let mut stack = self.get_pickup_item();
                let added = player.inventory.lock().add(&mut stack);
                // Vanilla `Player.take(this, 1)` only records pickup stats for
                // `ItemEntity` sources, so an arrow pickup has no other effect.
                added
            }
            Pickup::CreativeOnly => player.has_infinite_materials(),
        }
    }

    /// Vanilla `AbstractArrow.canHitEntity`.
    fn arrow_can_hit_entity(&self, entity: &dyn Entity) -> bool {
        // TODO: Apply vanilla `Player.canHarmPlayer` team checks once the
        // scoreboard team damage rules are available to entity code.
        self.base_can_hit_entity(entity)
            && !self.abstract_arrow_base().is_piercing_ignored(entity.id())
    }

    /// Vanilla `AbstractArrow.addAdditionalSaveData`.
    fn save_arrow(&self, nbt: &mut NbtCompound) {
        let base = self.abstract_arrow_base();
        nbt.insert("life", base.life() as i16);
        if let Some(last_state) = base.last_state() {
            nbt.insert(
                "inBlockState",
                NbtTag::Compound(block_state_nbt::save(last_state)),
            );
        }
        nbt.insert("shake", base.shake_time() as i8);
        nbt.insert("inGround", i8::from(self.is_in_ground()));
        nbt.insert("pickup", base.pickup().legacy_byte());
        nbt.insert("damage", base.base_damage());
        nbt.insert("crit", i8::from(self.is_crit_arrow()));
        nbt.insert("PierceLevel", self.get_pierce_level());
        nbt.insert("SoundEvent", base.sound_event().key.to_string());
        nbt.insert("item", self.get_pickup_item().to_nbt_tag());
        if let Some(weapon) = base.fired_from_weapon() {
            nbt.insert("weapon", weapon.to_nbt_tag());
        }
    }

    /// Vanilla `AbstractArrow.readAdditionalSaveData`.
    fn load_arrow(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        let base = self.abstract_arrow_base();
        base.set_life(i32::from(nbt.short("life").unwrap_or(0)));
        base.set_last_state(nbt.compound("inBlockState").and_then(block_state_nbt::load));
        base.set_shake_time(i32::from(nbt.byte("shake").unwrap_or(0) as u8));
        self.set_in_ground(nbt.byte("inGround").is_some_and(|value| value != 0));
        base.set_base_damage(nbt.double("damage").unwrap_or(ARROW_BASE_DAMAGE));
        base.set_pickup(nbt.byte("pickup").map_or(Pickup::Disallowed, |ordinal| {
            Pickup::by_ordinal(i32::from(ordinal))
        }));
        self.set_crit_arrow(nbt.byte("crit").is_some_and(|value| value != 0));
        self.set_pierce_level(nbt.byte("PierceLevel").unwrap_or(0));
        let sound_event = nbt
            .string("SoundEvent")
            .and_then(|key| Identifier::from_str(key.to_str().as_ref()).ok())
            .and_then(|identifier| REGISTRY.sound_events.by_key(&identifier))
            .unwrap_or_else(|| self.get_default_hit_ground_sound_event());
        base.set_sound_event(sound_event);
        let pickup_item = nbt
            .compound("item")
            .and_then(|tag| ItemStack::from_borrowed_compound(&tag))
            .unwrap_or_else(|| ItemStack::new(self.get_default_pickup_item()));
        self.set_pickup_item_stack(pickup_item);
        base.set_fired_from_weapon(
            nbt.compound("weapon")
                .and_then(|tag| ItemStack::from_borrowed_compound(&tag)),
        );
    }

    /// Returns whether this arrow is redirected by other projectiles (vanilla
    /// `Entity.isAttackable`).
    fn arrow_is_attackable(&self) -> bool {
        REGISTRY
            .entity_types
            .is_in_tag(self.entity_type(), &EntityTypeTag::REDIRECTABLE_PROJECTILE)
    }

    /// Vanilla `AbstractArrow.isPushedByFluid`.
    fn arrow_is_pushed_by_fluid(&self) -> bool {
        !self.is_in_ground()
    }

    /// Vanilla `AbstractArrow.isPickable`.
    fn arrow_is_pickable(&self) -> bool {
        !self.is_removed() && !self.is_in_ground()
    }

    /// Vanilla `AbstractArrow.shouldBounceOnWorldBorder`.
    fn arrow_should_bounce_on_world_border(&self) -> bool {
        true
    }

    /// Vanilla `Entity.isInWaterOrRain`, used to extinguish a flying arrow.
    fn is_in_water_or_rain(&self) -> bool {
        self.is_in_water()
            || self
                .level()
                .is_some_and(|world| world.is_raining_at(self.block_position()))
    }

    /// Reads one synced `id_flags` bit.
    fn arrow_flag(&self, flag: i8) -> bool {
        self.with_arrow_data(|data| (*data.id_flags.get() & flag) != 0)
    }

    /// Sets or clears one synced `id_flags` bit.
    fn set_arrow_flag(&self, flag: i8, value: bool) {
        self.with_arrow_data(|data| {
            let flags = *data.id_flags.get();
            let flags = if value { flags | flag } else { flags & !flag };
            data.id_flags.set(flags);
        });
    }
}

/// Vanilla `EnchantmentHelper.getPiercingCount` for an arrow's fired weapon.
///
/// `Piercing` stores an unconditional `projectile_piercing` value effect, so the
/// parsed value is read directly (`enchantment_helper` has no fold for this
/// component yet).
#[must_use]
pub fn piercing_count(weapon: &ItemStack) -> i32 {
    let count = weapon.apply_unconditional_enchantment_value_effects(
        EnchantmentEffectComponent::ProjectilePiercing,
        0.0,
    );
    count.max(0.0) as i32
}

/// Vanilla `ProjectileUtil.getMobArrow`: mob-fired arrows use the projectile
/// item's arrow factory, fall back to a plain arrow, and scale their damage with
/// the mob's charge.
///
/// Vanilla returns an `AbstractArrow`. Every vanilla `getMobArrow` call site
/// (skeleton, stray, bogged, illusioner) holds a plain or tipped arrow, so the
/// arrow entity is returned directly; a spectral arrow item would produce a
/// `SpectralArrow` in vanilla, which Steel logs instead of mistyping.
#[must_use]
pub fn get_mob_arrow(
    mob: &SharedEntity,
    projectile: ItemStack,
    power: f32,
    fired_from_weapon: Option<ItemStack>,
) -> ArrowEntity {
    let Some(world) = mob.level() else {
        log::debug!(
            "mob arrow requested for entity {} without a world; creating an unattached arrow",
            mob.id()
        );
        return ArrowEntity::new(
            &vanilla_entities::ARROW,
            next_entity_id(),
            mob.position(),
            Weak::<World>::new(),
        );
    };

    if projectile.is(&vanilla_items::SPECTRAL_ARROW) {
        log::debug!(
            "vanilla getMobArrow would create a spectral arrow for entity {}; Steel's mob-arrow factory returns a plain arrow",
            mob.id()
        );
    }

    let position = mob.position();
    let arrow = ArrowEntity::new(
        &vanilla_entities::ARROW,
        next_entity_id(),
        DVec3::new(
            position.x,
            mob.get_eye_y() - ARROW_SPAWN_EYE_OFFSET,
            position.z,
        ),
        Arc::downgrade(&world),
    );
    arrow.init_arrow(projectile, fired_from_weapon.as_ref(), Some(mob));
    arrow.set_base_damage_from_mob(power);
    arrow
}
