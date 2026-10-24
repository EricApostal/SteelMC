//! Thrown trident projectile entity (`ThrownTrident`).
//!
//! Mirrors vanilla `ThrownTrident` on top of the shared [`AbstractArrow`]
//! behavior. A trident hits exactly once for a flat damage value (8.0 before
//! enchantment modifiers, independent of its speed, and it never pierces),
//! remembers the enchantments of the stack it was thrown from, and — when that
//! stack carries `Loyalty` — flies back to its owner after the hit, dragging the
//! owner toward it until it is close enough to be caught. The loyalty level and
//! the enchantment glint are synced so clients can render the returning
//! trident. Vanilla's `shouldRender` (always true) is client-local and has no
//! server counterpart.
//!
//! GAP(`thrown_trident`): channeling does not summon its lightning bolt or play
//! `ITEM_TRIDENT_THUNDER`. The enchantment data is present
//! (`CHANNELING.effects.post_attack` = `SummonEntity { LIGHTNING_BOLT }` +
//! `PlaySound { ITEM_TRIDENT_THUNDER, 5.0, 1.0 }`) and the bolt path exists
//! (`World::spawn_lightning_bolt_with_cause`), but `enchantment_helper` cannot
//! run this effect: `entity_effect_is_supported` rejects both
//! `EnchantmentEntityEffect::SummonEntity` and
//! `EnchantmentEntityEffect::PlaySound`, and the effect's `minecraft:weather_check`
//! requirement parses as `EnchantmentEffectRequirements::Unsupported`, so the
//! post-attack fold silently skips it. Wiring the effect belongs in
//! `enchantment_helper`, not here.
//! GAP(`thrown_trident`): `hitBlockEnchantmentEffects` (block-hit enchantment
//! effects with an `onBreak` callback) is not run — `enchantment_helper`
//! exposes no `on_hit_block` fold (also TODO in [`AbstractArrow`]).
//! GAP(`thrown_trident`): the weapon-break callback that kills the trident is not
//! invoked — `enchantment_helper::do_post_attack_effects_with_item_source` has
//! no `on_break` callback.
//! GAP(`thrown_trident`): vanilla deflects with
//! `new Vec3(0.02, 0.2, 0.02)` as the power vector (the trident nearly stops
//! after a hit), while `Projectile::deflect` takes no power vector and
//! `ProjectileDeflection::Reverse` always scales by `-0.5`, so the trident
//! bounces off at full strength instead.
//! GAP(`thrown_trident`): vanilla gates the hit sound and the bounce on
//! `Entity.projectileReceivesSideEffectsOnHit(wasHurt)` (overridden by
//! `Enderman` to `wasHurt`), which Steel's [`Entity`] trait does not expose, so
//! an unhurt Enderman still plays `ITEM_TRIDENT_HIT` and deflects the trident.
//! GAP(`thrown_trident`): the old-position snapshot used for client interpolation
//! is taken after the loyalty pull instead of before it —
//! `AbstractArrow::abstract_arrow_tick` snapshots unconditionally at its start
//! and offers no pre-tick hook.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::vanilla_components::ENCHANTMENT_GLINT_OVERRIDE;
use steel_registry::enchantment_effect::EnchantmentEffectComponent;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_entity_data::{AbstractArrowEntityData, ThrownTridentEntityData};
use steel_registry::{sound_events, vanilla_damage_types, vanilla_items};
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::enchantment_helper::{self, EnchantmentDamageContext, EnchantmentPostAttackContext};
use crate::entity::damage::DamageSource;
use crate::entity::entities::objects::projectiles::abstract_arrow::{
    AbstractArrow, AbstractArrowBase, Pickup,
};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntityReference, EntitySyncedData, Projectile,
    ProjectileBase, ProjectileDeflection, RemovalReason, SharedEntity,
};
use crate::inventory::container::Container as _;
use crate::player::Player;
use crate::world::{ClipHitResult, World};

/// Vanilla `AbstractArrow.getDefaultGravity`.
const ARROW_GRAVITY: f64 = 0.05;

/// Vanilla `ThrownTrident.WATER_INERTIA`: a trident keeps almost all of its
/// speed in water.
const WATER_INERTIA: f32 = 0.99;

/// Vanilla `ThrownTrident.onHitEntity`: flat damage before enchantment
/// modifiers (a trident does not scale with projectile speed).
const TRIDENT_BASE_DAMAGE: f32 = 8.0;

/// Vanilla `AbstractArrow.tickDespawn` lifespan, in ticks.
///
/// The shared [`AbstractArrowBase`] keeps its own copy of this constant
/// private, so the overridden despawn check mirrors the value here.
const DESPAWN_TICKS: i32 = 1200;

/// Vanilla `ThrownTrident.tick`: ticks of being stuck before the trident is
/// considered to have dealt its damage and starts returning.
const IN_GROUND_DEALT_DAMAGE_TIME: i32 = 4;

/// A thrown trident.
#[entity_behavior(class = "ThrownTrident")]
pub struct ThrownTridentEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced `AbstractArrow` data plus this trident's loyalty and foil flags.
    entity_data: SyncMutex<ThrownTridentEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Non-synced `AbstractArrow` state (life, pickup, shake, damage, …).
    arrow_base: AbstractArrowBase,
    /// Vanilla `ThrownTrident.dealtDamage`: the trident hits at most once.
    dealt_damage: SyncMutex<bool>,
    /// Vanilla `ThrownTrident.clientSideReturnTridentTickCount`: guards the
    /// one-time return sound while flying back to the owner.
    return_trident_tick_count: SyncMutex<i32>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ThrownTridentEntity`.
unsafe impl DowncastType for ThrownTridentEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/thrown_trident");
}

impl ThrownTridentEntity {
    /// Creates a new trident with no owner and a default pickup stack.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::with_state(
            entity_type,
            EntityBase::new(id, position, entity_type.dimensions, world),
        )
    }

    /// Creates a trident from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::with_state(
            entity_type,
            EntityBase::from_load(load, entity_type.dimensions),
        )
    }

    /// Vanilla `ThrownTrident(Level, double, double, double, ItemStack)`: a
    /// trident thrown without an owner, taking its loyalty and glint from the
    /// item stack it was created from.
    #[must_use]
    pub fn from_item(
        entity_type: EntityTypeRef,
        id: i32,
        position: DVec3,
        world: Weak<World>,
        trident_item: ItemStack,
    ) -> Self {
        let trident = Self::new(entity_type, id, position, world);
        // Vanilla's position constructor passes the trident stack as both the
        // pickup stack and the firing weapon.
        trident.init_trident_item(&trident_item, None, Some(&trident_item));
        trident
    }

    /// Vanilla `ThrownTrident(Level, LivingEntity, ItemStack)`: a trident
    /// thrown by `owner`, spawned at the owner's eye height minus `0.1`.
    /// Vanilla passes no firing weapon to the superclass here, so block-hit
    /// and enchantment lookups fall back to the pickup stack.
    #[must_use]
    pub fn from_owner(
        entity_type: EntityTypeRef,
        id: i32,
        owner: &SharedEntity,
        world: Weak<World>,
        trident_item: ItemStack,
    ) -> Self {
        let position = owner.position();
        let trident = Self::new(
            entity_type,
            id,
            DVec3::new(position.x, owner.get_eye_y() - 0.1, position.z),
            world,
        );
        trident.init_trident_item(&trident_item, Some(owner), None);
        trident
    }

    fn with_state(entity_type: EntityTypeRef, base: EntityBase) -> Self {
        Self {
            base,
            entity_type,
            entity_data: SyncMutex::new(ThrownTridentEntityData::new()),
            projectile_base: ProjectileBase::new(),
            arrow_base: AbstractArrowBase::new(
                ItemStack::new(&vanilla_items::TRIDENT),
                &sound_events::ITEM_TRIDENT_HIT_GROUND,
            ),
            dealt_damage: SyncMutex::new(false),
            return_trident_tick_count: SyncMutex::new(0),
        }
    }

    /// Vanilla `ThrownTrident` constructors after `super(...)`: seeds the pickup
    /// stack, the firing weapon and the owner, then copies the loyalty level and
    /// the glint flag from the item.
    fn init_trident_item(
        &self,
        trident_item: &ItemStack,
        owner: Option<&SharedEntity>,
        fired_from_weapon: Option<&ItemStack>,
    ) {
        self.init_arrow(trident_item.clone(), fired_from_weapon, owner);
        self.set_loyalty(Self::loyalty_from_item(trident_item));
        self.set_foil(Self::foil_from_item(trident_item));
    }

    /// Vanilla `ThrownTrident.getLoyaltyFromItem`: the loyalty acceleration
    /// value clamped to a synced byte. Steel is server-only, so the vanilla
    /// `ServerLevel` branch always applies.
    fn loyalty_from_item(trident_item: &ItemStack) -> i8 {
        let acceleration = trident_item.apply_unconditional_enchantment_value_effects(
            EnchantmentEffectComponent::TridentReturnAcceleration,
            0.0,
        );
        (acceleration as i32).clamp(0, 127) as i8
    }

    /// Vanilla `ItemStack.hasFoil`: the glint override wins, otherwise the stack
    /// glints when enchanted.
    fn foil_from_item(trident_item: &ItemStack) -> bool {
        trident_item
            .get(ENCHANTMENT_GLINT_OVERRIDE)
            .copied()
            .unwrap_or_else(|| trident_item.is_enchanted())
    }

    /// Returns the synced loyalty level (vanilla `ID_LOYALTY`).
    #[must_use]
    pub fn loyalty(&self) -> i8 {
        *self.entity_data.lock().id_loyalty.get()
    }

    /// Sets the synced loyalty level.
    pub fn set_loyalty(&self, loyalty: i8) {
        self.entity_data.lock().id_loyalty.set(loyalty);
    }

    /// Returns whether the trident glints (vanilla `isFoil`).
    #[must_use]
    pub fn is_foil(&self) -> bool {
        *self.entity_data.lock().id_foil.get()
    }

    /// Sets the synced glint flag.
    pub fn set_foil(&self, foil: bool) {
        self.entity_data.lock().id_foil.set(foil);
    }

    /// Returns whether the trident already dealt its damage (vanilla
    /// `dealtDamage`).
    #[must_use]
    pub fn dealt_damage(&self) -> bool {
        *self.dealt_damage.lock()
    }

    /// Sets whether the trident already dealt its damage.
    pub fn set_dealt_damage(&self, dealt_damage: bool) {
        *self.dealt_damage.lock() = dealt_damage;
    }

    /// Vanilla `ThrownTrident.isAcceptibleReturnOwner`.
    fn is_acceptable_return_owner(&self) -> bool {
        let Some(owner) = self.get_owner() else {
            return false;
        };
        owner.is_alive() && !owner.is_spectator()
    }

    /// Vanilla `ThrownTrident.tick`'s loyalty block, run before the shared
    /// `AbstractArrow.tick` body.
    ///
    /// Returns `false` when vanilla returns out of `tick()` (the trident was
    /// just caught by a non-player owner), so the caller skips the shared arrow
    /// tick for this tick.
    fn tick_loyalty(&self) -> bool {
        if self.abstract_arrow_base().in_ground_time() > IN_GROUND_DEALT_DAMAGE_TIME {
            self.set_dealt_damage(true);
        }

        let loyalty = self.loyalty();
        if loyalty <= 0 || !(self.dealt_damage() || self.arrow_no_physics()) {
            return true;
        }
        let Some(owner) = self.get_owner() else {
            return true;
        };

        if !self.is_acceptable_return_owner() {
            if self.abstract_arrow_base().pickup() == Pickup::Allowed {
                let _ = self.spawn_at_location(self.get_pickup_item(), 0.1);
            }
            self.set_removed(RemovalReason::Discarded);
            return true;
        }

        let owner_eye = DVec3::new(owner.position().x, owner.get_eye_y(), owner.position().z);
        let position = self.position();
        if owner.as_player().is_none()
            && position.distance(owner_eye) < f64::from(owner.base().dimensions().width) + 1.0
        {
            // Vanilla `discard()`s and returns, skipping `super.tick()`.
            self.set_removed(RemovalReason::Discarded);
            return false;
        }

        self.set_arrow_no_physics(true);
        let toward_owner = owner_eye - position;
        let pulled = DVec3::new(
            position.x,
            position.y + toward_owner.y * 0.015 * f64::from(loyalty),
            position.z,
        );
        if let Err(error) = self.try_set_position(pulled) {
            log::debug!(
                "failed to pull returning trident {} toward its owner: {error}",
                self.id()
            );
        }
        let acceleration = 0.05 * f64::from(loyalty);
        self.set_velocity(self.velocity() * 0.95 + toward_owner.normalize_or_zero() * acceleration);

        let first_return_tick = {
            let mut count = self.return_trident_tick_count.lock();
            let first = *count == 0;
            *count += 1;
            first
        };
        if first_return_tick {
            self.play_sound(&sound_events::ITEM_TRIDENT_RETURN, 10.0, 1.0);
        }

        true
    }
}

impl Entity for ThrownTridentEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        if !self.tick_loyalty() {
            return;
        }
        Arc::clone(&self).abstract_arrow_tick();
    }

    fn get_default_gravity(&self) -> f64 {
        ARROW_GRAVITY
    }

    fn sound_source(&self) -> SoundSource {
        SoundSource::Neutral
    }

    fn spawn_data(&self) -> i32 {
        self.get_owner().map_or(0, |owner| owner.id())
    }

    fn restore_owner_reference(&self, owner: &SharedEntity) {
        self.cache_owner_entity(owner);
    }

    fn projectile_owner_uuid(&self) -> Option<uuid::Uuid> {
        self.owner_uuid()
    }

    fn projectile_owner(&self) -> Option<SharedEntity> {
        self.get_owner()
    }

    fn attackable(&self) -> bool {
        self.arrow_is_attackable()
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    /// Vanilla `ThrownTrident.playerTouch`: only the owner (or an ownerless
    /// trident) may pick the trident up.
    fn player_touch(self: Arc<Self>, player: &Arc<Player>) {
        if self.owned_by(player.as_ref()) || self.get_owner().is_none() {
            Arc::clone(&self).arrow_player_touch(player);
        }
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_arrow(nbt);
        nbt.insert("DealtDamage", self.dealt_damage());
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_arrow(nbt);
        self.set_dealt_damage(nbt.byte("DealtDamage").is_some_and(|value| value != 0));
        // Vanilla `readAdditionalSaveData` re-derives the loyalty level from
        // the loaded pickup stack rather than saving it.
        self.set_loyalty(Self::loyalty_from_item(
            &self.get_pickup_item_stack_origin(),
        ));
    }
}

impl Projectile for ThrownTridentEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    /// Vanilla `ThrownTrident.onHitEntity`: a flat, enchantment-modified hit
    /// that never pierces, always plays the trident hit sound and bounces back.
    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, _location: DVec3) {
        let Some(world) = self.level() else {
            return;
        };
        let owner = self.get_owner();
        let mut damage_source = DamageSource::environment(&vanilla_damage_types::TRIDENT)
            .with_direct_entity(Arc::clone(&self) as SharedEntity);
        damage_source = match &owner {
            Some(owner) => damage_source.with_causing_entity(Arc::clone(owner)),
            None => damage_source.with_causing_entity(Arc::clone(&self) as SharedEntity),
        };

        let mut damage = TRIDENT_BASE_DAMAGE;
        if let Some(weapon) = self.get_weapon_item() {
            let context =
                EnchantmentDamageContext::from_damage_source(entity.entity_type(), &damage_source);
            damage = enchantment_helper::modify_damage(&weapon, &context, damage);
        }

        self.set_dealt_damage(true);
        let was_hurt = entity.hurt(&world, &damage_source, damage);
        if was_hurt {
            if let Some(weapon) = self.get_weapon_item() {
                let context = EnchantmentPostAttackContext::new(entity, &damage_source);
                enchantment_helper::do_post_attack_effects_with_item_source(
                    &world,
                    entity.as_ref(),
                    &weapon,
                    &context,
                );
            }
            if let Some(mob) = entity.as_living_entity() {
                self.do_knockback(mob, &damage_source);
                self.do_post_hurt_effects(entity);
            }
        }

        // Vanilla gates this on `Entity.projectileReceivesSideEffectsOnHit`,
        // which only `Enderman` overrides; Steel has no such hook, so the sound
        // and the bounce always happen (see the module GAP list).
        self.play_sound(&sound_events::ITEM_TRIDENT_HIT, 1.0, 1.0);
        let owner_reference = self.owner_uuid().map(EntityReference::from_uuid);
        self.deflect(
            ProjectileDeflection::Reverse,
            Some(entity.as_ref()),
            owner_reference,
            false,
        );
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        self.abstract_arrow_on_hit_block(hit);
    }

    /// Vanilla `ThrownTrident.findHitEntity`: a trident that already dealt its
    /// damage hits nothing else.
    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        !self.dealt_damage() && self.arrow_can_hit_entity(entity)
    }

    fn should_bounce_on_world_border(&self) -> bool {
        self.arrow_should_bounce_on_world_border()
    }
}

impl AbstractArrow for ThrownTridentEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.arrow_base
    }

    fn with_arrow_data<R>(&self, f: impl FnOnce(&mut AbstractArrowEntityData) -> R) -> R {
        f(&mut self.entity_data.lock().abstract_arrow)
    }

    fn get_default_pickup_item(&self) -> ItemRef {
        &vanilla_items::TRIDENT
    }

    fn get_default_hit_ground_sound_event(&self) -> SoundEventRef {
        &sound_events::ITEM_TRIDENT_HIT_GROUND
    }

    fn get_water_inertia(&self) -> f32 {
        WATER_INERTIA
    }

    /// Vanilla `ThrownTrident.getWeaponItem`: the trident stack itself supplies
    /// the enchantments applied on hit.
    fn get_weapon_item(&self) -> Option<ItemStack> {
        Some(self.get_pickup_item_stack_origin())
    }

    /// Vanilla `ThrownTrident.tryPickup`: a returning trident is caught by its
    /// owner even when its pickup rule would not allow it.
    ///
    /// The first half mirrors `AbstractArrow::try_pickup` (vanilla
    /// `super.tryPickup`); Rust cannot call a trait default from its own
    /// override, so the body is repeated instead of delegated.
    fn try_pickup(&self, player: &Arc<Player>) -> bool {
        let picked_up = match self.abstract_arrow_base().pickup() {
            Pickup::Disallowed => false,
            Pickup::Allowed => {
                let mut stack = self.get_pickup_item();
                player.inventory.lock().add(&mut stack)
            }
            Pickup::CreativeOnly => player.has_infinite_materials(),
        };
        if picked_up {
            return true;
        }
        if !self.no_physics() || !self.owned_by(player.as_ref()) {
            return false;
        }
        let mut stack = self.get_pickup_item();
        player.inventory.lock().add(&mut stack)
    }

    /// Vanilla `ThrownTrident.tickDespawn`: a loyal trident that can be picked
    /// up never despawns while it is flying back.
    fn tick_despawn(&self) {
        if (self.abstract_arrow_base().pickup() != Pickup::Allowed || self.loyalty() <= 0)
            && self.abstract_arrow_base().increment_life() >= DESPAWN_TICKS
        {
            self.set_removed(RemovalReason::Discarded);
        }
    }
}
