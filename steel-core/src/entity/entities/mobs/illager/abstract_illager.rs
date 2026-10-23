//! Shared illager data and target selectors (vanilla `AbstractIllager` /
//! `SpellcasterIllager`).
//!
//! Vanilla models these as abstract classes; Steel uses plain data plus helper predicates
//! that each concrete illager wires into its own goal list.

use steel_registry::vanilla_entities;
use steel_utils::DowncastTypeKey;

use crate::entity::LivingEntity;

/// Vanilla `AbstractIllager.IllagerArmPose`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IllagerArmPose {
    #[default]
    /// Arms crossed at rest.
    Crossed,
    /// Swinging a melee weapon.
    Attacking,
    /// Casting a spell.
    Spellcasting,
    /// Holding a bow and arrow.
    BowAndArrow,
    /// Holding a loaded crossbow.
    CrossbowHold,
    /// Charging a crossbow.
    CrossbowCharge,
    /// Celebrating after a raid victory.
    Celebrating,
    /// No special arm pose.
    Neutral,
}

/// Vanilla `SpellcasterIllager.IllagerSpell`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IllagerSpell {
    #[default]
    /// No spell is being cast.
    None,
    /// Summons vexes.
    SummonVex,
    /// Evokes a line of evoker fangs.
    Fangs,
    /// Converts blue sheep into red sheep.
    Wololo,
    /// Makes the spellcaster disappear.
    Disappear,
    /// Blinds nearby players.
    Blindness,
}

impl IllagerSpell {
    /// Vanilla `IllagerSpell` declaration order, which is also the synced id order.
    pub const VALUES: [Self; 6] = [
        Self::None,
        Self::SummonVex,
        Self::Fangs,
        Self::Wololo,
        Self::Disappear,
        Self::Blindness,
    ];

    /// Vanilla `IllagerSpell.id`.
    #[must_use]
    pub const fn id(self) -> i32 {
        match self {
            Self::None => 0,
            Self::SummonVex => 1,
            Self::Fangs => 2,
            Self::Wololo => 3,
            Self::Disappear => 4,
            Self::Blindness => 5,
        }
    }

    /// Vanilla `IllagerSpell.spellColor`.
    #[must_use]
    pub const fn spell_color(self) -> [f64; 3] {
        match self {
            Self::None => [0.0, 0.0, 0.0],
            Self::SummonVex => [0.7, 0.7, 0.8],
            Self::Fangs => [0.4, 0.3, 0.35],
            Self::Wololo => [0.7, 0.5, 0.2],
            Self::Disappear => [0.3, 0.3, 0.8],
            Self::Blindness => [0.1, 0.1, 0.2],
        }
    }

    /// Vanilla `IllagerSpell.byId`.
    #[must_use]
    pub fn by_id(id: i32) -> Self {
        Self::VALUES
            .into_iter()
            .find(|spell| spell.id() == id)
            .unwrap_or(Self::None)
    }
}

/// The `DowncastType` keys of every raider type.
///
/// Vanilla passes a single `Raider.class` to `HurtByTargetGoal`; Steel's target goals match
/// concrete keys, so the class check becomes this list.
pub struct RaiderKeys;

impl RaiderKeys {
    /// Every raid mob's entity key.
    pub const ALL: [DowncastTypeKey; 6] = [
        DowncastTypeKey::new("steel:entity/pillager"),
        DowncastTypeKey::new("steel:entity/vindicator"),
        DowncastTypeKey::new("steel:entity/evoker"),
        DowncastTypeKey::new("steel:entity/illusioner"),
        DowncastTypeKey::new("steel:entity/ravager"),
        DowncastTypeKey::new("steel:entity/witch"),
    ];
}

/// Vanilla `NearestAttackableTargetGoal<>(this, AbstractVillager.class, …)`.
#[must_use]
pub fn is_target_villager(target: &dyn LivingEntity, _world: &crate::world::World) -> bool {
    let entity_type = target.entity_type();
    entity_type == &vanilla_entities::VILLAGER || entity_type == &vanilla_entities::WANDERING_TRADER
}

/// Vanilla `AbstractIllager.canAttack`: never attack a baby villager.
#[must_use]
pub fn can_attack_villager(target: &dyn LivingEntity, world: &crate::world::World) -> bool {
    !(is_target_villager(target, world) && target.is_baby())
}

/// Vanilla `NearestAttackableTargetGoal<>(this, IronGolem.class, …)`.
#[must_use]
pub fn is_target_iron_golem(target: &dyn LivingEntity, _world: &crate::world::World) -> bool {
    target.entity_type() == &vanilla_entities::IRON_GOLEM
}

/// Vanilla `AbstractIllager.considersEntityAsAlly` illager-friend branch: Illagers treat
/// `#minecraft:illager_friends` members as allies.
#[must_use]
pub fn is_illager_friend(target: &dyn LivingEntity) -> bool {
    use steel_registry::REGISTRY;
    use steel_registry::TaggedRegistryExt;
    use steel_registry::vanilla_entity_type_tags::EntityTypeTag;

    REGISTRY
        .entity_types
        .is_in_tag(target.entity_type(), &EntityTypeTag::ILLAGER_FRIENDS)
}
