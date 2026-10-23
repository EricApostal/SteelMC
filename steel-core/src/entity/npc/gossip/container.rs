//! Villager gossip storage (vanilla `GossipContainer`).

use rustc_hash::{FxHashMap, FxHashSet};
use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::UuidExt as _;
use steel_utils::random::Random;
use uuid::Uuid;

use super::GossipType;

/// Vanilla `GossipContainer.DISCARD_THRESHOLD`: values below this are dropped.
pub const DISCARD_THRESHOLD: i32 = 2;
/// Vanilla `Villager.GOSSIP_COOLDOWN`: ticks between two gossip exchanges for the same villager.
pub const GOSSIP_COOLDOWN: i64 = 1200;
/// Vanilla `Villager.GOSSIP_DECAY_INTERVAL`: ticks between two gossip decays.
pub const GOSSIP_DECAY_INTERVAL: i64 = 24000;
/// Vanilla `Villager.MAX_GOSSIP_TOPICS`: how many gossip entries a single transfer can move.
pub const MAX_GOSSIP_TOPICS: usize = 10;

/// NBT key of the whole gossip list on the owning entity.
const GOSSIPS_NBT_KEY: &str = "Gossips";
/// NBT key of a gossip entry's target UUID (vanilla codec field name).
const TARGET_NBT_KEY: &str = "Target";
/// NBT key of a gossip entry's type id (vanilla codec field name).
const TYPE_NBT_KEY: &str = "Type";
/// NBT key of a gossip entry's value (vanilla codec field name).
const VALUE_NBT_KEY: &str = "Value";

/// One stored gossip value (vanilla `GossipContainer.GossipEntry`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GossipEntry {
    /// The player (or witness UUID) the gossip is about.
    pub target: Uuid,
    /// Vanilla field `type`; renamed because `type` is a Rust keyword.
    pub kind: GossipType,
    /// The raw stored value, scaled by [`GossipType::weight`] to get reputation points.
    pub value: i32,
}

impl GossipEntry {
    /// Reputation points this entry contributes.
    #[must_use]
    pub const fn weighted_value(self) -> i32 {
        self.value * self.kind.weight()
    }
}

/// The per-target value map (vanilla `GossipContainer.EntityGossips`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct EntityGossips {
    entries: FxHashMap<GossipType, i32>,
}

impl EntityGossips {
    fn weighted_value(&self, types: impl Fn(GossipType) -> bool) -> i32 {
        self.entries
            .iter()
            .filter(|(kind, _)| types(**kind))
            .map(|(kind, value)| value * kind.weight())
            .sum()
    }

    fn unpack(&self, target: Uuid) -> impl Iterator<Item = GossipEntry> + '_ {
        self.entries.iter().map(move |(&kind, &value)| GossipEntry {
            target,
            kind,
            value,
        })
    }

    fn decay(&mut self) {
        self.entries.retain(|kind, value| {
            *value -= kind.decay_per_day();
            *value >= DISCARD_THRESHOLD
        });
    }

    fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn clamp_value(&mut self, kind: GossipType) {
        let value = self.entries.get(&kind).copied().unwrap_or(0);
        if value > kind.max() {
            self.entries.insert(kind, kind.max());
        }
        if value < DISCARD_THRESHOLD {
            self.entries.remove(&kind);
        }
    }
}

/// The gossip a mob knows about other entities (vanilla `GossipContainer`).
///
/// Vanilla 26.3 dropped the older `maxAndClear`/`update`/`removeUUID` API: a per-type value is
/// capped by [`GossipType::max`] on `add`, small values are discarded rather than truncated
/// ([`DISCARD_THRESHOLD`]), gossip about dead entities simply decays away, and
/// [`MAX_GOSSIP_TOPICS`] only bounds how many entries a single `transfer_from` call can move.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GossipContainer {
    gossips: FxHashMap<Uuid, EntityGossips>,
}

impl GossipContainer {
    /// Creates an empty container.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a container from raw gossip entries, mirroring the vanilla codec's
    /// `entries.forEach(e -> getOrCreate(e.target).entries.put(e.type, e.value))`.
    #[must_use]
    pub fn from_entries(entries: impl IntoIterator<Item = GossipEntry>) -> Self {
        let mut container = Self::new();
        for entry in entries {
            container
                .get_or_create(entry.target)
                .entries
                .insert(entry.kind, entry.value);
        }
        container
    }

    /// Subtracts [`GossipType::decay_per_day`] from every value and drops targets whose entries
    /// all fall below [`DISCARD_THRESHOLD`]. Runs every [`GOSSIP_DECAY_INTERVAL`] ticks.
    pub fn decay(&mut self) {
        self.gossips.retain(|_, gossips| {
            gossips.decay();
            !gossips.is_empty()
        });
    }

    /// Iterates every stored gossip value.
    pub fn unpack(&self) -> impl Iterator<Item = GossipEntry> + '_ {
        self.gossips
            .iter()
            .flat_map(|(&target, gossips)| gossips.unpack(target))
    }

    /// Whether this container holds no gossip.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gossips.is_empty()
    }

    /// Picks up to `max_count` gossip entries by absolute weighted value
    /// (vanilla `GossipContainer.selectGossipsForTransfer`).
    ///
    /// Vanilla collects into an identity set, so an entry drawn twice only counts once. It also
    /// iterates a `HashMap`, so the draw order is unspecified there; Steel sorts the drawn entries
    /// by its own iteration order, which is equivalent because transfers merge with `max`.
    fn select_gossips_for_transfer(
        &self,
        random: &mut impl Random,
        max_count: usize,
    ) -> Vec<GossipEntry> {
        let entries: Vec<GossipEntry> = self.unpack().collect();
        if entries.is_empty() {
            return Vec::new();
        }

        let mut ranges = Vec::with_capacity(entries.len());
        let mut ranges_end = 0;
        for entry in &entries {
            ranges_end += entry.weighted_value().abs();
            ranges.push(ranges_end - 1);
        }
        // Only reachable if a caller stored a zero value directly; vanilla's `nextInt` requires a
        // positive bound.
        if ranges_end <= 0 {
            return Vec::new();
        }

        let mut selected: FxHashSet<usize> = FxHashSet::default();
        for _ in 0..max_count {
            let choice = random.next_i32_bounded(ranges_end);
            let index = ranges.binary_search(&choice).unwrap_or_else(|index| index);
            selected.insert(index);
        }

        let mut indices: Vec<usize> = selected.into_iter().collect();
        indices.sort_unstable();
        indices.into_iter().map(|index| entries[index]).collect()
    }

    fn get_or_create(&mut self, target: Uuid) -> &mut EntityGossips {
        self.gossips.entry(target).or_default()
    }

    /// Copies up to `max_count` gossip entries from `source`, decaying each received value by
    /// [`GossipType::decay_per_transfer`] and merging with the higher existing value
    /// (vanilla `GossipContainer.transferFrom`).
    ///
    /// Returns the number of gossip entries selected for transfer (vanilla's `newGossips.size()`),
    /// which is higher than the number merged when a decayed value falls below
    /// [`DISCARD_THRESHOLD`].
    ///
    /// Wave 2 call sites: `Villager.gossip` (Villager.java:819-831) passes the villager's own RNG,
    /// [`MAX_GOSSIP_TOPICS`] and [`GOSSIP_COOLDOWN`].
    pub fn transfer_from(
        &mut self,
        source: &Self,
        random: &mut impl Random,
        max_count: usize,
    ) -> usize {
        let new_gossips = source.select_gossips_for_transfer(random, max_count);
        for gossip in &new_gossips {
            let decayed_value = gossip.value - gossip.kind.decay_per_transfer();
            if decayed_value >= DISCARD_THRESHOLD {
                self.get_or_create(gossip.target)
                    .entries
                    .entry(gossip.kind)
                    .and_modify(|value| *value = (*value).max(decayed_value))
                    .or_insert(decayed_value);
            }
        }
        new_gossips.len()
    }

    /// Weighted reputation of `entity`, filtered by `types`
    /// (vanilla `GossipContainer.getReputation`). Full reputation is `types = |_| true`.
    #[must_use]
    pub fn get_reputation(&self, entity: Uuid, types: impl Fn(GossipType) -> bool) -> i32 {
        self.gossips
            .get(&entity)
            .map_or(0, |gossips| gossips.weighted_value(types))
    }

    /// Counts targets whose weighted value for `kind` passes `value_test`
    /// (vanilla `GossipContainer.getCountForType`).
    #[must_use]
    pub fn get_count_for_type(&self, kind: GossipType, value_test: impl Fn(f64) -> bool) -> u64 {
        self.gossips
            .values()
            .filter(|gossips| {
                let weighted = gossips.entries.get(&kind).copied().unwrap_or(0) * kind.weight();
                value_test(f64::from(weighted))
            })
            .count() as u64
    }

    /// Adds `amount_to_add` to a target's value for `kind`, capping at [`GossipType::max`] and
    /// discarding values below [`DISCARD_THRESHOLD`] (vanilla `GossipContainer.add`).
    pub fn add(&mut self, target: Uuid, kind: GossipType, amount_to_add: i32) {
        let gossips = self.get_or_create(target);
        gossips
            .entries
            .entry(kind)
            .and_modify(|value| *value = merge_values_for_addition(kind, *value, amount_to_add))
            .or_insert(amount_to_add);
        gossips.clamp_value(kind);
        if gossips.is_empty() {
            self.gossips.remove(&target);
        }
    }

    /// Subtracts `amount_to_remove` from a target's value for `kind`.
    pub fn remove(&mut self, target: Uuid, kind: GossipType, amount_to_remove: i32) {
        self.add(target, kind, -amount_to_remove);
    }

    /// Removes a target's value for `kind` entirely
    /// (vanilla `GossipContainer.remove(UUID, GossipType)`; `remove` above is the three-argument
    /// overload).
    pub fn remove_entry(&mut self, target: Uuid, kind: GossipType) {
        let Some(gossips) = self.gossips.get_mut(&target) else {
            return;
        };
        gossips.entries.remove(&kind);
        if gossips.is_empty() {
            self.gossips.remove(&target);
        }
    }

    /// Removes `kind` from every target (vanilla `GossipContainer.remove(GossipType)`).
    pub fn remove_type(&mut self, kind: GossipType) {
        self.gossips.retain(|_, gossips| {
            gossips.entries.remove(&kind);
            !gossips.is_empty()
        });
    }

    /// Removes all gossip.
    pub fn clear(&mut self) {
        self.gossips.clear();
    }

    /// Overwrites this container's values with `container`'s (vanilla `GossipContainer.putAll`).
    pub fn put_all(&mut self, container: &Self) {
        for (&target, gossips) in &container.gossips {
            self.get_or_create(target)
                .entries
                .extend(gossips.entries.iter().map(|(&kind, &value)| (kind, value)));
        }
    }

    /// Deep copy (vanilla `GossipContainer.copy`, used when a villager turns into a zombie).
    #[must_use]
    pub fn copy(&self) -> Self {
        let mut container = Self::new();
        container.put_all(self);
        container
    }

    /// Writes the gossip list under `Gossips`, matching the vanilla codec shape
    /// (`{Target: [4 ints], Type: id, Value: int}`).
    pub fn save_to(&self, nbt: &mut NbtCompound) {
        let entries: Vec<NbtCompound> = self
            .unpack()
            .map(|entry| {
                let mut compound = NbtCompound::new();
                compound.insert(
                    TARGET_NBT_KEY,
                    NbtTag::IntArray(entry.target.to_int_array().to_vec()),
                );
                compound.insert(TYPE_NBT_KEY, entry.kind.id());
                compound.insert(VALUE_NBT_KEY, entry.value);
                compound
            })
            .collect();
        // An empty gossip list is omitted; loading treats a missing list as no gossip, matching
        // vanilla's `clear()` + `ifPresent(putAll)` load.
        if !entries.is_empty() {
            nbt.insert(GOSSIPS_NBT_KEY, NbtList::Compound(entries));
        }
    }

    /// Clears this container and loads the gossip list stored under `Gossips`
    /// (vanilla `Villager.readAdditionalSaveData`, Villager.java:482-483).
    ///
    /// Entries with an unknown type id, a malformed target or a non-positive value are skipped.
    pub fn load_from(&mut self, nbt: BorrowedNbtCompound<'_, '_>) {
        self.clear();
        let Some(compounds) = nbt.list(GOSSIPS_NBT_KEY).and_then(|list| list.compounds()) else {
            return;
        };
        for compound in compounds {
            let Some(target) = compound
                .int_array(TARGET_NBT_KEY)
                .and_then(|array| Uuid::from_int_array(&array))
            else {
                continue;
            };
            let Some(kind) = compound
                .string(TYPE_NBT_KEY)
                .and_then(|id| GossipType::from_id(id.to_str().as_ref()))
            else {
                continue;
            };
            // Vanilla's codec uses `ExtraCodecs.POSITIVE_INT` for `Value`.
            let Some(value) = compound.int(VALUE_NBT_KEY).filter(|value| *value >= 1) else {
                continue;
            };
            self.get_or_create(target).entries.insert(kind, value);
        }
    }
}

/// Vanilla `GossipContainer.mergeValuesForAddition`: the sum, saturated at the type maximum but
/// never lowering an already out-of-range value.
pub(super) fn merge_values_for_addition(kind: GossipType, old_value: i32, new_value: i32) -> i32 {
    let sum = old_value + new_value;
    if sum > kind.max() {
        kind.max().max(old_value)
    } else {
        sum
    }
}
