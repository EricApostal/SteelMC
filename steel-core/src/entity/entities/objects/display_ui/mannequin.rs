//! Vanilla mannequin entity: a player-shaped display stand.
//!
//! Vanilla reference: `Mannequin` (`world/entity/decoration/Mannequin.java`)
//! over `Avatar`. Steel has no `Avatar` trait, so this implementation stores the
//! avatar layers it can express directly on the generated
//! [`MannequinEntityData`] (`avatar.player_main_hand`,
//! `avatar.player_mode_customization`, `profile`, `immovable`, `description`)
//! and persists every vanilla saved field.
//!
//! Persistence-first scope: placement, profile passthrough, pose/main-hand/
//! hidden-layer/immovable/description round-trip, and living health/equipment.
//!
//! Reported gaps (no stubs, real vanilla behavior that Steel cannot express yet):
//! - **Game-profile resolution.** Vanilla `ResolvableProfile` resolves dynamic
//!   names/ids through the session service before rendering. Steel's
//!   [`ResolvableProfile`] carries `DynamicName`/`DynamicId` contents but has no
//!   resolver service, so a mannequin keeps the unresolved profile exactly as
//!   saved (identical bytes, no skin).
//! - **Implicit item components.** Vanilla copies the `profile` item component
//!   into the entity on spawn (`applyImplicitComponents`). Steel's entity spawn
//!   path has no per-entity implicit-component hook, so the placement caller must
//!   pass the profile explicitly through NBT.
//! - **`ALL_LAYERS` default.** Vanilla seeds
//!   `DATA_PLAYER_MODE_CUSTOMISATION` with every model part; Steel's generated
//!   default is `0`. [`MannequinEntity::new`] therefore writes the vanilla mask
//!   itself.

use std::sync::{Arc, Weak};

use glam::DVec3;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use simdnbt::{FromNbtTag, ToNbtTag};
use steel_macros::entity_behavior;
use steel_registry::entity_data::{EntityPose, HumanoidArm, ResolvableProfile};
use steel_registry::entity_type::EntityTypeRef;
use steel_registry::vanilla_entity_data::MannequinEntityData;
use steel_utils::locks::SyncMutex;
use steel_utils::{DowncastType, DowncastTypeKey};
use text_components::TextComponent;

use crate::entity::{
    Entity, EntityBase, EntityBaseLoad, EntitySyncedData, LivingEntity, LivingEntityBase,
    SharedEntity,
};
use crate::world::World;

/// Vanilla `Mannequin.ALL_LAYERS`: every `PlayerModelPart` mask bit.
const ALL_LAYERS: i8 = 0b0111_1111;
/// Vanilla `Mannequin.DEFAULT_DESCRIPTION` translation key.
const DEFAULT_DESCRIPTION_KEY: &str = "entity.minecraft.mannequin.label";
/// Vanilla `Mannequin.POSE_CODEC` accepted poses.
const VALID_POSES: [EntityPose; 5] = [
    EntityPose::Standing,
    EntityPose::Sneaking,
    EntityPose::Swimming,
    EntityPose::FallFlying,
    EntityPose::Sleeping,
];

/// Vanilla mannequin entity.
#[entity_behavior(class = "Mannequin")]
pub struct MannequinEntity {
    base: EntityBase,
    entity_type: EntityTypeRef,
    living_base: LivingEntityBase,
    entity_data: SyncMutex<MannequinEntityData>,
    /// Vanilla `description` field (not the synced copy, which can be hidden).
    description: SyncMutex<TextComponent>,
    /// Vanilla `hideDescription` field.
    hide_description: SyncMutex<bool>,
}

// SAFETY: This key is owned by Steel and uniquely identifies `MannequinEntity`.
unsafe impl DowncastType for MannequinEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:entity/mannequin");
}

impl MannequinEntity {
    /// Creates a mannequin at runtime.
    #[must_use]
    pub fn new(entity_type: EntityTypeRef, id: i32, position: DVec3, world: Weak<World>) -> Self {
        Self::new_with_base(
            EntityBase::new(id, position, entity_type.dimensions, world),
            entity_type,
        )
    }

    /// Reconstructs a mannequin from persisted base entity state.
    #[must_use]
    pub fn from_saved(entity_type: EntityTypeRef, load: EntityBaseLoad) -> Self {
        Self::new_with_base(
            EntityBase::from_load(load, entity_type.dimensions),
            entity_type,
        )
    }

    fn new_with_base(base: EntityBase, entity_type: EntityTypeRef) -> Self {
        let living_base = LivingEntityBase::new(entity_type);
        let mut entity_data = MannequinEntityData::new();
        living_base.initialize_synced_data(&mut entity_data);
        entity_data
            .avatar_mut()
            .player_mode_customization
            .set(ALL_LAYERS);
        let description = default_description();
        entity_data
            .description
            .set(Some(Box::new(description.clone())));
        Self {
            base,
            entity_type,
            living_base,
            entity_data: SyncMutex::new(entity_data),
            description: SyncMutex::new(description),
            hide_description: SyncMutex::new(false),
        }
    }

    /// Returns the mannequin's resolvable profile.
    #[must_use]
    pub fn profile(&self) -> ResolvableProfile {
        self.entity_data.lock().profile.get().clone()
    }

    /// Sets the mannequin's resolvable profile.
    pub fn set_profile(&self, profile: ResolvableProfile) {
        self.entity_data.lock().profile.set(profile);
    }

    /// Returns the vanilla `immovable` flag.
    #[must_use]
    pub fn is_immovable(&self) -> bool {
        *self.entity_data.lock().immovable.get()
    }

    /// Sets the vanilla `immovable` flag.
    pub fn set_immovable(&self, immovable: bool) {
        self.entity_data.lock().immovable.set(immovable);
    }

    /// Returns the visible model-part mask (`PlayerModelPart` bits).
    #[must_use]
    pub fn model_parts(&self) -> i8 {
        *self
            .entity_data
            .lock()
            .avatar()
            .player_mode_customization
            .get()
    }

    /// Sets the visible model-part mask.
    pub fn set_model_parts(&self, mask: i8) {
        self.entity_data
            .lock()
            .avatar_mut()
            .player_mode_customization
            .set(mask);
    }

    /// Returns the mannequin's main hand.
    #[must_use]
    pub fn main_arm(&self) -> HumanoidArm {
        *self.entity_data.lock().avatar().player_main_hand.get()
    }

    /// Sets the mannequin's main hand.
    pub fn set_main_arm(&self, arm: HumanoidArm) {
        self.entity_data
            .lock()
            .avatar_mut()
            .player_main_hand
            .set(arm);
    }

    /// Returns the label shown in the mannequin's name plate.
    #[must_use]
    pub fn description(&self) -> TextComponent {
        self.description.lock().clone()
    }

    /// Replaces the description and refreshes the synced copy.
    pub fn set_description(&self, description: TextComponent) {
        *self.description.lock() = description;
        self.update_description();
    }

    /// Toggles vanilla `hide_description`.
    pub fn set_hide_description(&self, hide_description: bool) {
        *self.hide_description.lock() = hide_description;
        self.update_description();
    }

    /// Returns whether the description is hidden.
    #[must_use]
    pub fn hide_description(&self) -> bool {
        *self.hide_description.lock()
    }

    /// Vanilla `Mannequin.updateDescription`: mirrors the field into synced data.
    fn update_description(&self) {
        let synced = if self.hide_description() {
            None
        } else {
            Some(Box::new(self.description()))
        };
        self.entity_data.lock().description.set(synced);
    }

    /// Vanilla `Mannequin.setPose` gated by the `POSE_CODEC` whitelist.
    pub fn set_valid_pose(&self, pose: EntityPose) {
        if VALID_POSES.contains(&pose) {
            self.set_pose(pose);
        }
    }

    /// Vanilla `Mannequin.LAYERS_CODEC` write side: the model-part byte itself.
    fn hidden_layers_byte(&self) -> i8 {
        self.model_parts()
    }
}

/// Vanilla `Mannequin.DEFAULT_DESCRIPTION`.
fn default_description() -> TextComponent {
    TextComponent::translated(text_components::translation::TranslatedMessage::new(
        DEFAULT_DESCRIPTION_KEY,
        None,
    ))
}

impl Entity for MannequinEntity {
    fn base(&self) -> &EntityBase {
        &self.base
    }

    fn entity_type(&self) -> EntityTypeRef {
        self.entity_type
    }

    fn base_tick(&self) {
        self.base_tick_living_entity();
    }

    fn tick(self: Arc<Self>) {
        let entity: SharedEntity = Arc::<Self>::clone(&self);
        self.tick_living_entity(&entity);
    }

    fn synced_data(&self) -> Option<&dyn EntitySyncedData> {
        Some(&self.entity_data)
    }

    fn is_effective_ai(&self) -> bool {
        !self.is_immovable() && self.is_server_driven_movement()
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_equipment(nbt);
        nbt.insert("profile", self.profile().to_nbt_tag());
        nbt.insert("hidden_layers", self.hidden_layers_byte());
        nbt.insert(
            "main_hand",
            match self.main_arm() {
                HumanoidArm::Left => "left",
                HumanoidArm::Right => "right",
            },
        );
        nbt.insert("pose", pose_serialized_name(self.pose()));
        nbt.insert("immovable", i8::from(self.is_immovable()));
        if self.hide_description() {
            nbt.insert("hide_description", i8::from(true));
        } else if self.description() != default_description() {
            nbt.insert("description", self.description().to_codec_nbt());
        }
    }

    fn load_additional(&self, nbt: BorrowedNbtCompoundView<'_, '_>) {
        self.load_equipment(nbt);

        if let Some(profile) = nbt.get("profile").and_then(ResolvableProfile::from_nbt_tag) {
            self.set_profile(profile);
        }

        self.set_model_parts(nbt.byte("hidden_layers").unwrap_or(ALL_LAYERS));
        self.set_main_arm(match nbt.string("main_hand").map(|value| value.to_str()) {
            Some(value) if value.as_ref() == "left" => HumanoidArm::Left,
            _ => HumanoidArm::Right,
        });
        if let Some(pose) = nbt
            .string("pose")
            .and_then(|value| parse_pose(value.to_str().as_ref()))
        {
            self.set_valid_pose(pose);
        }
        self.set_immovable(nbt.byte("immovable").is_some_and(|value| value != 0));
        self.set_hide_description(nbt.byte("hide_description").is_some_and(|value| value != 0));

        let description = nbt
            .get("description")
            .and_then(|tag| TextComponent::from_nbt(&tag.to_owned()))
            .unwrap_or_else(default_description);
        self.set_description(description);
    }
}

impl LivingEntity for MannequinEntity {
    fn living_base(&self) -> &LivingEntityBase {
        &self.living_base
    }

    fn is_immobile(&self) -> bool {
        self.is_immovable() || self.default_is_immobile()
    }

    fn get_health(&self) -> f32 {
        *self.entity_data.lock().living_entity().health.get()
    }

    fn set_health(&self, health: f32) {
        let max_health = self.get_max_health();
        let clamped = health.clamp(0.0, max_health);
        self.entity_data
            .lock()
            .living_entity_mut()
            .health
            .set(clamped);
    }
}

/// Vanilla `Pose.getSerializedName` for the poses a mannequin may store.
const fn pose_serialized_name(pose: EntityPose) -> &'static str {
    match pose {
        EntityPose::Sneaking => "crouching",
        EntityPose::Swimming => "swimming",
        EntityPose::FallFlying => "fall_flying",
        EntityPose::Sleeping => "sleeping",
        _ => "standing",
    }
}

/// Parses the vanilla serialized pose names a mannequin accepts.
///
/// Steel's [`EntityPose::from_str`] uses uppercase names, so the vanilla
/// lowercase codec is decoded here instead of through that `FromStr` impl.
fn parse_pose(value: &str) -> Option<EntityPose> {
    match value {
        "standing" => Some(EntityPose::Standing),
        "crouching" | "sneaking" => Some(EntityPose::Sneaking),
        "swimming" => Some(EntityPose::Swimming),
        "fall_flying" => Some(EntityPose::FallFlying),
        "sleeping" => Some(EntityPose::Sleeping),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-borrows owned NBT the way entity loading does from disk.
    fn with_reborrow<R>(
        nbt: &simdnbt::owned::NbtCompound,
        visitor: impl FnOnce(simdnbt::borrow::NbtCompound<'_, '_>) -> R,
    ) -> R {
        use simdnbt::borrow::read_compound;
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let mut cursor = std::io::Cursor::new(bytes.as_slice());
        let base = read_compound(&mut cursor).expect("entity NBT should reborrow");
        let borrowed: simdnbt::borrow::NbtCompound<'_, '_> = (&base).into();
        visitor(borrowed)
    }
    use steel_registry::vanilla_entities;

    fn new_mannequin() -> MannequinEntity {
        steel_registry::init_vanilla_registry();
        MannequinEntity::new(&vanilla_entities::MANNEQUIN, 1, DVec3::ZERO, Weak::new())
    }

    #[test]
    fn defaults_match_vanilla_mannequin() {
        let mannequin = new_mannequin();
        assert_eq!(mannequin.model_parts(), ALL_LAYERS);
        assert_eq!(mannequin.main_arm(), HumanoidArm::Right);
        assert!(!mannequin.is_immovable());
        assert!(!mannequin.hide_description());
        assert_eq!(mannequin.description(), default_description());
    }

    #[test]
    fn profile_and_flags_round_trip_through_nbt() {
        let mannequin = new_mannequin();
        mannequin.set_model_parts(ALL_LAYERS & !0b0100_0000);
        mannequin.set_main_arm(HumanoidArm::Left);
        mannequin.set_immovable(true);
        mannequin.set_valid_pose(EntityPose::Sneaking);

        let mut nbt = NbtCompound::new();
        mannequin.save_additional(&mut nbt);
        assert_eq!(nbt.byte("hidden_layers"), Some(ALL_LAYERS & !0b0100_0000));
        assert_eq!(
            nbt.string("main_hand").map(|value| value.to_string()),
            Some("left".to_string())
        );
        assert_eq!(
            nbt.string("pose").map(|value| value.to_string()),
            Some("crouching".to_string())
        );
        assert_eq!(nbt.byte("immovable"), Some(1));

        let reloaded = new_mannequin();
        with_reborrow(&nbt, |borrowed| reloaded.load_additional(borrowed));
        assert_eq!(reloaded.model_parts(), ALL_LAYERS & !0b0100_0000);
        assert_eq!(reloaded.main_arm(), HumanoidArm::Left);
        assert_eq!(reloaded.pose(), EntityPose::Sneaking);
        assert!(reloaded.is_immovable());
        assert_eq!(reloaded.profile(), mannequin.profile());
    }

    #[test]
    fn invalid_poses_are_rejected_like_vanilla() {
        let mannequin = new_mannequin();
        mannequin.set_valid_pose(EntityPose::Dying);
        assert_eq!(mannequin.pose(), EntityPose::Standing);
        mannequin.set_valid_pose(EntityPose::Sleeping);
        assert_eq!(mannequin.pose(), EntityPose::Sleeping);
    }

    #[test]
    fn hiding_the_description_clears_the_synced_copy_only() {
        let mannequin = new_mannequin();
        mannequin.set_hide_description(true);
        assert!(mannequin.entity_data.lock().description.get().is_none());
        assert_eq!(mannequin.description(), default_description());
        mannequin.set_hide_description(false);
        assert!(mannequin.entity_data.lock().description.get().is_some());
    }
}
