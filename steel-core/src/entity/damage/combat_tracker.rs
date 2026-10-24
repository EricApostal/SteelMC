//! Vanilla `CombatTracker`: recent damage entries, combat timing, and death-message composition.
//!
//! Mirrors `net.minecraft.world.damagesource.CombatTracker`. The tracker never holds a
//! reference back to its owner: every method that needs the victim takes it as an
//! argument, so the tracker lives in [`crate::entity::LivingEntityBase`] without an
//! ownership cycle.

use steel_registry::blocks::BlockRef;
use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::damage_type::DeathMessageType;
use steel_registry::vanilla_block_tags::BlockTag;
use steel_registry::vanilla_blocks;
use steel_registry::vanilla_damage_type_tags;
use steel_utils::translations;
use text_components::interactivity::{ClickEvent, HoverEvent};
use text_components::{Modifier as _, TextComponent};

use super::{DamageSource, death_message, has_custom_name, item_display_name, main_hand_item};
use crate::entity::{Entity, LivingEntity, SharedEntity};

/// Vanilla `CombatTracker.RESET_DAMAGE_STATUS_TIME`.
pub const RESET_DAMAGE_STATUS_TIME: i32 = 100;
/// Vanilla `CombatTracker.RESET_COMBAT_STATUS_TIME`.
pub const RESET_COMBAT_STATUS_TIME: i32 = 300;
/// Vanilla `CombatTracker.INTENTIONAL_GAME_DESIGN_STYLE` click target.
const INTENTIONAL_GAME_DESIGN_BUG_URL: &str = "https://bugs.mojang.com/browse/MCPE-28723";
/// Vanilla `CombatTracker.INTENTIONAL_GAME_DESIGN_STYLE` hover text.
const INTENTIONAL_GAME_DESIGN_BUG_ID: &str = "MCPE-28723";
/// Vanilla `getMostSignificantFall` threshold for reporting a fall entry.
const SIGNIFICANT_FALL_DISTANCE: f32 = 5.0;

/// Vanilla `FallLocation`: the block or fluid that broke a fatal fall.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallLocation {
    /// No specific block, or a fall not tied to one.
    Generic,
    /// Ladders and trapdoors.
    Ladder,
    /// Vines.
    Vines,
    /// Weeping vines.
    WeepingVines,
    /// Twisting vines.
    TwistingVines,
    /// Scaffolding.
    Scaffolding,
    /// Any other climbable block.
    OtherClimbable,
    /// Water.
    Water,
}

impl FallLocation {
    /// Vanilla `FallLocation.languageKey`.
    #[must_use]
    pub const fn language_key(self) -> &'static str {
        match self {
            Self::Generic => "death.fell.accident.generic",
            Self::Ladder => "death.fell.accident.ladder",
            Self::Vines => "death.fell.accident.vines",
            Self::WeepingVines => "death.fell.accident.weeping_vines",
            Self::TwistingVines => "death.fell.accident.twisting_vines",
            Self::Scaffolding => "death.fell.accident.scaffolding",
            Self::OtherClimbable => "death.fell.accident.other_climbable",
            Self::Water => "death.fell.accident.water",
        }
    }

    /// Vanilla `FallLocation.blockToFallLocation`.
    #[must_use]
    pub fn block_to_fall_location(block: BlockRef) -> Self {
        if block == &vanilla_blocks::LADDER || block.has_tag(&BlockTag::TRAPDOORS) {
            Self::Ladder
        } else if block == &vanilla_blocks::VINE {
            Self::Vines
        } else if block == &vanilla_blocks::WEEPING_VINES
            || block == &vanilla_blocks::WEEPING_VINES_PLANT
        {
            Self::WeepingVines
        } else if block == &vanilla_blocks::TWISTING_VINES
            || block == &vanilla_blocks::TWISTING_VINES_PLANT
        {
            Self::TwistingVines
        } else if block == &vanilla_blocks::SCAFFOLDING {
            Self::Scaffolding
        } else {
            Self::OtherClimbable
        }
    }

    /// Vanilla `FallLocation.getCurrentFallLocation`.
    #[must_use]
    pub fn current_fall_location(mob: &dyn LivingEntity) -> Option<Self> {
        if let Some(pos) = mob.living_base().last_climbable_pos() {
            let world = mob.level()?;
            let state = world.get_block_state(pos);
            return Some(Self::block_to_fall_location(state.get_block()));
        }
        mob.is_in_water().then_some(Self::Water)
    }
}

/// One vanilla `CombatEntry`: a single recorded hit on the victim.
#[derive(Debug, Clone)]
pub struct CombatEntry {
    source: DamageSource,
    damage: f32,
    fall_location: Option<FallLocation>,
    fall_distance: f32,
}

impl CombatEntry {
    /// The damage source that produced this entry.
    #[must_use]
    pub const fn source(&self) -> &DamageSource {
        &self.source
    }

    /// The post-reduction damage that was applied.
    #[must_use]
    pub const fn damage(&self) -> f32 {
        self.damage
    }

    /// The fall location observed when the entry was recorded.
    #[must_use]
    pub const fn fall_location(&self) -> Option<FallLocation> {
        self.fall_location
    }

    /// The victim's fall distance when the entry was recorded.
    #[must_use]
    pub const fn fall_distance(&self) -> f32 {
        self.fall_distance
    }
}

/// Combat-state transitions produced by [`CombatTracker::record_damage`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CombatStateChange {
    /// The victim entered combat, so vanilla `onEnterCombat` must run.
    pub entered_combat: bool,
    /// The victim left combat, so vanilla `onLeaveCombat` must run.
    pub left_combat: bool,
}

/// Vanilla `CombatTracker`: the victim's recent damage and combat timing.
#[derive(Debug, Default)]
pub struct CombatTracker {
    entries: Vec<CombatEntry>,
    last_damage_time: i32,
    combat_start_time: i32,
    combat_end_time: i32,
    in_combat: bool,
    taking_damage: bool,
}

impl CombatTracker {
    /// Creates an empty tracker.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            last_damage_time: 0,
            combat_start_time: 0,
            combat_end_time: 0,
            in_combat: false,
            taking_damage: false,
        }
    }

    /// Vanilla `CombatTracker.recordDamage`.
    ///
    /// Reports the resulting combat transitions so the owner can run vanilla
    /// `onEnterCombat` / `onLeaveCombat`.
    pub fn record_damage(
        &mut self,
        mob: &dyn LivingEntity,
        source: &DamageSource,
        damage: f32,
    ) -> CombatStateChange {
        let left_combat = self.recheck_status(mob);
        let fall_location = FallLocation::current_fall_location(mob);
        self.entries.push(CombatEntry {
            source: source.clone(),
            damage,
            fall_location,
            fall_distance: mob.fall_distance() as f32,
        });
        self.last_damage_time = mob.tick_count();
        self.taking_damage = true;
        let mut entered_combat = false;
        if !self.in_combat && Entity::is_alive(mob) && should_enter_combat(source) {
            self.in_combat = true;
            self.combat_start_time = mob.tick_count();
            self.combat_end_time = self.combat_start_time;
            entered_combat = true;
        }
        CombatStateChange {
            entered_combat,
            left_combat,
        }
    }

    /// Vanilla `CombatTracker.getDeathMessage`.
    #[must_use]
    pub fn get_death_message(&self, mob: &dyn LivingEntity) -> TextComponent {
        let Some(killing_blow) = self.entries.last() else {
            return death_message("death.attack.generic", vec![mob.display_name()]);
        };
        let killing_source = killing_blow.source();
        let knock_off_entry = self.get_most_significant_fall();
        match killing_source.damage_type.death_message_type {
            DeathMessageType::FallVariants => match knock_off_entry {
                Some(knock_off_entry) => {
                    Self::get_fall_message(knock_off_entry, killing_source.causing_entity(), mob)
                }
                None => killing_source.get_localized_death_message(mob),
            },
            DeathMessageType::IntentionalGameDesign => {
                let key = format!("death.attack.{}", killing_source.damage_type.message_id);
                let link = translations::CHAT_SQUARE_BRACKETS
                    .message([death_message(format!("{key}.link"), Vec::new())])
                    .component()
                    .click_event(ClickEvent::open_url(INTENTIONAL_GAME_DESIGN_BUG_URL))
                    .hover_event(HoverEvent::show_text(TextComponent::plain(
                        INTENTIONAL_GAME_DESIGN_BUG_ID,
                    )));
                death_message(format!("{key}.message"), vec![mob.display_name(), link])
            }
            DeathMessageType::Default => killing_source.get_localized_death_message(mob),
        }
    }

    /// Vanilla `CombatTracker.getFallMessage`.
    fn get_fall_message(
        knock_off_entry: &CombatEntry,
        killing_entity: Option<&SharedEntity>,
        mob: &dyn LivingEntity,
    ) -> TextComponent {
        let knock_off_source = knock_off_entry.source();
        if !knock_off_source.is(&vanilla_damage_type_tags::DamageTypeTag::IS_FALL)
            && !knock_off_source
                .is(&vanilla_damage_type_tags::DamageTypeTag::ALWAYS_MOST_SIGNIFICANT_FALL)
        {
            let killer_name = killing_entity.map(|entity| entity.display_name());
            let attacker_entity = knock_off_source.causing_entity();
            let attacker_name = attacker_entity.map(|entity| entity.display_name());
            if let Some(attacker_name) = attacker_name
                && killer_name.as_ref() != Some(&attacker_name)
            {
                return Self::get_message_for_assisted_fall(
                    attacker_entity,
                    &attacker_name,
                    "death.fell.assist.item",
                    "death.fell.assist",
                    mob,
                );
            }
            match killer_name {
                Some(killer_name) => Self::get_message_for_assisted_fall(
                    killing_entity,
                    &killer_name,
                    "death.fell.finish.item",
                    "death.fell.finish",
                    mob,
                ),
                None => death_message("death.fell.killer", vec![mob.display_name()]),
            }
        } else {
            let fall_location = knock_off_entry
                .fall_location()
                .unwrap_or(FallLocation::Generic);
            death_message(fall_location.language_key(), vec![mob.display_name()])
        }
    }

    /// Vanilla `CombatTracker.getMessageForAssistedFall`.
    fn get_message_for_assisted_fall(
        attacker_entity: Option<&SharedEntity>,
        attacker_name: &TextComponent,
        message_with_item: &'static str,
        message_without_item: &'static str,
        mob: &dyn LivingEntity,
    ) -> TextComponent {
        let attacker_item = main_hand_item(attacker_entity);
        if !attacker_item.is_empty() && has_custom_name(&attacker_item) {
            death_message(
                message_with_item,
                vec![
                    mob.display_name(),
                    attacker_name.clone(),
                    item_display_name(&attacker_item),
                ],
            )
        } else {
            death_message(
                message_without_item,
                vec![mob.display_name(), attacker_name.clone()],
            )
        }
    }

    /// Vanilla `CombatTracker.getMostSignificantFall`.
    #[must_use]
    pub fn get_most_significant_fall(&self) -> Option<&CombatEntry> {
        let mut result: Option<&CombatEntry> = None;
        let mut alternative: Option<&CombatEntry> = None;
        let mut alternative_damage = 0.0_f32;
        let mut best_fall = 0.0_f32;

        for (index, entry) in self.entries.iter().enumerate() {
            let source = entry.source();
            let is_fake_fall =
                source.is(&vanilla_damage_type_tags::DamageTypeTag::ALWAYS_MOST_SIGNIFICANT_FALL);
            let fall_distance = if is_fake_fall {
                f32::MAX
            } else {
                entry.fall_distance()
            };
            if (source.is(&vanilla_damage_type_tags::DamageTypeTag::IS_FALL) || is_fake_fall)
                && fall_distance > 0.0
                && (result.is_none() || fall_distance > best_fall)
            {
                result = Some(if index > 0 {
                    &self.entries[index - 1]
                } else {
                    entry
                });
                best_fall = fall_distance;
            }

            if entry.fall_location().is_some()
                && (alternative.is_none() || entry.damage() > alternative_damage)
            {
                alternative = Some(entry);
                alternative_damage = entry.damage();
            }
        }

        if best_fall > SIGNIFICANT_FALL_DISTANCE {
            result
        } else if alternative_damage > SIGNIFICANT_FALL_DISTANCE {
            alternative
        } else {
            None
        }
    }

    /// Vanilla `CombatTracker.getCombatDuration`.
    #[must_use]
    pub const fn get_combat_duration(&self, tick_count: i32) -> i32 {
        if self.in_combat {
            tick_count.wrapping_sub(self.combat_start_time)
        } else {
            self.combat_end_time.wrapping_sub(self.combat_start_time)
        }
    }

    /// Vanilla `CombatTracker.recheckStatus`.
    ///
    /// Returns whether the victim left combat, so the caller can run vanilla
    /// `onLeaveCombat`.
    pub fn recheck_status(&mut self, mob: &dyn LivingEntity) -> bool {
        let reset = if self.in_combat {
            RESET_COMBAT_STATUS_TIME
        } else {
            RESET_DAMAGE_STATUS_TIME
        };
        if self.taking_damage
            && (!Entity::is_alive(mob)
                || mob.tick_count().wrapping_sub(self.last_damage_time) > reset)
        {
            let was_in_combat = self.in_combat;
            self.taking_damage = false;
            self.in_combat = false;
            self.combat_end_time = mob.tick_count();
            self.entries.clear();
            return was_in_combat;
        }
        false
    }

    /// The recorded damage entries, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[CombatEntry] {
        &self.entries
    }

    /// Whether the victim is currently in combat.
    #[must_use]
    pub const fn is_in_combat(&self) -> bool {
        self.in_combat
    }

    /// Whether the victim has taken damage recently.
    #[must_use]
    pub const fn is_taking_damage(&self) -> bool {
        self.taking_damage
    }
}

/// Vanilla `CombatTracker.shouldEnterCombat`: only living attackers start combat.
fn should_enter_combat(source: &DamageSource) -> bool {
    source
        .causing_entity()
        .is_some_and(|entity| entity.as_living_entity().is_some())
}
