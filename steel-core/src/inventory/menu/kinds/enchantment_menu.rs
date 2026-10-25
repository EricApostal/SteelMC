//! Enchantment (enchanting table) menu.
//!
//! Mirrors Vanilla `EnchantmentMenu`: the menu owns a two-slot container (the
//! item being enchanted and the lapis lazuli), recomputes the three offers from
//! the powered bookshelves around the table, and applies the selected offer on
//! a container button click. The menu takes the table position instead of a
//! container reference because the container is menu-local, exactly like
//! Vanilla's anonymous `SimpleContainer(2)`.
//!
//! # Determinism
//!
//! The offers are a pure function of the table's enchantment seed and the item,
//! so the rolls must follow Java's `RandomSource` exactly. Vanilla reseeds one
//! `RandomSource` with the seed (and with `seed + slot` for each offer) and
//! draws through `nextInt`/`nextFloat`. [`steel_registry::enchantment::table`]
//! exposes the same maths over a generic [`rand::Rng`], which cannot reproduce
//! `nextInt(bound)` for non-power-of-two bounds, so the RNG-touching parts are
//! ported here on [`LegacyRandom`] while the RNG-free selection helpers are
//! reused from that module.

use std::array::from_fn;
use std::sync::Arc;

use steel_protocol::packets::game::SoundSource;
use steel_registry::{
    REGISTRY, RegistryExt, TaggedRegistryExt,
    blocks::block_state_ext::BlockStateExt,
    data_components::vanilla_components::ENCHANTABLE,
    enchantment::{
        EnchantmentRef,
        table::{
            EnchantmentInstance, filter_compatible_enchantments, get_available_enchantment_results,
        },
    },
    sound_events, vanilla_blocks, vanilla_custom_stats,
    vanilla_enchantment_tags::EnchantmentTag,
    vanilla_items, vanilla_menu_types,
};
use steel_utils::{
    BlockPos, Downcast as _, DowncastType, DowncastTypeKey,
    random::{Random as _, legacy_random::LegacyRandom},
};

use crate::behavior::advancement_hooks::raise_enchanted_item;
use crate::behavior::blocks::EnchantingTableBlock;
use crate::block_entity::BlockEntity as _;
use crate::block_entity::entities::EnchantingTableBlockEntity;
use crate::inventory::prelude::*;
use crate::inventory::slots::SlotStorage;
use crate::player::player_inventory::{PlayerInventory, Prediction};
use crate::world::World;

/// Number of slots in the enchanting table container: the item and the lapis.
pub const ENCHANTMENT_SLOTS: usize = 2;

/// Number of enchantment offers the table shows.
const ENCHANTMENT_OFFERS: usize = 3;

/// Returns the enchantment seed of the table at `pos`, or 0 when the block
/// entity is missing.
fn table_seed(world: &World, pos: BlockPos) -> i32 {
    let Some(block_entity) = world.get_block_entity(pos) else {
        return 0;
    };
    let Some(table) = block_entity.downcast_ref::<EnchantingTableBlockEntity>() else {
        return 0;
    };
    table.enchantment_seed()
}

/// Stores `seed` on the table at `pos`.
///
/// Runs without any container lock held, matching the lock discipline of the
/// menu hooks that cannot reach the world.
fn store_table_seed(world: &World, pos: BlockPos, seed: i32) {
    let Some(block_entity) = world.get_block_entity(pos) else {
        return;
    };
    let Some(table) = block_entity.downcast_ref::<EnchantingTableBlockEntity>() else {
        return;
    };
    if table.enchantment_seed() == seed {
        return;
    }
    table.set_enchantment_seed(seed);
    table.set_changed();
}

/// Builds the enchanting table menu.
#[must_use]
pub fn enchantment(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    pos: BlockPos,
    world: &Arc<World>,
) -> Menu {
    let enchant_slots = SimpleContainer::new(ENCHANTMENT_SLOTS).into_shared();

    let mut builder = MenuBuilder::new(&vanilla_menu_types::ENCHANTMENT, container_id);

    let item = builder.section_at(
        &enchant_slots,
        [0],
        SectionKind::custom(|container, index| Box::new(EnchantItemSlot::new(container, index))),
    );
    let lapis = builder.section_at(
        &enchant_slots,
        [1],
        SectionKind::restricted(|_, stack| stack.is(&vanilla_items::LAPIS_LAZULI)),
    );
    let player = builder.player_inventory(&inventory);

    // Data slots in Vanilla's order: the three costs, the seed, the three
    // enchantment clues and the three level clues. The client indexes the clue
    // slots by position, so the seed slot must stay even though Steel keeps the
    // seed on the table rather than on the player.
    let enchantment_seed = table_seed(world, pos);
    let costs = from_fn(|_| builder.data_slot(0));
    let seed = builder.data_slot(truncated_seed(enchantment_seed));
    let enchant_clue = from_fn(|_| builder.data_slot(-1));
    let level_clue = from_fn(|_| builder.data_slot(-1));

    builder.drain([item, lapis]);

    builder.build(EnchantmentKind {
        enchant_slots,
        item,
        lapis,
        player,
        block_pos: pos,
        world: Arc::clone(world),
        costs,
        seed,
        enchantment_seed,
        enchant_clue,
        level_clue,
    })
}

/// Slot 0 of the enchanting table: the item being enchanted.
///
/// Vanilla limits this slot to a single item through the anonymous slot's
/// `getMaxStackSize` override; placement itself is unrestricted.
pub struct EnchantItemSlot {
    base: NormalSlot,
}

// SAFETY: This key is owned by Steel and uniquely identifies the enchanting
// table's item slot.
unsafe impl DowncastType for EnchantItemSlot {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:slot/enchanting_table_item");
}

impl EnchantItemSlot {
    /// Creates the item slot backed by `container` at `index`.
    #[must_use]
    pub fn new(container: impl Into<ContainerRef>, index: usize) -> Self {
        Self {
            base: NormalSlot::new(container, index),
        }
    }
}

impl Slot for EnchantItemSlot {
    fn storage(&self) -> &SlotStorage {
        self.base.storage()
    }

    fn get_item<'a>(&self, guard: &'a ContainerLockGuard) -> &'a ItemStack {
        self.base.get_item(guard)
    }

    fn get_item_mut<'a>(&self, guard: &'a mut ContainerLockGuard) -> &'a mut ItemStack {
        self.base.get_item_mut(guard)
    }

    fn set_item(&self, guard: &mut ContainerLockGuard, stack: ItemStack) {
        self.base.set_item(guard, stack);
    }

    fn get_max_stack_size(&self, _guard: &ContainerLockGuard) -> i32 {
        1
    }

    fn set_changed(&self, guard: &mut ContainerLockGuard) {
        self.base.set_changed(guard);
    }

    fn get_container_slot(&self) -> usize {
        self.base.get_container_slot()
    }
}

/// Per-menu enchanting table state.
pub struct EnchantmentKind {
    /// The menu-local item and lapis container.
    enchant_slots: Shared<SimpleContainer>,
    item: Section,
    lapis: Section,
    player: PlayerInventorySections,
    block_pos: BlockPos,
    world: Arc<World>,
    costs: [DataSlot; ENCHANTMENT_OFFERS],
    /// Client-facing copy of [`Self::enchantment_seed`].
    seed: DataSlot,
    /// The table's enchantment seed, cached at menu construction so no
    /// container-locked hook has to reach into the world.
    ///
    /// Vanilla keeps the seed on the player; Steel keeps it on the table (see
    /// [`EnchantingTableBlockEntity::enchantment_seed`]) and writes the rerolled
    /// value back when the menu closes.
    enchantment_seed: i32,
    enchant_clue: [DataSlot; ENCHANTMENT_OFFERS],
    level_clue: [DataSlot; ENCHANTMENT_OFFERS],
}

// SAFETY: This key is owned by Steel and uniquely identifies the enchanting
// table menu behavior.
unsafe impl DowncastType for EnchantmentKind {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:menu/enchantment");
}

impl EnchantmentKind {
    /// Recomputes the three offers and their client-visible clues.
    ///
    /// Mirrors Vanilla `EnchantmentMenu.slotsChanged`.
    fn update_offers(&mut self, behavior: &mut MenuBehavior, guard: &ContainerLockGuard) {
        let container_id = ContainerId::from_arc(&self.enchant_slots);
        let item = guard
            .get(container_id)
            .map_or_else(ItemStack::empty, |container| container.get_item(0).clone());

        let seed = self.enchantment_seed;
        self.seed.set(behavior, truncated_seed(seed));

        if item.is_empty() || !item.is_enchantable() {
            self.clear_offers(behavior);
            return;
        }

        let bookcases = EnchantingTableBlock::bookshelf_count(&*self.world, self.block_pos);

        let mut random = LegacyRandom::from_seed(0);
        random.set_seed(i64::from(seed));

        let mut costs = [0i32; ENCHANTMENT_OFFERS];
        for (slot, cost) in costs.iter_mut().enumerate() {
            let slot_index = i32::try_from(slot).unwrap_or(i32::MAX);
            let computed = Self::get_enchantment_cost(&mut random, slot_index, bookcases, &item);
            *cost = if computed < slot_index + 1 {
                0
            } else {
                computed
            };
        }

        let source = Self::table_enchantments();
        let mut enchant_clues = [-1i32; ENCHANTMENT_OFFERS];
        let mut level_clues = [-1i32; ENCHANTMENT_OFFERS];
        for (slot, cost) in costs.iter().enumerate() {
            if *cost <= 0 {
                continue;
            }
            let slot_index = i32::try_from(slot).unwrap_or(i32::MAX);
            let list =
                Self::get_enchantment_list(&mut random, seed, &item, slot_index, *cost, &source);
            if list.is_empty() {
                continue;
            }
            let bound = i32::try_from(list.len()).unwrap_or(i32::MAX);
            let index = random.next_i32_bounded(bound);
            let Ok(index) = usize::try_from(index) else {
                continue;
            };
            let Some(picked) = list.get(index) else {
                continue;
            };
            enchant_clues[slot] = REGISTRY
                .enchantments
                .id_from_key(&picked.enchantment.key)
                .and_then(|id| i32::try_from(id).ok())
                .unwrap_or(-1);
            level_clues[slot] = picked.level;
        }

        for slot in 0..ENCHANTMENT_OFFERS {
            self.costs[slot].set(behavior, i16::try_from(costs[slot]).unwrap_or(i16::MAX));
            self.enchant_clue[slot].set(behavior, i16::try_from(enchant_clues[slot]).unwrap_or(-1));
            self.level_clue[slot].set(behavior, i16::try_from(level_clues[slot]).unwrap_or(-1));
        }
    }

    /// Clears the offers and clues, as Vanilla does for a missing or
    /// unenchantable item.
    fn clear_offers(&mut self, behavior: &mut MenuBehavior) {
        for slot in 0..ENCHANTMENT_OFFERS {
            self.costs[slot].set(behavior, 0);
            self.enchant_clue[slot].set(behavior, -1);
            self.level_clue[slot].set(behavior, -1);
        }
    }

    /// Every enchantment in `#minecraft:in_enchanting_table`, in tag order.
    fn table_enchantments() -> Vec<EnchantmentRef> {
        REGISTRY
            .enchantments
            .get_tag(&EnchantmentTag::IN_ENCHANTING_TABLE)
            .unwrap_or_default()
    }

    /// Rolls the enchantments one offer grants.
    ///
    /// Mirrors Vanilla `EnchantmentMenu.getEnchantmentList`, including the
    /// per-slot reseed and the single-enchantment book rule.
    fn get_enchantment_list(
        random: &mut LegacyRandom,
        seed: i32,
        item: &ItemStack,
        slot: i32,
        enchantment_cost: i32,
        source: &[EnchantmentRef],
    ) -> Vec<EnchantmentInstance> {
        random.set_seed(i64::from(seed.wrapping_add(slot)));
        let mut list = Self::select_enchantment(random, item, enchantment_cost, source);
        if item.is(&vanilla_items::BOOK) && list.len() > 1 {
            let bound = i32::try_from(list.len()).unwrap_or(i32::MAX);
            let index = random.next_i32_bounded(bound);
            if let Ok(index) = usize::try_from(index) {
                list.remove(index);
            }
        }
        list
    }

    /// Vanilla `EnchantmentHelper.getEnchantmentCost`, drawn through Java's
    /// `RandomSource.nextInt`.
    fn get_enchantment_cost(
        random: &mut LegacyRandom,
        slot: i32,
        bookcases: i32,
        item: &ItemStack,
    ) -> i32 {
        if item.get(ENCHANTABLE).is_none() {
            return 0;
        }

        let bookcases = bookcases.min(15);
        let selected = random.next_i32_bounded(8)
            + 1
            + (bookcases >> 1)
            + random.next_i32_bounded(bookcases + 1);

        match slot {
            0 => (selected / 3).max(1),
            1 => selected * 2 / 3 + 1,
            _ => selected.max(bookcases * 2),
        }
    }

    /// Vanilla `EnchantmentHelper.selectEnchantment`, drawn through Java's
    /// `RandomSource`.
    #[expect(
        clippy::cast_precision_loss,
        reason = "vanilla performs the enchanting cost maths in Java float precision"
    )]
    fn select_enchantment(
        random: &mut LegacyRandom,
        item: &ItemStack,
        enchantment_cost: i32,
        source: &[EnchantmentRef],
    ) -> Vec<EnchantmentInstance> {
        let mut results = Vec::new();
        let Some(enchantable) = item.get(ENCHANTABLE) else {
            return results;
        };

        let enchantability = enchantable.value();
        let mut enchantment_cost = enchantment_cost
            + 1
            + random.next_i32_bounded(enchantability / 4 + 1)
            + random.next_i32_bounded(enchantability / 4 + 1);
        let random_span = (random.next_f32() + random.next_f32() - 1.0) * 0.15;
        let scaled = enchantment_cost as f32 + enchantment_cost as f32 * random_span;
        enchantment_cost = java_round(scaled).clamp(1, i32::MAX);

        let mut enchantments = get_available_enchantment_results(enchantment_cost, item, source);
        if enchantments.is_empty() {
            return results;
        }

        if let Some(picked) = Self::weighted_random_item(random, &enchantments) {
            results.push(picked);
        }

        while random.next_i32_bounded(50) <= enchantment_cost {
            if let Some(last) = results.last() {
                filter_compatible_enchantments(&mut enchantments, last);
            }

            if enchantments.is_empty() {
                break;
            }

            if let Some(picked) = Self::weighted_random_item(random, &enchantments) {
                results.push(picked);
            }
            enchantment_cost /= 2;
        }

        results
    }

    /// Vanilla `WeightedRandom.getRandomItem`, drawn through Java's
    /// `RandomSource.nextInt`.
    fn weighted_random_item(
        random: &mut LegacyRandom,
        items: &[EnchantmentInstance],
    ) -> Option<EnchantmentInstance> {
        let total_weight: i64 = items.iter().map(|item| i64::from(item.weight())).sum();
        let Ok(total_weight) = i32::try_from(total_weight) else {
            return None;
        };
        if total_weight <= 0 {
            return None;
        }

        let mut selection = i64::from(random.next_i32_bounded(total_weight));
        for item in items {
            selection -= i64::from(item.weight());
            if selection < 0 {
                return Some(*item);
            }
        }

        None
    }
}

impl MenuKind for EnchantmentKind {
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        self.world.get_block_state(self.block_pos).get_block() == &vanilla_blocks::ENCHANTING_TABLE
            && player.is_within_block_interaction_range_with_buffer(self.block_pos, 4.0)
    }

    fn on_open(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_offers(behavior, guard);
    }

    fn slots_changed(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        self.update_offers(behavior, guard);
    }

    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        store_table_seed(&self.world, self.block_pos, self.enchantment_seed);
    }

    fn on_button_clicked(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        button_id: i32,
        player: &Player,
    ) -> bool {
        let Ok(slot) = usize::try_from(button_id) else {
            return false;
        };
        if slot >= ENCHANTMENT_OFFERS {
            return false;
        }

        let container_id = ContainerId::from_arc(&self.enchant_slots);
        let Some(container) = guard.get(container_id) else {
            return false;
        };
        let item = container.get_item(0).clone();
        let currency = container.get_item(1).clone();

        let enchantment_cost = button_id + 1;
        let infinite_materials = player.has_infinite_materials();
        if (currency.is_empty() || currency.count() < enchantment_cost) && !infinite_materials {
            return false;
        }

        let cost = i32::from(self.costs[slot].get(behavior));
        let experience_level = player.experience.lock().level();
        if cost <= 0
            || item.is_empty()
            || ((experience_level < enchantment_cost || experience_level < cost)
                && !infinite_materials)
        {
            return false;
        }

        let seed = self.enchantment_seed;
        let source = Self::table_enchantments();
        let mut random = LegacyRandom::from_seed(0);
        let enchantments =
            Self::get_enchantment_list(&mut random, seed, &item, button_id, cost, &source);
        if enchantments.is_empty() {
            // Vanilla accepts the click but applies nothing when no offer fits.
            return true;
        }

        player.give_experience_levels(-enchantment_cost);

        let mut enchanted = item;
        if enchanted.is(&vanilla_items::BOOK) {
            enchanted.set_item(&vanilla_items::ENCHANTED_BOOK.key);
        }
        for instance in &enchantments {
            enchanted.upgrade_enchantment(
                instance.enchantment.key.clone(),
                u32::try_from(instance.level).unwrap_or(1),
            );
        }

        let mut currency = currency;
        currency.consume(enchantment_cost, infinite_materials);

        let Some(container) = guard.get_mut(container_id) else {
            return false;
        };
        // Snapshot the enchanted stack so the raise happens after the container
        // borrow ends; `run_unlocked` releases the menu containers first because
        // the raise takes the player's equipment (inventory) lock.
        let enchanted_item = enchanted.clone();
        container.set_item(0, enchanted);
        container.set_item(
            1,
            if currency.is_empty() {
                ItemStack::empty()
            } else {
                currency
            },
        );

        player.award_custom_stat(&vanilla_custom_stats::ENCHANT_ITEM);

        // Vanilla `EnchantmentMenu.clickMenuButton` raises `enchanted_item` with
        // the enchanted stack and the offer's level cost right after the stat.
        guard.run_unlocked(|| {
            raise_enchanted_item(player, &enchanted_item, enchantment_cost);
        });

        // Vanilla rerolls the seed in `Player.onEnchantmentPerformed`, so the
        // next offers differ; the table receives it when the menu closes.
        self.enchantment_seed = rand::random();
        self.update_offers(behavior, guard);

        self.world.play_sound(
            &sound_events::BLOCK_ENCHANTMENT_TABLE_USE,
            SoundSource::Blocks,
            self.block_pos,
            1.0,
            0.9 + rand::random::<f32>() * 0.1,
            None,
        );

        true
    }

    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> Option<ItemStack> {
        if slot_index >= behavior.slots().len() {
            return Some(ItemStack::empty());
        }
        let clicked = behavior.slots()[slot_index].get_item(guard).clone();
        if clicked.is_empty() {
            return Some(ItemStack::empty());
        }
        let mut remaining = clicked.clone();

        let moved = if self.item.contains(slot_index) || self.lapis.contains(slot_index) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.player.all().start(),
                self.player.all().end(),
                FillDirection::Backward,
            )
        } else if clicked.is(&vanilla_items::LAPIS_LAZULI) {
            behavior.move_item_stack_to(
                guard,
                slot_index,
                &mut remaining,
                self.lapis.start(),
                self.lapis.end(),
                FillDirection::Forward,
            )
        } else {
            let item_slot = self.item.start();
            if behavior.slots()[item_slot].has_item(guard)
                || !behavior.slots()[item_slot].may_place(&clicked)
            {
                return Some(ItemStack::empty());
            }
            // Vanilla moves a single item into the enchant slot per click.
            let single = clicked.copy_with_count(1);
            remaining.shrink(1);
            behavior.slots()[item_slot].set_by_player(guard, single, &ItemStack::empty());
            true
        };

        if !moved {
            return Some(ItemStack::empty());
        }
        behavior.update_quick_move_source(guard, slot_index, &remaining, &clicked);
        if remaining.count() == clicked.count() {
            return Some(ItemStack::empty());
        }
        if let Some(remainder) = behavior.slots()[slot_index].on_take(guard, &remaining, player) {
            player.add_item_or_drop_with_guard(guard, remainder, Prediction::Predicted);
        }
        Some(clicked)
    }
}

/// Truncates a 32-bit seed to the `short` Vanilla sends through a data slot.
const fn truncated_seed(seed: i32) -> i16 {
    let bytes = seed.to_le_bytes();
    i16::from_le_bytes([bytes[0], bytes[1]])
}

/// Vanilla `Math.round(float)`: `floor(value + 0.5)` narrowed to `int`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "vanilla narrows the rounded float to a Java int"
)]
fn java_round(value: f32) -> i32 {
    (value + 0.5).floor() as i32
}
