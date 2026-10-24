//! Vanilla `SlotRanges`: the named slot ranges commands and slot sources use.
//!
//! Mirrors `net.minecraft.world.inventory.{SlotRange, SlotRanges}`. The table
//! is vanilla's declaration order verbatim, including the per-index names
//! (`container.0` … `container.53`) and the `*` aggregate of every prefix.

/// Vanilla `world.inventory.SlotRange`: a named range of vanilla slot ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SlotRange {
    name: Box<str>,
    slots: Box<[i32]>,
}

impl SlotRange {
    /// The vanilla serialized name (`SlotRanges` key).
    #[must_use]
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The vanilla slot ids covered by this range, in vanilla order.
    #[must_use]
    pub(crate) fn slots(&self) -> &[i32] {
        &self.slots
    }

    /// Returns whether this range covers exactly one slot.
    #[must_use]
    pub(crate) fn is_single(&self) -> bool {
        self.slots.len() == 1
    }
}

/// Vanilla `SlotRanges.SLOTS`: every named range, in vanilla declaration order.
#[must_use]
pub(crate) fn slot_ranges() -> &'static [SlotRange] {
    static SLOT_RANGES: std::sync::LazyLock<Vec<SlotRange>> =
        std::sync::LazyLock::new(build_slot_ranges);
    &SLOT_RANGES
}

/// Vanilla `SlotRanges.nameToIds`.
#[must_use]
pub(crate) fn name_to_range(name: &str) -> Option<&'static SlotRange> {
    slot_ranges().iter().find(|range| range.name() == name)
}

fn build_slot_ranges() -> Vec<SlotRange> {
    fn single(ranges: &mut Vec<SlotRange>, name: impl Into<String>, id: i32) {
        push(ranges, name, vec![id]);
    }

    fn push(ranges: &mut Vec<SlotRange>, name: impl Into<String>, slots: Vec<i32>) {
        ranges.push(SlotRange {
            name: name.into().into_boxed_str(),
            slots: slots.into_boxed_slice(),
        });
    }

    fn range(ranges: &mut Vec<SlotRange>, prefix: &str, offset: i32, size: i32) {
        let mut all = Vec::with_capacity(size as usize);
        for index in 0..size {
            let slot = offset + index;
            single(ranges, format!("{prefix}{index}"), slot);
            all.push(slot);
        }
        push(ranges, format!("{prefix}*"), all);
    }

    // Vanilla resolves the equipment indices through `EquipmentSlot.getIndex`:
    // mainhand 98, offhand 99, feet 100, legs 101, chest 102, head 103, body
    // 105, saddle 106.
    let mut ranges = Vec::new();
    single(&mut ranges, "contents", 0);
    range(&mut ranges, "container.", 0, 54);
    range(&mut ranges, "hotbar.", 0, 9);
    range(&mut ranges, "inventory.", 9, 27);
    range(&mut ranges, "enderchest.", 200, 27);
    range(&mut ranges, "mob.inventory.", 300, 8);
    range(&mut ranges, "horse.", 500, 15);
    single(&mut ranges, "weapon", 98);
    single(&mut ranges, "weapon.mainhand", 98);
    single(&mut ranges, "weapon.offhand", 99);
    push(&mut ranges, "weapon.*", vec![98, 99]);
    single(&mut ranges, "armor.head", 103);
    single(&mut ranges, "armor.chest", 102);
    single(&mut ranges, "armor.legs", 101);
    single(&mut ranges, "armor.feet", 100);
    single(&mut ranges, "armor.body", 105);
    push(&mut ranges, "armor.*", vec![103, 102, 101, 100, 105]);
    single(&mut ranges, "saddle", 106);
    single(&mut ranges, "horse.chest", 499);
    single(&mut ranges, "player.cursor", 499);
    range(&mut ranges, "player.crafting.", 500, 4);
    ranges
}
