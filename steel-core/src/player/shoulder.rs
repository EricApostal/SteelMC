//! Shoulder-riding entities (vanilla `Player` shoulder state and
//! `ServerPlayer` shoulder entity handling).
//!
//! Vanilla keeps two NBT compounds on the player (`ShoulderEntityLeft` /
//! `ShoulderEntityRight`) plus a synced parrot variant per shoulder. A parrot
//! that lands on its owner's shoulder is serialized into one of those slots and
//! discarded; the entity is respawned next to the player when the shoulder is
//! cleared.

use std::str::FromStr;
use std::sync::Arc;

use glam::DVec3;
use simdnbt::owned::NbtCompound;
use steel_registry::entity_variant::ParrotVariant;
use steel_registry::sound_events;
use steel_registry::{REGISTRY, RegistryExt as _, vanilla_entities};
use steel_utils::Identifier;

use crate::entity::{Entity as _, LivingEntity as _, add_spawned_entity, create_entity_instance};
use crate::player::Player;

/// Vanilla `ServerPlayer.playShoulderEntityAmbientSound` ambient-sound chance (1 in 200).
const SHOULDER_AMBIENT_CHANCE: i32 = 200;

/// Vanilla `Parrot.getPitch` voice pitch jitter.
const SHOULDER_PARROT_PITCH_JITTER: f32 = 0.2;

/// Vanilla `ServerPlayer.respawnEntityOnShoulder` vertical offset above the player.
const SHOULDER_RESPAWN_Y_OFFSET: f64 = 0.7;

/// Vanilla `ServerPlayer.removeEntitiesOnShoulder` grace period after perching.
const SHOULDER_PERCH_GRACE_TICKS: i64 = 20;

impl Player {
    /// Vanilla `ServerPlayer.getShoulderEntityLeft`.
    #[must_use]
    pub fn shoulder_entity_left(&self) -> NbtCompound {
        self.shoulder_entity_left.lock().clone()
    }

    /// Vanilla `ServerPlayer.getShoulderEntityRight`.
    #[must_use]
    pub fn shoulder_entity_right(&self) -> NbtCompound {
        self.shoulder_entity_right.lock().clone()
    }

    /// Vanilla `ServerPlayer.setShoulderEntityLeft`.
    ///
    /// The synced parrot variant is derived from the stored tag, exactly like
    /// vanilla's `setShoulderEntityLeft`.
    pub fn set_shoulder_entity_left(&self, tag: NbtCompound) {
        let variant = extract_parrot_variant(&tag);
        *self.shoulder_entity_left.lock() = tag;
        self.set_shoulder_parrot_left(variant);
    }

    /// Vanilla `ServerPlayer.setShoulderEntityRight`.
    pub fn set_shoulder_entity_right(&self, tag: NbtCompound) {
        let variant = extract_parrot_variant(&tag);
        *self.shoulder_entity_right.lock() = tag;
        self.set_shoulder_parrot_right(variant);
    }

    /// Vanilla `Player.getShoulderParrotLeft`.
    #[must_use]
    pub fn shoulder_parrot_left(&self) -> Option<ParrotVariant> {
        self.entity_data
            .lock()
            .shoulder_parrot_left
            .get()
            .map(|id| ParrotVariant::by_id(id as i32))
    }

    /// Vanilla `Player.getShoulderParrotRight`.
    #[must_use]
    pub fn shoulder_parrot_right(&self) -> Option<ParrotVariant> {
        self.entity_data
            .lock()
            .shoulder_parrot_right
            .get()
            .map(|id| ParrotVariant::by_id(id as i32))
    }

    /// Vanilla `Player.setShoulderParrotLeft`.
    pub fn set_shoulder_parrot_left(&self, variant: Option<ParrotVariant>) {
        self.entity_data
            .lock()
            .shoulder_parrot_left
            .set(variant.map(|variant| variant.id() as u32));
        self.sync_entity_data();
    }

    /// Vanilla `Player.setShoulderParrotRight`.
    pub fn set_shoulder_parrot_right(&self, variant: Option<ParrotVariant>) {
        self.entity_data
            .lock()
            .shoulder_parrot_right
            .set(variant.map(|variant| variant.id() as u32));
        self.sync_entity_data();
    }

    /// Vanilla `ServerPlayer.setEntityOnShoulder`.
    ///
    /// Fails while riding, airborne, in water or in powder snow; otherwise the
    /// tag fills the first empty shoulder.
    pub fn set_entity_on_shoulder(&self, tag: NbtCompound) -> bool {
        if self.is_passenger()
            || !self.on_ground()
            || self.is_in_water()
            || self.is_in_powder_snow()
        {
            return false;
        }

        if self.shoulder_entity_left.lock().is_empty() {
            self.set_shoulder_entity_left(tag);
            *self.time_entity_sat_on_shoulder.lock() = self.get_world().game_time();
            true
        } else if self.shoulder_entity_right.lock().is_empty() {
            self.set_shoulder_entity_right(tag);
            *self.time_entity_sat_on_shoulder.lock() = self.get_world().game_time();
            true
        } else {
            false
        }
    }

    /// Vanilla `ServerPlayer.handleShoulderEntities`, run at the end of `aiStep`.
    pub(crate) fn handle_shoulder_entities(&self) {
        self.play_shoulder_entity_ambient_sound(&self.shoulder_entity_left());
        self.play_shoulder_entity_ambient_sound(&self.shoulder_entity_right());
        if self.fall_distance() > 0.5
            || self.is_in_water()
            || self.is_flying()
            || self.is_sleeping()
            || self.is_in_powder_snow()
        {
            self.remove_entities_on_shoulder();
        }
    }

    /// Vanilla `ServerPlayer.playShoulderEntityAmbientSound`.
    ///
    /// Deliberate difference: vanilla's `Parrot.getAmbient` branch that plays an
    /// imitated mob sound (1 in 1000) is not applied here, because that table is
    /// private to the parrot module; the ambient parrot sound is played instead.
    fn play_shoulder_entity_ambient_sound(&self, tag: &NbtCompound) {
        if tag.is_empty() || tag.byte("Silent").is_some_and(|silent| silent != 0) {
            return;
        }
        if rand::random_range(0..SHOULDER_AMBIENT_CHANCE) != 0 {
            return;
        }

        let Some(id) = tag.string("id") else {
            return;
        };
        let Ok(id) = Identifier::from_str(&id.to_string()) else {
            return;
        };
        if id != vanilla_entities::PARROT.key {
            return;
        }

        self.get_world().play_sound_at(
            &sound_events::ENTITY_PARROT_AMBIENT,
            self.sound_source(),
            self.position(),
            1.0,
            (rand::random::<f32>() - rand::random::<f32>()) * SHOULDER_PARROT_PITCH_JITTER + 1.0,
            None,
        );
    }

    /// Vanilla `ServerPlayer.removeEntitiesOnShoulder`.
    ///
    /// The perching grace period keeps a parrot from being dismounted on the
    /// same tick that put it on the shoulder.
    pub fn remove_entities_on_shoulder(&self) {
        if *self.time_entity_sat_on_shoulder.lock() + SHOULDER_PERCH_GRACE_TICKS
            >= self.get_world().game_time()
        {
            return;
        }

        self.respawn_entity_on_shoulder(&self.shoulder_entity_left());
        self.set_shoulder_entity_left(NbtCompound::new());
        self.respawn_entity_on_shoulder(&self.shoulder_entity_right());
        self.set_shoulder_entity_right(NbtCompound::new());
    }

    /// Vanilla `ServerPlayer.respawnEntityOnShoulder`.
    fn respawn_entity_on_shoulder(&self, tag: &NbtCompound) {
        if tag.is_empty() {
            return;
        }

        let Some(id) = tag.string("id") else {
            return;
        };
        let Ok(id) = Identifier::from_str(&id.to_string()) else {
            return;
        };
        let Some(entity_type) = REGISTRY.entity_types.by_key(&id) else {
            return;
        };

        let world = self.get_world();
        let position = self.position();
        let Ok(entity) = create_entity_instance(&world, entity_type, position) else {
            return;
        };

        // The stored tag carries the tameable owner, so loading it restores the
        // same `Owner` vanilla would set explicitly through `TamableAnimal.setOwner`.
        if entity.load_with_owned_components(tag).is_err() {
            return;
        }
        let respawn_position = position + DVec3::new(0.0, SHOULDER_RESPAWN_Y_OFFSET, 0.0);
        if entity.try_set_position(respawn_position).is_ok() {
            entity.set_old_position_to_current();
        }
        if let Err(error) = add_spawned_entity(&world, Arc::clone(&entity)) {
            log::warn!(
                "Failed to respawn {} from {}'s shoulder: {error:?}",
                entity_type.key,
                self.gameprofile.name()
            );
        }
    }

    /// Returns whether either shoulder holds a stored entity.
    #[must_use]
    pub fn has_shoulder_entity(&self) -> bool {
        !self.shoulder_entity_left.lock().is_empty()
            || !self.shoulder_entity_right.lock().is_empty()
    }
}

/// Vanilla `Player.extractParrotVariant`.
fn extract_parrot_variant(tag: &NbtCompound) -> Option<ParrotVariant> {
    if tag.is_empty() {
        return None;
    }

    let id = Identifier::from_str(&tag.string("id")?.to_string()).ok()?;
    if id != vanilla_entities::PARROT.key {
        return None;
    }
    Some(ParrotVariant::by_id(tag.int("Variant").unwrap_or(0)))
}
