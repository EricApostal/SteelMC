use std::fmt;
use std::sync::Arc;

use glam::DVec3;
use steel_registry::{
    REGISTRY, TaggedRegistryExt, damage_type::DamageScaling, damage_type::DamageType,
    vanilla_damage_type_tags,
};
use text_components::TextComponent;

use crate::entity::{LivingEntity, SharedEntity, get_kill_credit};
use crate::player::Player;

use super::{death_message, has_custom_name, item_display_name, main_hand_item};

/// Describes damage while retaining the exact direct and causing entity allocations.
///
/// Removal and respawn never rebind attribution. Store recent sources in
/// [`super::DamageHistory`] to avoid self/mutual ownership cycles.
#[derive(Clone)]
pub struct DamageSource {
    /// The damage type registry entry.
    pub damage_type: &'static DamageType,
    causing_entity: Option<SharedEntity>,
    direct_entity: Option<SharedEntity>,
    source_position: Option<DVec3>,
}

impl DamageSource {
    /// Environmental damage with no entity or position context (void, starvation, etc.).
    #[must_use]
    pub const fn environment(damage_type: &'static DamageType) -> Self {
        Self {
            damage_type,
            causing_entity: None,
            direct_entity: None,
            source_position: None,
        }
    }

    /// Damage delivered directly by its causing entity.
    #[must_use]
    pub fn direct(damage_type: &'static DamageType, entity: SharedEntity) -> Self {
        Self {
            damage_type,
            causing_entity: Some(Arc::clone(&entity)),
            direct_entity: Some(entity),
            source_position: None,
        }
    }

    /// Adds the entity ultimately responsible for the damage.
    #[must_use]
    pub fn with_causing_entity(mut self, entity: SharedEntity) -> Self {
        self.causing_entity = Some(entity);
        self
    }

    /// Adds the direct entity that delivered the damage.
    #[must_use]
    pub fn with_direct_entity(mut self, entity: SharedEntity) -> Self {
        self.direct_entity = Some(entity);
        self
    }

    /// The original cause (e.g. a projectile's shooter), retained after removal.
    #[must_use]
    pub const fn causing_entity(&self) -> Option<&SharedEntity> {
        self.causing_entity.as_ref()
    }

    /// The original direct entity (e.g. the projectile), retained after removal.
    #[must_use]
    pub const fn direct_entity(&self) -> Option<&SharedEntity> {
        self.direct_entity.as_ref()
    }

    /// The explicit source position, or the direct entity's position when none was set.
    /// Distinct from the raw position sent in packets.
    #[must_use]
    pub fn source_position(&self) -> Option<DVec3> {
        self.source_position
            .or_else(|| self.direct_entity().map(|entity| entity.position()))
    }

    /// The explicit source position without the direct-entity fallback, used by the
    /// damage-event packet.
    #[must_use]
    pub const fn source_position_raw(&self) -> Option<DVec3> {
        self.source_position
    }

    /// Whether the causing player has vanilla infinite-materials abilities.
    #[must_use]
    pub fn is_creative_player(&self) -> bool {
        self.causing_entity()
            .and_then(|entity| entity.as_player())
            .is_some_and(Player::has_infinite_materials)
    }

    /// Adds the vanilla source position used by damage events and knockback.
    #[must_use]
    pub const fn with_source_position(mut self, source_position: DVec3) -> Self {
        self.source_position = Some(source_position);
        self
    }

    /// Whether this damage bypasses creative/spectator invulnerability.
    #[must_use]
    pub fn bypasses_invulnerability(&self) -> bool {
        self.is(&vanilla_damage_type_tags::DamageTypeTag::BYPASSES_INVULNERABILITY)
    }

    /// Returns whether this damage type is in the given vanilla damage-type tag.
    #[must_use]
    pub fn is(&self, tag: &steel_utils::Identifier) -> bool {
        REGISTRY.damage_types.is_in_tag(self.damage_type, tag)
    }

    /// Whether the damage was dealt directly by the causing entity rather than
    /// indirectly, e.g. by a thrown projectile.
    #[must_use]
    pub fn is_direct(&self) -> bool {
        match (self.causing_entity(), self.direct_entity()) {
            (None, None) => true,
            (Some(cause), Some(direct)) => Arc::ptr_eq(cause, direct),
            _ => false,
        }
    }

    /// Whether this damage bypasses the invulnerability cooldown timer.
    /// No vanilla damage types currently use this, but the logic exists in
    /// `LivingEntity.hurtServer()`.
    /// TODO: use damage type tag query once supported
    #[expect(clippy::unused_self, reason = "this is an api function")]
    #[must_use]
    pub const fn bypasses_cooldown(&self) -> bool {
        false
    }

    /// Vanilla `DamageSource.getLocalizedDeathMessage`.
    ///
    /// Composes `death.attack.<msgId>` with vanilla's argument order:
    /// victim only, victim plus attacker, or victim, attacker, and weapon when the
    /// attacker's main-hand item has a custom name.
    #[must_use]
    pub fn get_localized_death_message(&self, victim: &dyn LivingEntity) -> TextComponent {
        let death_key = format!("death.attack.{}", self.damage_type.message_id);
        if self.causing_entity.is_none() && self.direct_entity.is_none() {
            let kill_credit = victim
                .level()
                .and_then(|world| get_kill_credit(victim, world.as_ref()));
            return match kill_credit {
                Some(source) => death_message(
                    format!("{death_key}.player"),
                    vec![victim.display_name(), source.display_name()],
                ),
                None => death_message(death_key, vec![victim.display_name()]),
            };
        }

        let attacker = self.causing_entity.as_ref().or(self.direct_entity.as_ref());
        let name = attacker.map_or_else(TextComponent::new, |entity| entity.display_name());
        let held = main_hand_item(self.causing_entity.as_ref());
        if !held.is_empty() && has_custom_name(&held) {
            death_message(
                format!("{death_key}.item"),
                vec![victim.display_name(), name, item_display_name(&held)],
            )
        } else {
            death_message(death_key, vec![victim.display_name(), name])
        }
    }

    /// Whether the damage scales with difficulty for the retained causing entity.
    #[must_use]
    pub fn scales_with_difficulty(&self) -> bool {
        let causing_entity = self.causing_entity();
        match self.damage_type.scaling {
            DamageScaling::Never => false,
            DamageScaling::WhenCausedByLivingNonPlayer => causing_entity.is_some_and(|entity| {
                entity.as_living_entity().is_some() && entity.as_player().is_none()
            }),
            DamageScaling::Always => true,
        }
    }
}

impl fmt::Debug for DamageSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DamageSource")
            .field("damage_type", &self.damage_type)
            .field(
                "causing_entity",
                &self
                    .causing_entity
                    .as_ref()
                    .map(|entity| entity.generation()),
            )
            .field(
                "direct_entity",
                &self
                    .direct_entity
                    .as_ref()
                    .map(|entity| entity.generation()),
            )
            .field("source_position", &self.source_position)
            .finish()
    }
}
