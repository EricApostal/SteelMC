//! Arrow projectile entity (`Arrow`).
//!
//! Mirrors vanilla `Arrow` on top of the shared [`AbstractArrow`] behavior: the
//! arrow carries potion contents in its pickup stack, exposes the resulting
//! effect colour as synced data, and applies those effects to whatever it hits.
//! A potion-tipped arrow that has been stuck in the ground for
//! `EXPOSED_POTION_DECAY_TIME` ticks loses its contents and turns back into a
//! plain arrow.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_macros::entity_behavior;
use steel_protocol::packets::game::SoundSource;
use steel_registry::data_components::PotionContents;
use steel_registry::data_components::vanilla_components::POTION_CONTENTS;
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::vanilla_entity_data::{AbstractArrowEntityData, ArrowEntityData};
use steel_registry::{sound_events, vanilla_entities, vanilla_items};
use steel_utils::entity_events::EntityStatus;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};

use crate::entity::entities::objects::projectiles::abstract_arrow::{
    AbstractArrow, AbstractArrowBase,
};
use crate::entity::entities::objects::projectiles::thrown_potion::{
    potion_color, potion_contents, potion_duration_scale,
};
use crate::entity::potion_contents::{scale_effect_duration, to_runtime_instance};
use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, Projectile, ProjectileBase, SharedEntity,
};
use crate::world::{ClipHitResult, World};

/// Vanilla `AbstractArrow.getDefaultGravity`.
const ARROW_GRAVITY: f64 = 0.05;

/// Vanilla `Arrow.EXPOSED_POTION_DECAY_TIME`: ticks stuck in the ground before a
/// potion-tipped arrow loses its contents.
const EXPOSED_POTION_DECAY_TIME: i32 = 600;

/// Vanilla `Arrow.NO_EFFECT_COLOR`.
const NO_EFFECT_COLOR: i32 = -1;

/// An arrow.
#[entity_behavior(class = "Arrow")]
pub struct ArrowEntity {
    /// Common entity fields (id, uuid, position, etc.).
    base: EntityBase,
    /// Vanilla entity type registered for this implementation.
    entity_type: EntityTypeRef,
    /// Synced `AbstractArrow` data plus this arrow's effect colour.
    entity_data: SyncMutex<ArrowEntityData>,
    /// Shared `Projectile` state (owner / left-owner / has-been-shot).
    projectile_base: ProjectileBase,
    /// Non-synced `AbstractArrow` state (life, pickup, shake, damage, …).
    arrow_base: AbstractArrowBase,
}

// SAFETY: This key is owned by Steel and uniquely identifies `ArrowEntity`.
unsafe impl DowncastType for ArrowEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/arrow");
}

impl ArrowEntity {
    /// Creates a new arrow with no owner and a plain default pickup stack.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self {
            base: EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            arrow_base: AbstractArrowBase::new(
                ItemStack::new(&vanilla_items::ARROW),
                &sound_events::ENTITY_ARROW_HIT,
            ),
        }
    }

    /// Creates an arrow from saved base data.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self {
            base: EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
            entity_data: SyncMutex::new(ArrowEntityData::new()),
            projectile_base: ProjectileBase::new(),
            arrow_base: AbstractArrowBase::new(
                ItemStack::new(&vanilla_items::ARROW),
                &sound_events::ENTITY_ARROW_HIT,
            ),
        }
    }

    /// Returns the arrow's potion contents (vanilla `getPotionContents`).
    #[must_use]
    pub fn potion_contents(&self) -> PotionContents {
        potion_contents(&self.get_pickup_item_stack_origin())
    }

    /// Returns the potion duration scale of the pickup stack (vanilla
    /// `getPotionDurationScale`).
    #[must_use]
    pub fn potion_duration_scale(&self) -> f32 {
        potion_duration_scale(&self.get_pickup_item_stack_origin())
    }

    /// Replaces the arrow's potion contents and refreshes the synced effect
    /// colour (vanilla `setPotionContents`).
    pub fn set_potion_contents(&self, contents: PotionContents) {
        let mut stack = self.get_pickup_item_stack_origin();
        stack.set(POTION_CONTENTS, contents);
        self.set_pickup_item_stack(stack);
        self.update_effect_color();
    }

    /// Adds one effect to the arrow's potion contents (vanilla `addEffect`).
    pub fn add_effect(&self, effect: steel_registry::MobEffectInstance) {
        let contents = self.potion_contents();
        let mut effects = contents.custom_effects().to_vec();
        effects.push(effect);
        self.set_potion_contents(PotionContents::new(
            contents.potion(),
            contents.custom_color(),
            effects,
            contents.custom_name().map(str::to_owned),
        ));
    }

    /// Returns the synced effect colour (vanilla `getColor`).
    #[must_use]
    pub fn color(&self) -> i32 {
        *self.entity_data.lock().id_effect_color.get()
    }

    /// Refreshes the synced effect colour from the pickup stack's potion
    /// contents (vanilla `updateColor`).
    fn update_effect_color(&self) {
        let contents = self.potion_contents();
        let color = if contents == PotionContents::empty() {
            NO_EFFECT_COLOR
        } else {
            potion_color(&contents)
        };
        self.entity_data.lock().id_effect_color.set(color);
    }

    /// Vanilla `Arrow.tick`'s server half: a potion-tipped arrow stuck in the
    /// ground long enough puffs its contents and reverts to a plain arrow.
    fn tick_exposed_potion(&self) {
        let in_ground_time = self.abstract_arrow_base().in_ground_time();
        if !self.is_in_ground() || in_ground_time < EXPOSED_POTION_DECAY_TIME {
            return;
        }
        if self.potion_contents() == PotionContents::empty() {
            return;
        }

        // Vanilla broadcasts entity event 0 so clients puff the effect colour.
        self.broadcast_entity_event(EntityStatus::TippedArrow);
        self.set_pickup_item_stack(ItemStack::new(&vanilla_items::ARROW));
    }
}

impl Entity for ArrowEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn tick(self: Arc<Self>) {
        Arc::clone(&self).abstract_arrow_tick();
        self.tick_exposed_potion();
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

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_projectile(nbt);
        self.save_arrow(nbt);
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_projectile(nbt);
        self.load_arrow(nbt);
        self.update_effect_color();
    }
}

impl Projectile for ArrowEntity {
    fn projectile_base(&self) -> &ProjectileBase {
        &self.projectile_base
    }

    fn on_hit_entity(self: Arc<Self>, entity: &SharedEntity, location: DVec3) {
        self.abstract_arrow_on_hit_entity(entity, location);
    }

    fn on_hit_block(self: Arc<Self>, hit: &ClipHitResult) {
        self.abstract_arrow_on_hit_block(hit);
    }

    fn can_hit_entity(&self, entity: &dyn Entity) -> bool {
        self.arrow_can_hit_entity(entity)
    }

    fn should_bounce_on_world_border(&self) -> bool {
        self.arrow_should_bounce_on_world_border()
    }
}

impl AbstractArrow for ArrowEntity {
    fn abstract_arrow_base(&self) -> &AbstractArrowBase {
        &self.arrow_base
    }

    fn with_arrow_data<R>(&self, f: impl FnOnce(&mut AbstractArrowEntityData) -> R) -> R {
        f(&mut self.entity_data.lock().abstract_arrow)
    }

    fn get_default_pickup_item(&self) -> ItemRef {
        &vanilla_items::ARROW
    }

    fn do_post_hurt_effects(&self, mob: &SharedEntity) {
        // Vanilla `Arrow.doPostHurtEffects`: apply the arrow's potion contents
        // to the struck mob with the arrow's duration scale.
        let Some(living) = mob.as_living_entity() else {
            return;
        };
        let contents = self.potion_contents();
        let scale = self.potion_duration_scale();
        for effect in contents.all_effects() {
            let duration = scale_effect_duration(effect.duration(), scale);
            living.add_mob_effect(to_runtime_instance(&effect, duration));
        }
    }
}
