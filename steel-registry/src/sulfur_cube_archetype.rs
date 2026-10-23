use rustc_hash::FxHashMap;
use steel_utils::Identifier;

use crate::attribute::AttributeModifierOperation;

/// Vanilla `SulfurCubeArchetype`, loaded from `sulfur_cube_archetype/<key>.json`.
#[derive(Debug)]
pub struct SulfurCubeArchetype {
    pub key: Identifier,
    /// Resolved item ids of the archetype's `items` item tag.
    pub items: Vec<Identifier>,
    pub attribute_modifiers: Vec<SulfurAttributeEntry>,
    pub buoyant: bool,
    pub explosion: Option<SulfurExplosionData>,
    pub contact_damage: Option<SulfurContactDamage>,
    pub knockback_modifiers: SulfurKnockbackModifiers,
    pub sound_settings: SulfurSoundSettings,
}

/// Vanilla `SulfurCubeArchetype.AttributeEntry`.
///
/// Vanilla stores an `AttributeModifier`; the JSON flattens its `id`, `amount` and
/// `operation`, so this mirrors the flattened shape directly.
#[derive(Debug)]
pub struct SulfurAttributeEntry {
    pub attribute: Identifier,
    pub id: Identifier,
    pub amount: f64,
    pub operation: AttributeModifierOperation,
}

/// Vanilla `SulfurCubeArchetype.ExplosionData`.
#[derive(Debug)]
pub struct SulfurExplosionData {
    pub power: i32,
    pub causes_fire: bool,
    pub fuse: i32,
}

/// Vanilla `SulfurCubeArchetype.ContactDamage`.
#[derive(Debug)]
pub struct SulfurContactDamage {
    pub damage_type: Identifier,
    pub amount: f32,
    pub attribute_to_source: bool,
}

/// Vanilla `SulfurCubeArchetype.KnockbackModifiers`.
#[derive(Debug)]
pub struct SulfurKnockbackModifiers {
    pub horizontal_power: f32,
    pub vertical_power: f32,
}

/// Vanilla `SulfurCubeArchetype.SoundSettings`.
#[derive(Debug)]
pub struct SulfurSoundSettings {
    pub hit_sound: Identifier,
    pub push_sound: Identifier,
    pub push_sound_impulse_threshold: f32,
    pub push_sound_cooldown: f32,
}

pub type SulfurCubeArchetypeRef = &'static SulfurCubeArchetype;

pub struct SulfurCubeArchetypeRegistry {
    sulfur_cube_archetypes_by_id: Vec<SulfurCubeArchetypeRef>,
    sulfur_cube_archetypes_by_key: FxHashMap<Identifier, usize>,
    allows_registering: bool,
}

impl SulfurCubeArchetypeRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sulfur_cube_archetypes_by_id: Vec::new(),
            sulfur_cube_archetypes_by_key: FxHashMap::default(),
            allows_registering: true,
        }
    }
}

crate::impl_standard_methods!(
    SulfurCubeArchetypeRegistry,
    SulfurCubeArchetypeRef,
    sulfur_cube_archetypes_by_id,
    sulfur_cube_archetypes_by_key,
    allows_registering
);

crate::impl_registry!(
    SulfurCubeArchetypeRegistry,
    SulfurCubeArchetype,
    sulfur_cube_archetypes_by_id,
    sulfur_cube_archetypes_by_key,
    sulfur_cube_archetypes
);
