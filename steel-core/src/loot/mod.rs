//! Runtime loot machinery.
//!
//! The slot model shared by `/item`, `execute if items`, and slot-bearing loot
//! functions. Vanilla shape: `net.minecraft.world.item.slot.*`,
//! `net.minecraft.world.entity.{SlotAccess, SlotProvider}`, and
//! `net.minecraft.world.item.ItemProvider`.
//!
//! This module also carries the two context objects the `minecraft:dynamic` and
//! `minecraft:slots` loot entries read, which vanilla keeps on `LootParams`:
//! [`LootedContainer`] (the looted container's stacks) and [`LootSlotSources`]
//! (the `slot_source` resolver).

pub mod slot;

use steel_registry::{
    item_stack::ItemStack,
    loot_table::{BlockEntityRef, LootSlotSourceResolver},
    slot_source::SlotSourceHolder,
};

use crate::block_entity::BlockEntity;
use crate::inventory::lock::ContainerLockGuard;
use slot::{FnSlotAccess, SlotAccess, SlotProvider, SlotSourceContext, provide_slot_source_holder};

/// The looted container of a block drop, snapshotted for the loot context.
///
/// Vanilla hangs the live container off the loot parameters: the container
/// blocks register their own slots as the `minecraft:contents` dynamic drop,
/// and `BaseContainerBlockEntity.getLootContext` installs the same container as
/// `LootContextParams.CONTAINER` (plus the block entity itself as
/// `BLOCK_ENTITY`).
///
/// Steel copies the stacks instead of lending the locked container: a loot table
/// can raise advancements and scoreboard writes, and no container lock may be
/// held across that.
pub(crate) struct LootedContainer {
    items: Vec<ItemStack>,
}

impl LootedContainer {
    /// Snapshots the container of `block_entity`, or `None` when there is no
    /// block entity or it has no container capability.
    #[must_use]
    pub(crate) fn snapshot(block_entity: Option<&dyn BlockEntity>) -> Option<Self> {
        let container_ref = block_entity?.container_ref()?;
        let guard = ContainerLockGuard::lock_all(&[&container_ref]);
        let container = guard.get(container_ref.container_id())?;
        Some(Self {
            items: container.items().to_vec(),
        })
    }

    /// The snapshotted stacks in vanilla slot order, empty slots included.
    #[must_use]
    pub(crate) fn items(&self) -> &[ItemStack] {
        &self.items
    }

    /// Vanilla's block-entity view of this container: the looted block entity
    /// with the container's stacks attached.
    #[must_use]
    pub(crate) fn block_entity_ref<'a>(
        &'a self,
        block_entity: &'a dyn BlockEntity,
    ) -> BlockEntityRef<'a> {
        BlockEntityRef {
            block_entity_type: Some(&block_entity.get_type().key),
            custom_name: None,
            inventory: Some(&self.items),
        }
    }
}

/// Vanilla `Container` as a `SlotProvider`, over the copied stacks.
///
/// The snapshot refuses writes: loot evaluation only reads (`SlotLoot` streams
/// `SlotCollection.itemCopies()`), and a copy is not the container. Vanilla's
/// `SlotAccess.set` returning `false` is exactly "this slot refused the stack".
impl SlotProvider for LootedContainer {
    fn get_slot(&self, slot: i32) -> Option<Box<dyn SlotAccess + '_>> {
        let index = usize::try_from(slot).ok()?;
        let stack = self.items.get(index)?.clone();
        Some(Box::new(FnSlotAccess::new(
            move || stack.clone(),
            |_| false,
        )))
    }
}

/// The `slot_source` resolver a loot context installs.
///
/// Vanilla `SlotSource.provide(LootContext)` reads the context objects
/// (`LootContextParams.CONTAINER`, the entity targets, `BLOCK_ENTITY`) and keeps
/// the ones that are `SlotProvider`s. Steel's registry-side `LootContext` only
/// carries type-erased views, so the caller hands the providers it can address;
/// an object the caller cannot address stays absent and resolves to no slots,
/// which is vanilla's fallback for a context object that is not a provider.
pub(crate) struct LootSlotSources<'a> {
    container: Option<&'a dyn SlotProvider>,
    this_entity: Option<&'a dyn SlotProvider>,
    block_entity: Option<&'a dyn SlotProvider>,
}

impl<'a> LootSlotSources<'a> {
    /// Builds a resolver from the providers the caller can address.
    #[must_use]
    pub(crate) const fn new(
        container: Option<&'a dyn SlotProvider>,
        this_entity: Option<&'a dyn SlotProvider>,
        block_entity: Option<&'a dyn SlotProvider>,
    ) -> Self {
        Self {
            container,
            this_entity,
            block_entity,
        }
    }
}

impl LootSlotSourceResolver for LootSlotSources<'_> {
    fn slot_source_items(&self, source: &'static SlotSourceHolder) -> Vec<ItemStack> {
        let context = SlotSourceContext {
            container: self.container,
            this_entity: self.this_entity,
            block_entity: self.block_entity,
        };
        provide_slot_source_holder(source, &context).item_copies()
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::assert_is_empty,
        reason = "`assert!(xs.is_empty())` reads better than equality against an empty array"
    )]

    use std::sync::Weak;

    use rand::SeedableRng;
    use rand::rngs::StdRng;
    use steel_registry::{
        init_vanilla_registry,
        loot_table::{LootEntry, LootPool, LootTable, LootType, NumberProvider},
        slot_source::{RangeSlotSource, SlotSource, SlotSourceContextArg, SlotSourceHolder},
        vanilla_blocks, vanilla_items,
    };
    use steel_utils::{BlockPos, Identifier};

    use super::*;
    use crate::block_entity::entities::ChestBlockEntity;

    /// The vanilla `minecraft:contents` dynamic-drop id.
    const CONTENTS: Identifier = Identifier::vanilla_static("contents");

    fn contents_table() -> LootTable {
        static POOLS: &[LootPool] = &[LootPool {
            rolls: NumberProvider::Constant(1.0),
            bonus_rolls: 0.0,
            entries: &[LootEntry::Dynamic {
                name: CONTENTS,
                conditions: &[],
            }],
            conditions: &[],
            functions: &[],
        }];
        LootTable {
            key: Identifier::vanilla_static("test/chest_contents"),
            loot_type: LootType::Block,
            pools: POOLS,
            functions: &[],
            random_sequence: None,
        }
    }

    /// A chest with an empty first slot and an out-of-range tail: vanilla slot
    /// 0 is empty, vanilla slot 2 holds a diamond.
    fn stocked_chest() -> ChestBlockEntity {
        init_vanilla_registry();
        let chest = ChestBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::CHEST.default_state(),
        );
        chest
            .container_ref()
            .and_then(|container_ref| {
                let mut guard = ContainerLockGuard::lock_all(&[&container_ref]);
                let container = guard.get_mut(container_ref.container_id())?;
                for slot in 0..container.get_container_size() {
                    container.set_item(slot, ItemStack::empty());
                }
                container.set_item(2, ItemStack::with_count(&vanilla_items::DIAMOND, 3));
                Some(())
            })
            .expect("a fresh chest exposes a container");
        chest
    }

    #[test]
    fn chest_snapshot_feeds_the_dynamic_contents_drop() {
        let chest = stocked_chest();
        let container = LootedContainer::snapshot(Some(&chest as &dyn BlockEntity))
            .expect("a chest has a container");
        assert!(container.items()[0].is_empty());
        assert_eq!(
            container.items()[2],
            ItemStack::with_count(&vanilla_items::DIAMOND, 3)
        );

        let slot_sources = LootSlotSources::new(Some(&container), None, Some(&container));
        let mut rng = StdRng::seed_from_u64(1);
        let mut ctx = steel_registry::loot_table::LootContext::new(&mut rng)
            .with_block_entity(container.block_entity_ref(&chest))
            .with_slot_sources(&slot_sources);
        let drops = contents_table().get_random_items(&mut ctx);

        // The container drop mirrors knocking the chest open: the diamond, and
        // nothing for the empty slots.
        assert_eq!(drops, [ItemStack::with_count(&vanilla_items::DIAMOND, 3)]);
    }

    #[test]
    fn slots_range_reads_only_the_selected_container_slots() {
        static RANGE: SlotSource = SlotSource::SlotRange(RangeSlotSource {
            source: SlotSourceContextArg::Container,
            slots: "container.*",
        });
        static POOLS: &[LootPool] = &[LootPool {
            rolls: NumberProvider::Constant(1.0),
            bonus_rolls: 0.0,
            entries: &[LootEntry::Slots {
                slot_source: &SlotSourceHolder::Inline(&RANGE),
                conditions: &[],
                functions: &[],
            }],
            conditions: &[],
            functions: &[],
        }];
        static TABLE: LootTable = LootTable {
            key: Identifier::vanilla_static("test/chest_slots"),
            loot_type: LootType::Block,
            pools: POOLS,
            functions: &[],
            random_sequence: None,
        };
        static OUT_OF_RANGE_RANGE: SlotSource = SlotSource::SlotRange(RangeSlotSource {
            source: SlotSourceContextArg::Container,
            slots: "enderchest.*",
        });
        static OUT_OF_RANGE_POOLS: &[LootPool] = &[LootPool {
            rolls: NumberProvider::Constant(1.0),
            bonus_rolls: 0.0,
            entries: &[LootEntry::Slots {
                slot_source: &SlotSourceHolder::Inline(&OUT_OF_RANGE_RANGE),
                conditions: &[],
                functions: &[],
            }],
            conditions: &[],
            functions: &[],
        }];
        static OUT_OF_RANGE_TABLE: LootTable = LootTable {
            key: Identifier::vanilla_static("test/out_of_range_slots"),
            loot_type: LootType::Block,
            pools: OUT_OF_RANGE_POOLS,
            functions: &[],
            random_sequence: None,
        };

        let chest = stocked_chest();
        let container = LootedContainer::snapshot(Some(&chest as &dyn BlockEntity))
            .expect("a chest has a container");
        let slot_sources = LootSlotSources::new(Some(&container), None, Some(&container));
        let mut rng = StdRng::seed_from_u64(1);
        let mut ctx = steel_registry::loot_table::LootContext::new(&mut rng)
            .with_block_entity(container.block_entity_ref(&chest))
            .with_slot_sources(&slot_sources);

        // `container.*` names vanilla slots 0-53 of a 27-slot chest: slot 2's
        // diamond drops, the 26 other in-range slots are empty, and the
        // out-of-range tail contributes nothing.
        assert_eq!(
            TABLE.get_random_items(&mut ctx),
            [ItemStack::with_count(&vanilla_items::DIAMOND, 3)]
        );

        // `enderchest.*` names vanilla slots 200-226, entirely beyond the
        // chest: a range the container has no slot for yields nothing.
        assert!(OUT_OF_RANGE_TABLE.get_random_items(&mut ctx).is_empty());
    }

    #[test]
    fn slots_range_against_no_container_yields_nothing() {
        static RANGE: SlotSource = SlotSource::SlotRange(RangeSlotSource {
            source: SlotSourceContextArg::Container,
            slots: "container.0",
        });
        static POOLS: &[LootPool] = &[LootPool {
            rolls: NumberProvider::Constant(1.0),
            bonus_rolls: 0.0,
            entries: &[LootEntry::Slots {
                slot_source: &SlotSourceHolder::Inline(&RANGE),
                conditions: &[],
                functions: &[],
            }],
            conditions: &[],
            functions: &[],
        }];
        static TABLE: LootTable = LootTable {
            key: Identifier::vanilla_static("test/empty_slots"),
            loot_type: LootType::Block,
            pools: POOLS,
            functions: &[],
            random_sequence: None,
        };

        init_vanilla_registry();
        let slot_sources = LootSlotSources::new(None, None, None);
        let mut rng = StdRng::seed_from_u64(1);
        let mut ctx =
            steel_registry::loot_table::LootContext::new(&mut rng).with_slot_sources(&slot_sources);
        assert!(TABLE.get_random_items(&mut ctx).is_empty());
    }
}
