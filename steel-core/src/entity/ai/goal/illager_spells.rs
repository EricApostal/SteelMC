//! Vanilla `SpellcasterIllager` goals (`SpellcasterCastingSpellGoal` and
//! `SpellcasterUseSpellGoal`) shared by every spellcasting illager, plus the two Illusioner
//! spells.
//!
//! Vanilla keeps the spell state on the abstract `SpellcasterIllager` class and subclasses
//! `SpellcasterUseSpellGoal` per spell. Steel has no inheritance, so [`Spellcaster`] is the
//! capability holding the caster state (casting tick count, current spell, casting sound),
//! [`SpellcasterSpell`] carries the per-spell hooks, and [`SpellcasterUseSpellGoal`] is the
//! shared state machine. The evoker spells live in `evoker.rs` next to the entity they cast
//! for; both casters are bridged by [`caster_of`].

use steel_registry::sound_event::SoundEventRef;
use steel_registry::vanilla_mob_effects;
use steel_utils::Downcast as _;
use steel_utils::types::Difficulty;

use super::reduced_tick_delay;
use super::selector::{Goal, GoalControls};
use crate::entity::entities::mobs::illager::{EvokerEntity, IllagerSpell, IllusionerEntity};
use crate::entity::{Entity, LivingEntity, Mob, MobEffectInstance, PathfinderMob, SharedEntity};

/// Vanilla `SpellcasterIllager.SpellcasterUseSpellGoal.getCastWarmupTime`.
const DEFAULT_CAST_WARMUP_TIME: i32 = 20;
/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal.getCastingTime`.
const BLINDNESS_CASTING_TIME: i32 = 20;
/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal.getCastingInterval`.
const BLINDNESS_CASTING_INTERVAL: i32 = 180;
/// Vanilla `MobEffectInstance(MobEffects.BLINDNESS, 400)`.
const BLINDNESS_DURATION: i32 = 400;
/// Vanilla `Illusioner.IllusionerMirrorSpellGoal.getCastingTime`.
const MIRROR_CASTING_TIME: i32 = 20;
/// Vanilla `Illusioner.IllusionerMirrorSpellGoal.getCastingInterval`.
const MIRROR_CASTING_INTERVAL: i32 = 340;
/// Vanilla `MobEffectInstance(MobEffects.INVISIBILITY, 1200)`.
const MIRROR_INVISIBILITY_DURATION: i32 = 1200;

/// The `SpellcasterIllager` state every spellcasting illager owns.
///
/// Vanilla declares this on the abstract class; Steel keeps the fields on each entity and
/// exposes them through this capability so the shared goals never downcast for state.
pub(crate) trait Spellcaster {
    /// Vanilla `SpellcasterIllager.isCastingSpell` (server side).
    fn is_casting_spell(&self) -> bool;

    /// Vanilla `SpellcasterIllager.spellCastingTickCount`.
    fn spell_casting_tick_count(&self) -> i32;

    /// Vanilla `SpellcasterIllager.spellCastingTickCount = ticks`.
    fn set_spell_casting_tick_count(&self, ticks: i32);

    /// Vanilla `SpellcasterIllager.setIsCastingSpell`.
    fn set_is_casting_spell(&self, spell: IllagerSpell);

    /// Vanilla `SpellcasterIllager.getCastingSoundEvent`.
    fn casting_sound(&self) -> SoundEventRef;
}

/// Bridges a `dyn PathfinderMob` back to the [`Spellcaster`] it implements.
///
/// Vanilla reaches the caster through the inner-class `SpellcasterIllager.this`; Steel's
/// goals only receive the mob, so this is the single downcast point shared by every spell
/// goal. New spellcasters are added here.
fn caster_of(mob: &dyn PathfinderMob) -> Option<&dyn Spellcaster> {
    if let Some(evoker) = mob.downcast_ref::<EvokerEntity>() {
        return Some(evoker);
    }
    if let Some(illusioner) = mob.downcast_ref::<IllusionerEntity>() {
        return Some(illusioner);
    }
    None
}

impl Spellcaster for EvokerEntity {
    fn is_casting_spell(&self) -> bool {
        EvokerEntity::is_casting_spell(self)
    }

    fn spell_casting_tick_count(&self) -> i32 {
        EvokerEntity::spell_casting_tick_count(self)
    }

    fn set_spell_casting_tick_count(&self, ticks: i32) {
        EvokerEntity::set_spell_casting_tick_count(self, ticks);
    }

    fn set_is_casting_spell(&self, spell: IllagerSpell) {
        EvokerEntity::set_is_casting_spell(self, spell);
    }

    fn casting_sound(&self) -> SoundEventRef {
        EvokerEntity::casting_sound(self)
    }
}

impl Spellcaster for IllusionerEntity {
    fn is_casting_spell(&self) -> bool {
        IllusionerEntity::is_casting_spell(self)
    }

    fn spell_casting_tick_count(&self) -> i32 {
        IllusionerEntity::spell_casting_tick_count(self)
    }

    fn set_spell_casting_tick_count(&self, ticks: i32) {
        IllusionerEntity::set_spell_casting_tick_count(self, ticks);
    }

    fn set_is_casting_spell(&self, spell: IllagerSpell) {
        IllusionerEntity::set_is_casting_spell(self, spell);
    }

    fn casting_sound(&self) -> SoundEventRef {
        IllusionerEntity::casting_sound(self)
    }
}

/// One `SpellcasterIllager.SpellcasterUseSpellGoal` subclass.
///
/// `mob` is always the caster; a concrete spell downcasts it to the entity it belongs to,
/// mirroring vanilla's inner-class access to `SpellcasterIllager.this`.
pub(crate) trait SpellcasterSpell: Send {
    /// Vanilla `SpellcasterUseSpellGoal.getSpell`.
    fn spell(&self) -> IllagerSpell;

    /// Vanilla `SpellcasterUseSpellGoal.getCastingTime`.
    fn casting_time(&self) -> i32;

    /// Vanilla `SpellcasterUseSpellGoal.getCastingInterval`.
    fn casting_interval(&self) -> i32;

    /// Vanilla `SpellcasterUseSpellGoal.getSpellPrepareSound`.
    fn spell_prepare_sound(&self) -> Option<SoundEventRef>;

    /// Vanilla `SpellcasterUseSpellGoal.performSpellCasting`.
    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob);

    /// Vanilla `SpellcasterUseSpellGoal.getCastWarmupTime`.
    fn cast_warmup_time(&self) -> i32 {
        DEFAULT_CAST_WARMUP_TIME
    }

    /// Whether the base `canUse` requires a live combat target.
    ///
    /// Vanilla `Evoker.EvokerWololoSpellGoal` replaces `canUse` outright and deliberately
    /// casts at a sheep while the evoker has *no* target, so the shared state machine has to
    /// skip the target precondition for it.
    fn requires_target(&self) -> bool {
        true
    }

    /// `canContinueToUse` for a spell that [`Self::requires_target`] does not apply to.
    fn can_continue_without_target(&self, _mob: &dyn PathfinderMob) -> bool {
        true
    }

    /// Extra `canUse` predicate of the concrete subclass.
    fn can_use_extra(&mut self, _mob: &dyn PathfinderMob) -> bool {
        true
    }

    /// Extra `start` work of the concrete subclass.
    fn on_start(&mut self, _mob: &dyn PathfinderMob) {}

    /// Vanilla subclass `stop` work (the wololo spell clears its sheep target).
    fn on_stop(&mut self, _mob: &dyn PathfinderMob) {}
}

/// Vanilla `SpellcasterIllager.SpellcasterUseSpellGoal`.
pub(crate) struct SpellcasterUseSpellGoal<S: SpellcasterSpell> {
    spell: S,
    attack_warmup_delay: i32,
    next_attack_tick_count: i32,
}

impl<S: SpellcasterSpell> SpellcasterUseSpellGoal<S> {
    pub(crate) const fn new(spell: S) -> Self {
        Self {
            spell,
            attack_warmup_delay: 0,
            next_attack_tick_count: 0,
        }
    }
}

impl<S: SpellcasterSpell + 'static> Goal for SpellcasterUseSpellGoal<S> {
    fn controls(&self) -> GoalControls {
        GoalControls::EMPTY
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(caster) = caster_of(mob) else {
            return false;
        };
        if self.spell.requires_target() {
            let Some(target) = mob.target() else {
                return false;
            };
            if !target
                .as_living_entity()
                .is_some_and(LivingEntity::is_alive)
            {
                return false;
            }
        }
        if caster.is_casting_spell() || mob.tick_count() < self.next_attack_tick_count {
            return false;
        }
        self.spell.can_use_extra(mob)
    }

    fn can_continue_to_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        if self.attack_warmup_delay <= 0 {
            return false;
        }
        if !self.spell.requires_target() {
            return self.spell.can_continue_without_target(mob);
        }
        mob.target()
            .and_then(|target| target.as_living_entity().map(LivingEntity::is_alive))
            .is_some_and(|alive| alive)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        let Some(caster) = caster_of(mob) else {
            return;
        };
        self.attack_warmup_delay = reduced_tick_delay(self.spell.cast_warmup_time());
        caster.set_spell_casting_tick_count(self.spell.casting_time());
        self.next_attack_tick_count = mob.tick_count() + self.spell.casting_interval();
        if let Some(sound) = self.spell.spell_prepare_sound() {
            mob.play_sound(sound, 1.0, 1.0);
        }
        caster.set_is_casting_spell(self.spell.spell());
        self.spell.on_start(mob);
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        self.spell.on_stop(mob);
    }

    fn requires_update_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        self.attack_warmup_delay -= 1;
        if self.attack_warmup_delay == 0 {
            self.spell.perform_spell_casting(mob);
            if let Some(caster) = caster_of(mob) {
                mob.play_sound(caster.casting_sound(), 1.0, 1.0);
            }
        }
    }
}

/// Vanilla `SpellcasterIllager.SpellcasterCastingSpellGoal`.
pub(crate) struct SpellcasterCastingSpellGoal;

impl Goal for SpellcasterCastingSpellGoal {
    fn controls(&self) -> GoalControls {
        GoalControls::MOVE | GoalControls::LOOK
    }

    fn can_use(&mut self, mob: &dyn PathfinderMob) -> bool {
        caster_of(mob).is_some_and(Spellcaster::is_casting_spell)
    }

    fn start(&mut self, mob: &dyn PathfinderMob) {
        mob.mob_base().navigation().lock().stop();
    }

    fn stop(&mut self, mob: &dyn PathfinderMob) {
        if let Some(caster) = caster_of(mob) {
            caster.set_is_casting_spell(IllagerSpell::None);
        }
    }

    fn tick(&mut self, mob: &dyn PathfinderMob, _entity: &SharedEntity) {
        let Some(target) = mob.target() else {
            return;
        };
        let eye = glam::DVec3::new(target.position().x, target.get_eye_y(), target.position().z);
        mob.mob_base().controls().lock().look_control.set_look_at(
            eye,
            mob.max_head_y_rot(),
            mob.max_head_x_rot(),
        );
    }
}

/// Vanilla `Illusioner.IllusionerMirrorSpellGoal`.
pub(crate) struct IllusionerMirrorSpell;

impl SpellcasterSpell for IllusionerMirrorSpell {
    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Disappear
    }

    fn casting_time(&self) -> i32 {
        MIRROR_CASTING_TIME
    }

    fn casting_interval(&self) -> i32 {
        MIRROR_CASTING_INTERVAL
    }

    fn spell_prepare_sound(&self) -> Option<SoundEventRef> {
        Some(&steel_registry::sound_events::ENTITY_ILLUSIONER_PREPARE_MIRROR)
    }

    fn can_use_extra(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(caster) = mob.downcast_ref::<IllusionerEntity>() else {
            return false;
        };
        !caster.has_mob_effect(&vanilla_mob_effects::INVISIBILITY)
    }

    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob) {
        let Some(caster) = mob.downcast_ref::<IllusionerEntity>() else {
            return;
        };
        caster.add_mob_effect(MobEffectInstance::with_duration(
            &vanilla_mob_effects::INVISIBILITY,
            MIRROR_INVISIBILITY_DURATION,
            0,
        ));
        // Vanilla rebuilds the four mirror offsets client-side in `Illusioner.aiStep`; Steel
        // keeps them server-side so a future sync packet can drive the client duplicates.
        caster.reroll_illusion_offsets();
    }
}

/// Vanilla `Illusioner.IllusionerBlindnessSpellGoal`.
#[derive(Default)]
pub(crate) struct IllusionerBlindnessSpell {
    last_target_id: i32,
}

impl SpellcasterSpell for IllusionerBlindnessSpell {
    fn spell(&self) -> IllagerSpell {
        IllagerSpell::Blindness
    }

    fn casting_time(&self) -> i32 {
        BLINDNESS_CASTING_TIME
    }

    fn casting_interval(&self) -> i32 {
        BLINDNESS_CASTING_INTERVAL
    }

    fn spell_prepare_sound(&self) -> Option<SoundEventRef> {
        Some(&steel_registry::sound_events::ENTITY_ILLUSIONER_PREPARE_BLINDNESS)
    }

    fn can_use_extra(&mut self, mob: &dyn PathfinderMob) -> bool {
        let Some(target) = mob.target() else {
            return false;
        };
        if target.id() == self.last_target_id {
            return false;
        }
        // Vanilla `getCurrentDifficultyAt(...).isHarderThan(Difficulty.NORMAL.ordinal())`.
        mob.level()
            .is_some_and(|world| world.difficulty() == Difficulty::Hard)
    }

    fn on_start(&mut self, mob: &dyn PathfinderMob) {
        if let Some(target) = mob.target() {
            self.last_target_id = target.id();
        }
    }

    fn perform_spell_casting(&mut self, mob: &dyn PathfinderMob) {
        let Some(target) = mob.target() else {
            return;
        };
        let Some(target) = target.as_living_entity() else {
            return;
        };
        target.add_mob_effect(MobEffectInstance::with_duration(
            &vanilla_mob_effects::BLINDNESS,
            BLINDNESS_DURATION,
            0,
        ));
    }
}
