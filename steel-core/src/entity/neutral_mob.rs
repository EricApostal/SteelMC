//! Persistent anger for neutral mobs (vanilla `net.minecraft.world.entity.NeutralMob`).
//!
//! Vanilla 26.3 has no `world/entity/ai/anger` package: `AngerLevel`/`AngerManagement` are
//! Warden-only (`world/entity/monster/warden/`), so mobs that hit back use only this trait.
//!
//! The per-entity state lives in [`PersistentAnger`]; a mob owns one instance and returns it from
//! [`NeutralMob::persistent_anger`]. Everything else mirrors the vanilla interface: the anger end
//! time, the persistent anger target reference, the universal-anger game-rule checks, and the
//! `anger_end_time`/`angry_at` save data.

use std::any::try_as_dyn;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::NbtCompound;
use steel_registry::vanilla_game_rules::{FORGIVE_DEAD_PLAYERS, UNIVERSAL_ANGER};
use steel_utils::locks::SyncMutex;
use steel_utils::types::GameType;

use crate::entity::{Entity, EntityReference, LivingEntity, Mob, SharedEntity};
use crate::player::Player;
use steel_utils::types::Difficulty;

/// Vanilla `NeutralMob.NO_ANGER_END_TIME`.
pub const NO_ANGER_END_TIME: i64 = -1;
/// Vanilla `NeutralMob.TAG_ANGER_END_TIME`.
pub const TAG_ANGER_END_TIME: &str = "anger_end_time";
/// Vanilla `NeutralMob.TAG_ANGRY_AT`.
pub const TAG_ANGRY_AT: &str = "angry_at";
/// Legacy vanilla key still read by `NeutralMob.readPersistentAngerSaveData`.
const TAG_LEGACY_ANGER_TIME: &str = "AngerTime";

/// Per-entity persistent anger state (vanilla's `NeutralMob` end time + target reference fields).
///
/// `Debug` is not derived: the mutex contents are lock-guarded at runtime.
pub struct PersistentAnger {
    end_time: SyncMutex<i64>,
    target: SyncMutex<Option<EntityReference>>,
}

impl PersistentAnger {
    /// Creates empty anger state: no timer and no target.
    #[must_use]
    pub fn new() -> Self {
        Self {
            end_time: SyncMutex::new(NO_ANGER_END_TIME),
            target: SyncMutex::new(None),
        }
    }

    /// Returns the world tick the anger ends at, or [`NO_ANGER_END_TIME`].
    #[must_use]
    pub fn end_time(&self) -> i64 {
        *self.end_time.lock()
    }

    /// Sets the world tick the anger ends at.
    pub fn set_end_time(&self, end_time: i64) {
        *self.end_time.lock() = end_time;
    }

    /// Returns the persistent anger target reference, if any.
    #[must_use]
    pub fn target(&self) -> Option<EntityReference> {
        self.target.lock().clone()
    }

    /// Replaces the persistent anger target reference.
    pub fn set_target(&self, target: Option<EntityReference>) {
        *self.target.lock() = target;
    }
}

/// Vanilla `NeutralMob`: a mob that remembers who angered it beyond the current target.
pub trait NeutralMob: Mob {
    /// Returns this mob's persistent anger state.
    fn persistent_anger(&self) -> &PersistentAnger;

    /// Vanilla `NeutralMob.startPersistentAngerTimer`: samples this mob's anger duration.
    fn start_persistent_anger_timer(&self);

    /// Vanilla `NeutralMob.getPersistentAngerEndTime`.
    fn get_persistent_anger_end_time(&self) -> i64 {
        self.persistent_anger().end_time()
    }

    /// Vanilla `NeutralMob.setPersistentAngerEndTime`.
    fn set_persistent_anger_end_time(&self, end_time: i64) {
        self.persistent_anger().set_end_time(end_time);
    }

    /// Vanilla `NeutralMob.getPersistentAngerTarget`.
    fn get_persistent_anger_target(&self) -> Option<EntityReference> {
        self.persistent_anger().target()
    }

    /// Vanilla `NeutralMob.setPersistentAngerTarget`.
    fn set_persistent_anger_target(&self, target: Option<EntityReference>) {
        self.persistent_anger().set_target(target);
    }

    /// Vanilla `NeutralMob.setTimeToRemainAngry`.
    fn set_time_to_remain_angry(&self, remaining_time: i64) {
        if let Some(world) = self.level() {
            self.set_persistent_anger_end_time(world.game_time() + remaining_time);
        }
    }

    /// Vanilla `NeutralMob.isAngry`.
    fn is_angry(&self) -> bool {
        let end_time = self.get_persistent_anger_end_time();
        if end_time <= 0 {
            return false;
        }

        self.level()
            .is_some_and(|world| end_time - world.game_time() > 0)
    }

    /// Vanilla `NeutralMob.isAngryAt`.
    fn is_angry_at(&self, entity: &dyn LivingEntity) -> bool {
        if !Mob::can_attack(self, entity) {
            return false;
        }

        if is_valid_player_target(entity) && self.is_angry_at_all_players() {
            return true;
        }

        self.get_persistent_anger_target()
            .is_some_and(|target| target.uuid() == entity.uuid())
    }

    /// Vanilla `NeutralMob.isAngryAtAllPlayers`.
    fn is_angry_at_all_players(&self) -> bool {
        self.level().is_some_and(|world| {
            world.get_game_rule(&UNIVERSAL_ANGER)
                && self.is_angry()
                && self.get_persistent_anger_target().is_none()
        })
    }

    /// Vanilla `NeutralMob.updatePersistentAnger`.
    ///
    /// `stay_angry_if_target_present` keeps the timer running while a target is present; mobs
    /// tick this every server tick (e.g. `IronGolem.aiStep`).
    fn update_persistent_anger(&self, stay_angry_if_target_present: bool) {
        let previous_target = self.mob_base().target(|_| true);
        let persistent_anger_target = self.get_persistent_anger_target();

        if let Some(previous_target) = &previous_target
            && previous_target
                .as_living_entity()
                .is_some_and(LivingEntity::is_dead_or_dying)
            && persistent_anger_target
                .as_ref()
                .is_some_and(|target| target.uuid() == previous_target.uuid())
            && previous_target.as_mob().is_some()
        {
            self.stop_being_angry();
            return;
        }

        let target = self.target();
        if let Some(target) = &target {
            let is_new_target = persistent_anger_target
                .as_ref()
                .is_none_or(|reference| reference.uuid() != target.uuid());
            if is_new_target {
                self.set_persistent_anger_target(Some(EntityReference::from_entity(target)));
            }

            if is_new_target || stay_angry_if_target_present {
                self.start_persistent_anger_timer();
            }
        }

        if persistent_anger_target.is_some()
            && !self.is_angry()
            && (target
                .as_ref()
                .and_then(|target| target.as_living_entity())
                .is_none_or(|target| !is_valid_player_target(target))
                || !stay_angry_if_target_present)
        {
            self.stop_being_angry();
        }

        if let Some(entity) = persistent_anger_target
            .as_ref()
            .and_then(|reference| self.resolve_persistent_anger_entity(reference))
            && let Some(player) = entity.as_player()
            && (player.game_mode() == GameType::Creative
                || player.is_spectator()
                || self
                    .level()
                    .is_some_and(|world| world.difficulty() == Difficulty::Peaceful))
        {
            self.stop_being_angry();
        }
    }

    /// Vanilla `NeutralMob.playerDied`: forgives a dead player when the game rule allows it.
    fn player_died(&self, player: &Player) {
        if !self
            .level()
            .is_some_and(|world| world.get_game_rule(&FORGIVE_DEAD_PLAYERS))
        {
            return;
        }

        if self
            .get_persistent_anger_target()
            .is_some_and(|target| target.uuid() == player.uuid())
        {
            self.stop_being_angry();
        }
    }

    /// Vanilla `NeutralMob.forgetCurrentTargetAndRefreshUniversalAnger`.
    fn forget_current_target_and_refresh_universal_anger(&self) {
        self.stop_being_angry();
        self.start_persistent_anger_timer();
    }

    /// Vanilla `NeutralMob.stopBeingAngry`.
    fn stop_being_angry(&self) {
        self.set_last_hurt_by_mob(None);
        self.set_persistent_anger_target(None);
        Mob::set_target(self, None);
        self.set_persistent_anger_end_time(NO_ANGER_END_TIME);
    }

    /// Vanilla `NeutralMob.addPersistentAngerSaveData`.
    fn add_persistent_anger_save_data(&self, nbt: &mut NbtCompound) {
        nbt.insert(TAG_ANGER_END_TIME, self.get_persistent_anger_end_time());
        if let Some(target) = self.get_persistent_anger_target() {
            target.store(nbt, TAG_ANGRY_AT);
        }
    }

    /// Vanilla `NeutralMob.readPersistentAngerSaveData`.
    fn read_persistent_anger_save_data(&self, nbt: &BorrowedNbtCompoundView<'_, '_>) {
        if let Some(end_time) = nbt.long(TAG_ANGER_END_TIME) {
            self.set_persistent_anger_end_time(end_time);
        } else if let Some(legacy_anger_time) = nbt.int(TAG_LEGACY_ANGER_TIME) {
            self.set_time_to_remain_angry(i64::from(legacy_anger_time));
        } else {
            self.set_persistent_anger_end_time(NO_ANGER_END_TIME);
        }

        let target = EntityReference::read(nbt, TAG_ANGRY_AT);
        self.set_persistent_anger_target(target.clone());

        // Vanilla only resolves the reference on the server; a mob without a level keeps the UUID.
        if let Some(target) = target
            && let Some(world) = self.level()
        {
            let resolved = target.get_entity(&world);
            Mob::set_target(self, resolved.as_ref());
        }
    }

    /// Resolves this mob's persistent anger reference to a live entity in the same world.
    fn resolve_persistent_anger_entity(&self, reference: &EntityReference) -> Option<SharedEntity> {
        let world = self.level()?;
        let entity = reference.get_entity(&world)?;
        Some(entity)
    }
}

/// Vanilla `NeutralMob.isValidPlayerTarget`.
fn is_valid_player_target(target: &dyn LivingEntity) -> bool {
    let Some(player) = target.as_player() else {
        return false;
    };

    player.game_mode() != GameType::Creative
        && !player.is_spectator()
        && player
            .level()
            .is_none_or(|world| world.difficulty() != Difficulty::Peaceful)
}

/// Returns `entity` as a [`NeutralMob`] when it has persistent anger behavior.
///
/// TODO(integration): replaced by `Entity::as_neutral_mob()` once `entity/mod.rs` exposes the
/// capability accessor alongside `as_player`/`is_monster`.
#[must_use]
pub fn as_neutral_mob(entity: &dyn Entity) -> Option<&dyn NeutralMob> {
    try_as_dyn::<dyn Entity, dyn NeutralMob>(entity)
}
