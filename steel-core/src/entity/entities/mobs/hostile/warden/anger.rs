//! Vanilla `net.minecraft.world.entity.monster.warden.AngerManagement` and
//! `AngerLevel`.
//!
//! Vanilla keys its suspect table by entity identity and holds strong references
//! so suspects stay loaded. Steel cannot hash `Arc<dyn Entity>` and must not keep
//! entities alive, so the table is keyed by suspect UUID and holds
//! [`WeakEntity`] handles: an entry whose handle no longer upgrades is vanilla's
//! `UNLOADED_TO_CHUNK` suspect — its anger stays in the serialized half of the
//! table (`angerByUuid`) until the entity loads again.

use std::cmp::Ordering;
use std::mem;
use std::sync::Arc;

use rustc_hash::FxHashMap;
use simdnbt::borrow::NbtCompound as BorrowedNbtCompoundView;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::sound_event::SoundEventRef;
use steel_registry::sound_events;
use steel_utils::UuidExt;
use uuid::Uuid;

use crate::entity::{RemovalReason, SharedEntity, WeakEntity};
use crate::world::World;

/// Vanilla `AngerManagement.MAX_ANGER`.
const MAX_ANGER: i32 = 150;

/// Vanilla `AngerManagement.CONVERSION_DELAY`.
const CONVERSION_DELAY: i32 = 2;

/// Vanilla `AngerManagement.DEFAULT_ANGER_DECREASE`, applied once per
/// [`AngerManagement::tick`].
const DEFAULT_ANGER_DECREASE: i32 = 1;

/// Vanilla `AngerLevel`: the anger thresholds that drive ambient sounds and the
/// warden's `FIGHT` activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngerLevel {
    /// Vanilla `AngerLevel.CALM`.
    Calm,
    /// Vanilla `AngerLevel.AGITATED`.
    Agitated,
    /// Vanilla `AngerLevel.ANGRY`.
    Angry,
}

impl AngerLevel {
    /// Vanilla `AngerLevel.minimumAnger`.
    #[must_use]
    pub const fn minimum_anger(self) -> i32 {
        match self {
            Self::Calm => 0,
            Self::Agitated => 40,
            Self::Angry => 80,
        }
    }

    /// Vanilla `AngerLevel.byAnger`, including its sorted-threshold lookup.
    #[must_use]
    pub const fn by_anger(anger: i32) -> Self {
        if anger >= Self::Angry.minimum_anger() {
            Self::Angry
        } else if anger >= Self::Agitated.minimum_anger() {
            Self::Agitated
        } else {
            Self::Calm
        }
    }

    /// Vanilla `AngerLevel.isAngry`.
    #[must_use]
    pub const fn is_angry(self) -> bool {
        matches!(self, Self::Angry)
    }

    /// Vanilla `AngerLevel.getAmbientSound`.
    #[must_use]
    pub fn ambient_sound(self) -> SoundEventRef {
        match self {
            Self::Calm => &sound_events::ENTITY_WARDEN_AMBIENT,
            Self::Agitated => &sound_events::ENTITY_WARDEN_AGITATED,
            Self::Angry => &sound_events::ENTITY_WARDEN_ANGRY,
        }
    }

    /// Vanilla `AngerLevel.getListeningSound`.
    #[must_use]
    pub fn listening_sound(self) -> SoundEventRef {
        match self {
            Self::Calm => &sound_events::ENTITY_WARDEN_LISTENING,
            Self::Agitated | Self::Angry => &sound_events::ENTITY_WARDEN_LISTENING_ANGRY,
        }
    }
}

/// A live suspect: vanilla's `suspects` entry plus the UUID the table is keyed by.
struct Suspect {
    uuid: Uuid,
    entity: WeakEntity,
}

/// Returns whether vanilla moves a removed suspect's anger into the serialized
/// table instead of forgetting it.
const fn persists_anger(reason: RemovalReason) -> bool {
    matches!(
        reason,
        RemovalReason::ChangedWorld
            | RemovalReason::UnloadedToChunk
            | RemovalReason::StoredWithPlayer
    )
}

/// Vanilla `AngerManagement`: the per-suspect anger table of one warden.
pub(crate) struct AngerManagement {
    /// Vanilla `AngerManagement.conversionDelay`.
    conversion_delay: i32,
    /// Vanilla `AngerManagement.highestAnger`.
    highest_anger: i32,
    /// Vanilla `AngerManagement.suspects`, kept in vanilla's sorter order.
    suspects: Vec<Suspect>,
    /// Vanilla `AngerManagement.angerBySuspect`, keyed by suspect UUID.
    anger_by_suspect: FxHashMap<Uuid, i32>,
    /// Vanilla `AngerManagement.angerByUuid`, the serialized suspects.
    anger_by_uuid: Vec<(Uuid, i32)>,
}

impl Default for AngerManagement {
    fn default() -> Self {
        Self::new()
    }
}

impl AngerManagement {
    /// Creates an empty table, as vanilla's constructor with no saved suspects.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            conversion_delay: rand::random_range(0..=CONVERSION_DELAY),
            highest_anger: 0,
            suspects: Vec::new(),
            anger_by_suspect: FxHashMap::default(),
            anger_by_uuid: Vec::new(),
        }
    }

    /// Vanilla `AngerManagement.tick`: decay every suspect, resolve the
    /// serialized suspects, and re-sort.
    pub(crate) fn tick(&mut self, world: &World, is_valid: impl Fn(&SharedEntity) -> bool) {
        self.conversion_delay -= 1;
        if self.conversion_delay <= 0 {
            self.convert_from_uuids(world);
            self.conversion_delay = CONVERSION_DELAY;
        }

        self.anger_by_uuid.retain_mut(|(_, anger)| {
            if *anger <= 1 {
                return false;
            }
            *anger -= DEFAULT_ANGER_DECREASE;
            true
        });

        let suspects = mem::take(&mut self.suspects);
        for suspect in suspects {
            let Some(anger) = self.anger_by_suspect.get(&suspect.uuid).copied() else {
                continue;
            };

            let entity = suspect.entity.upgrade();
            let removal_reason = entity.as_ref().and_then(|entity| entity.removal_reason());
            let unloaded = entity.is_none();
            let valid = entity.as_ref().is_some_and(&is_valid) && removal_reason.is_none();

            if anger > 1 && valid {
                self.anger_by_suspect
                    .insert(suspect.uuid, anger - DEFAULT_ANGER_DECREASE);
                self.suspects.push(suspect);
                continue;
            }

            self.anger_by_suspect.remove(&suspect.uuid);

            // Vanilla keeps the anger of suspects that left the level or the
            // loaded area, and forgets every other invalid suspect.
            let persists = unloaded || removal_reason.is_some_and(persists_anger);
            if anger > 1 && persists {
                self.anger_by_uuid
                    .push((suspect.uuid, anger - DEFAULT_ANGER_DECREASE));
            }
        }

        self.sort_and_update_highest_anger();
    }

    /// Vanilla `AngerManagement.convertFromUuids`: resolve saved suspects that
    /// are loaded again.
    fn convert_from_uuids(&mut self, world: &World) {
        for (uuid, anger) in mem::take(&mut self.anger_by_uuid) {
            let Some(entity) = world.get_entity_by_uuid(&uuid) else {
                self.anger_by_uuid.push((uuid, anger));
                continue;
            };

            self.anger_by_suspect.insert(uuid, anger);
            self.suspects.push(Suspect {
                uuid,
                entity: Arc::downgrade(&entity),
            });
        }
    }

    /// Vanilla `AngerManagement.increaseAnger`, including its
    /// `MIN(150, anger + increment)` clamp *before* the serialized anger of a new
    /// suspect is added.
    pub(crate) fn increase_anger(&mut self, entity: &SharedEntity, increment: i32) -> i32 {
        let uuid = entity.uuid();
        let new_suspect = !self.anger_by_suspect.contains_key(&uuid);
        let mut current_anger =
            (self.anger_by_suspect.get(&uuid).copied().unwrap_or(0) + increment).min(MAX_ANGER);
        self.anger_by_suspect.insert(uuid, current_anger);

        if new_suspect {
            if let Some(index) = self
                .anger_by_uuid
                .iter()
                .position(|(serialized, _)| *serialized == uuid)
            {
                current_anger += self.anger_by_uuid.remove(index).1;
            }
            self.anger_by_suspect.insert(uuid, current_anger);
            self.suspects.push(Suspect {
                uuid,
                entity: Arc::downgrade(entity),
            });
        }

        self.sort_and_update_highest_anger();
        current_anger
    }

    /// Vanilla `AngerManagement.clearAnger`.
    pub(crate) fn clear_anger(&mut self, entity: &SharedEntity) {
        let uuid = entity.uuid();
        self.anger_by_suspect.remove(&uuid);
        self.suspects.retain(|suspect| suspect.uuid != uuid);
        self.sort_and_update_highest_anger();
    }

    /// Vanilla `AngerManagement.getActiveAnger`.
    #[must_use]
    pub(crate) fn get_active_anger(&self, current_target: Option<&SharedEntity>) -> i32 {
        current_target.map_or(self.highest_anger, |target| {
            self.anger_by_suspect
                .get(&target.uuid())
                .copied()
                .unwrap_or(0)
        })
    }

    /// Vanilla `AngerManagement.getActiveEntity`: the first sorted suspect that
    /// passes `filter` and is a living entity.
    #[must_use]
    pub(crate) fn get_active_entity(
        &self,
        filter: impl Fn(&SharedEntity) -> bool,
    ) -> Option<SharedEntity> {
        self.suspects
            .iter()
            .filter_map(|suspect| suspect.entity.upgrade())
            .find(|entity| entity.is_living_entity() && filter(entity))
    }

    /// Vanilla `AngerManagement.sortAndUpdateHighestAnger`.
    ///
    /// Vanilla's comparator updates `highestAnger` as a side effect of the sort;
    /// recomputing the maximum over the same suspects afterwards is equivalent,
    /// including vanilla's `suspects.size() == 1` special case.
    fn sort_and_update_highest_anger(&mut self) {
        let anger_by_suspect = &self.anger_by_suspect;
        self.suspects.sort_by(|left, right| {
            if left.uuid == right.uuid {
                return Ordering::Equal;
            }

            let left_anger = anger_by_suspect.get(&left.uuid).copied().unwrap_or(0);
            let right_anger = anger_by_suspect.get(&right.uuid).copied().unwrap_or(0);
            let left_angry = AngerLevel::by_anger(left_anger).is_angry();
            let right_angry = AngerLevel::by_anger(right_anger).is_angry();
            if left_angry != right_angry {
                return if left_angry {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }

            let left_player = left
                .entity
                .upgrade()
                .is_some_and(|entity| entity.as_player().is_some());
            let right_player = right
                .entity
                .upgrade()
                .is_some_and(|entity| entity.as_player().is_some());
            if left_player != right_player {
                return if left_player {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }

            right_anger.cmp(&left_anger)
        });

        self.highest_anger = self
            .suspects
            .iter()
            .map(|suspect| {
                self.anger_by_suspect
                    .get(&suspect.uuid)
                    .copied()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0);
    }

    /// Vanilla `AngerManagement.createUuidAngerPairs`, written as the `anger`
    /// compound (`suspects` list of `uuid`/`anger` pairs).
    pub(crate) fn save(&self, nbt: &mut NbtCompound) {
        let mut suspects = Vec::with_capacity(self.suspects.len() + self.anger_by_uuid.len());
        for suspect in &self.suspects {
            let Some(anger) = self.anger_by_suspect.get(&suspect.uuid) else {
                continue;
            };
            suspects.push(suspect_entry(suspect.uuid, *anger));
        }
        for (uuid, anger) in &self.anger_by_uuid {
            suspects.push(suspect_entry(*uuid, *anger));
        }

        let mut anger = NbtCompound::new();
        anger.insert("suspects", NbtList::Compound(suspects));
        nbt.insert("anger", anger);
    }

    /// Vanilla `AngerManagement.codec`'s decode half, including
    /// `lenientOptionalFieldOf`'s "a malformed list falls back to empty" rule.
    #[must_use]
    pub(crate) fn load(nbt: &BorrowedNbtCompoundView<'_, '_>) -> Self {
        let mut anger_by_uuid = Vec::new();
        let mut well_formed = true;

        if let Some(compounds) = nbt
            .compound("anger")
            .and_then(|anger| anger.list("suspects"))
            .and_then(|list| list.compounds())
        {
            for compound in compounds {
                match (
                    compound
                        .int_array("uuid")
                        .and_then(|uuid| Uuid::from_int_array(&uuid)),
                    compound.int("anger"),
                ) {
                    (Some(uuid), Some(anger)) if anger >= 0 => anger_by_uuid.push((uuid, anger)),
                    _ => well_formed = false,
                }
            }
        }

        Self {
            anger_by_uuid: if well_formed {
                anger_by_uuid
            } else {
                Vec::new()
            },
            ..Self::new()
        }
    }
}

/// Vanilla `AngerManagement.SUSPECT_ANGER_PAIR`.
fn suspect_entry(uuid: Uuid, anger: i32) -> NbtCompound {
    let mut entry = NbtCompound::new();
    entry.insert("uuid", NbtTag::IntArray(uuid.to_int_array().to_vec()));
    entry.insert("anger", anger);
    entry
}
