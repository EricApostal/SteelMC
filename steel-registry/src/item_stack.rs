//! Item stack implementation.

use std::{
    borrow::Cow,
    io::{Cursor, Result, Write},
};

use rand::RngExt;

use steel_utils::{
    DowncastType, Identifier,
    codec::VarInt,
    java,
    random::{Random, xoroshiro::Xoroshiro},
    serial::{ReadFrom, WriteTo},
};
use text_components::TextComponent;

use crate::{
    REGISTRY, RegistryEntry, RegistryExt,
    damage_type::DamageTypeRef,
    data_components::{
        Component, ComponentData, ComponentPatchEntry, CustomData, DataComponentMap,
        DataComponentPatch, DataComponentType,
        vanilla_components::{
            ATTACK_RANGE, ATTRIBUTE_MODIFIERS, AttackRange, BUNDLE_CONTENTS, CHARGED_PROJECTILES,
            CONTAINER, CUSTOM_DATA, CUSTOM_NAME, DAMAGE, DAMAGE_RESISTANT, DAMAGE_TYPE,
            ENCHANTABLE, ENCHANTMENTS, EQUIPPABLE, Equippable, ITEM_NAME, ItemAttributeModifiers,
            ItemEnchantments, MAX_DAMAGE, MAX_STACK_SIZE, MINIMUM_ATTACK_CHARGE,
            OMINOUS_BOTTLE_AMPLIFIER, OminousBottleAmplifier, PIERCING_WEAPON, PiercingWeapon,
            REPAIRABLE, STORED_ENCHANTMENTS, TOOL, Tool, UNBREAKABLE, WEAPON, WRITTEN_BOOK_CONTENT,
            Weapon,
        },
    },
    enchantment::EnchantmentRef,
    enchantment::table::EnchantmentRandom as _,
    enchantment_effect::EnchantmentEffectComponent,
    equipment::EquipmentSlot,
    item_stack_template::ItemStackTemplate,
    items::{Item, ItemRef},
    vanilla_items,
};

/// A stack of items with a count and component modifications.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemStack {
    /// The item type. AIR represents an empty stack.
    pub item: ItemRef,
    /// The number of items in this stack.
    pub count: i32,
    /// Modifications to the prototype components.
    patch: DataComponentPatch,
}

impl Default for ItemStack {
    fn default() -> Self {
        Self::empty()
    }
}

impl ItemStack {
    /// Creates an empty item stack (using AIR).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            item: &vanilla_items::AIR,
            count: 0,
            patch: DataComponentPatch::new(),
        }
    }

    /// Creates a new item stack with count 1.
    #[must_use]
    pub fn new(item: ItemRef) -> Self {
        Self::with_count(item, 1)
    }

    /// Creates a new item stack with the specified count.
    #[must_use]
    pub fn with_count(item: ItemRef, count: i32) -> Self {
        Self {
            item,
            count,
            patch: DataComponentPatch::new(),
        }
    }

    /// Creates a new item stack with the specified count and component patch.
    #[must_use]
    pub fn with_count_and_patch(item: ItemRef, count: i32, mut patch: DataComponentPatch) -> Self {
        patch.sanitize_against(&item.components);
        Self { item, count, patch }
    }

    #[must_use]
    const fn prototype(&self) -> &'static DataComponentMap {
        &self.item.components
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.item == &*vanilla_items::AIR || self.count <= 0
    }

    #[must_use]
    pub fn item(&self) -> ItemRef {
        if self.is_empty() {
            &vanilla_items::AIR
        } else {
            self.item
        }
    }

    #[must_use]
    pub fn count(&self) -> i32 {
        if self.is_empty() { 0 } else { self.count }
    }

    /// Returns the component patch. Empty stacks have an empty patch.
    #[must_use]
    pub fn components_patch(&self) -> &DataComponentPatch {
        if self.is_empty() {
            DataComponentPatch::empty()
        } else {
            &self.patch
        }
    }

    pub const fn set_count(&mut self, count: i32) {
        self.count = count;
    }

    /// Increases the count by the given amount.
    pub const fn grow(&mut self, amount: i32) {
        self.count += amount;
    }

    /// Decreases the count by the given amount.
    pub const fn shrink(&mut self, amount: i32) {
        self.count -= amount;
    }

    /// Decreases the count by one.
    pub const fn shrink_one(&mut self) {
        self.count -= 1;
    }

    /// Decreases the count by the given amount, unless the user has infinite
    /// materials.
    ///
    /// See: `Player::has_infinite_materials`
    #[inline]
    pub const fn consume(&mut self, amount: i32, has_infinite_materials: bool) {
        if !has_infinite_materials {
            self.shrink(amount);
        }
    }

    /// [Consumes](Self::consume) a single item from this stack.
    #[inline]
    pub const fn consume_one(&mut self, has_infinite_materials: bool) {
        self.consume(1, has_infinite_materials);
    }

    /// Splits off the given amount and [consumes](Self::consume) it from this stack.
    #[inline]
    #[must_use]
    pub fn consume_and_return(&mut self, amount: i32, has_infinite_materials: bool) -> Self {
        let split = self.copy_with_count(amount);
        self.consume(amount, has_infinite_materials);
        split
    }

    /// Splits off the specified amount from this stack and returns it as a new stack.
    ///
    /// If the amount is greater than or equal to the current count, this stack becomes
    /// empty and the entire contents are returned.
    pub fn split(&mut self, amount: i32) -> Self {
        let take = amount.min(self.count);
        let result = Self {
            item: self.item,
            count: take,
            patch: self.patch.clone(),
        };
        self.shrink(take);
        result
    }

    /// Copies the identity (item type and patch) from another stack.
    ///
    /// Used when splitting stacks to preserve components.
    #[must_use]
    pub fn copy_with_count(&self, count: i32) -> Self {
        if self.is_empty() {
            Self::empty()
        } else {
            Self {
                item: self.item,
                count,
                patch: self.patch.clone(),
            }
        }
    }

    /// Returns true if this item can stack (max stack size > 1 and not damaged).
    /// Damaged items cannot stack.
    #[must_use]
    pub fn is_stackable(&self) -> bool {
        self.max_stack_size() > 1 && (!self.is_damageable_item() || !self.is_damaged())
    }

    /// Returns true if this item can take damage.
    #[must_use]
    pub fn is_damageable_item(&self) -> bool {
        self.has(MAX_DAMAGE) && !self.has(UNBREAKABLE) && self.has(DAMAGE)
    }

    /// Returns true if this item has taken damage.
    #[must_use]
    pub fn is_damaged(&self) -> bool {
        self.is_damageable_item() && self.get_damage_value() > 0
    }

    /// Gets the current damage value of this item.
    #[must_use]
    pub fn get_damage_value(&self) -> i32 {
        self.get(DAMAGE)
            .copied()
            .unwrap_or(0)
            .clamp(0, self.get_max_damage())
    }

    /// Sets the damage value of this item.
    pub fn set_damage_value(&mut self, value: i32) {
        let clamped = value.clamp(0, self.get_max_damage());
        self.set(DAMAGE, clamped);
    }

    /// Gets the maximum damage this item can take before breaking.
    #[must_use]
    pub fn get_max_damage(&self) -> i32 {
        self.get(MAX_DAMAGE).copied().unwrap_or(0)
    }

    /// Returns true if the item is broken (damage >= max damage).
    #[must_use]
    pub fn is_broken(&self) -> bool {
        self.is_damageable_item() && self.get_damage_value() >= self.get_max_damage()
    }

    #[must_use]
    pub fn next_damage_will_break(&self) -> bool {
        self.is_damageable_item() && self.get_damage_value() >= self.get_max_damage() - 1
    }

    /// Damages the item and breaks it if durability reaches zero.
    ///
    /// Returns `true` if the item broke and should be removed/replaced.
    pub fn hurt_and_break(&mut self, amount: i32, has_infinite_materials: bool) -> bool {
        let mut random = Xoroshiro::from_seed_unmixed(rand::rng().random());
        self.hurt_and_break_with_random(amount, has_infinite_materials, &mut random)
    }

    /// Damages the item using the supplied random source for data-driven
    /// `minecraft:item_damage` enchantment effects.
    ///
    /// Returns `true` if the item broke and should be removed/replaced.
    pub fn hurt_and_break_with_random(
        &mut self,
        amount: i32,
        has_infinite_materials: bool,
        random: &mut impl Random,
    ) -> bool {
        if !self.is_damageable_item() {
            return false;
        }

        if has_infinite_materials {
            return false;
        }

        let effective_amount = self.process_durability_change(amount, random);

        if effective_amount == 0 {
            return false;
        }

        let new_damage = self.get_damage_value() + effective_amount;

        // TODO: Trigger ITEM_DURABILITY_CHANGED advancement criteria

        self.set_damage_value(new_damage);

        if self.is_broken() {
            // TODO: Call onEquippedItemBroken callback which:
            // - Broadcasts entity event (byte 47 for mainhand) for break sound/particles
            // - Stops location-based effects (removes attribute modifiers)
            self.shrink_one();
            return true;
        }

        false
    }

    fn process_durability_change(&self, amount: i32, random: &mut impl Random) -> i32 {
        if amount <= 0 {
            return amount;
        }

        let Some(enchantments) = self.get_enchantments() else {
            return amount;
        };

        let mut value = amount as f32;
        for (key, level) in enchantments.iter() {
            if *level == 0 {
                continue;
            }
            let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
                continue;
            };

            for effect in enchantment.effects.item_damage {
                if effect.requirements.is_some_and(|requirements| {
                    requirements.matches_item_context(self.item()) != Some(true)
                }) {
                    continue;
                }
                value = effect
                    .effect
                    .process_with_random(*level as i32, random, value);
            }
        }

        value as i32
    }

    /// Returns true if this item has the specified component (by type).
    #[must_use]
    pub fn has<T: 'static>(&self, component: DataComponentType<T>) -> bool {
        self.has_component(&component.key)
    }

    /// Returns true if this item has the specified component (by key).
    ///
    /// Empty stacks have no components.
    #[must_use]
    pub fn has_component(&self, key: &Identifier) -> bool {
        if self.is_empty() {
            return false;
        }
        match self.patch.get_entry(key) {
            Some(ComponentPatchEntry::Set(_)) => true,
            Some(ComponentPatchEntry::Removed) => false,
            None => self.prototype().get_raw(key).is_some(),
        }
    }

    #[must_use]
    pub fn is_same_item(a: &Self, b: &Self) -> bool {
        a.item().key == b.item().key
    }

    /// Checks if two stacks have the same item and components.
    #[must_use]
    pub fn is_same_item_same_components(a: &Self, b: &Self) -> bool {
        if !Self::is_same_item(a, b) {
            return false;
        }
        if a.is_empty() && b.is_empty() {
            return true;
        }
        a.components_equal(b)
    }

    #[must_use]
    pub fn matches(a: &Self, b: &Self) -> bool {
        a.count() == b.count() && Self::is_same_item_same_components(a, b)
    }

    #[must_use]
    pub fn is(&self, item: ItemRef) -> bool {
        self.item().key == item.key
    }

    #[must_use]
    pub fn max_stack_size(&self) -> i32 {
        self.get(MAX_STACK_SIZE).copied().unwrap_or(1)
    }

    /// Validates the complete stack constraints enforced by Vanilla's `ItemStack.validateStrict`.
    pub fn validate_strict(&self) -> Result<()> {
        let max_stack_size = self.max_stack_size();
        if self.has(MAX_DAMAGE) && max_stack_size > 1 {
            return Err(std::io::Error::other(
                "Item cannot be both damageable and stackable",
            ));
        }

        if let Some(container) = self.get(CONTAINER) {
            validate_contained_item_sizes(container.items().iter().flatten())?;
        }

        if let Some(bundle) = self.get(BUNDLE_CONTENTS) {
            validate_contained_item_sizes(bundle.items())?;
            bundle.validate_weight()?;
        }

        if let Some(projectiles) = self.get(CHARGED_PROJECTILES) {
            validate_contained_item_sizes(projectiles.items())?;
        }

        if self.count > max_stack_size {
            return Err(std::io::Error::other(format!(
                "Item stack with stack size of {} was larger than maximum: {max_stack_size}",
                self.count
            )));
        }
        Ok(())
    }

    /// Returns the equippable component if this item has one.
    #[must_use]
    pub fn get_equippable(&self) -> Option<&Equippable> {
        self.get(EQUIPPABLE)
    }

    /// Returns the item attribute modifiers component.
    #[must_use]
    pub fn get_attribute_modifiers(&self) -> Option<&ItemAttributeModifiers> {
        self.get(ATTRIBUTE_MODIFIERS)
    }

    /// Returns the equipment slot this item can be equipped to, if any.
    #[must_use]
    pub fn get_equippable_slot(&self) -> Option<EquipmentSlot> {
        self.get_equippable().map(|e| e.slot)
    }

    /// Returns true if this item can be equipped in the given slot.
    #[must_use]
    pub fn is_equippable_in_slot(&self, slot: EquipmentSlot) -> bool {
        self.get_equippable_slot() == Some(slot)
    }

    /// Gets the raw component data by key. Empty stacks have no components.
    #[must_use]
    pub fn get_effective_value_raw(&self, key: &Identifier) -> Option<&ComponentData> {
        if self.is_empty() {
            return None;
        }
        match self.patch.get_entry(key) {
            Some(ComponentPatchEntry::Set(data)) => Some(data),
            Some(ComponentPatchEntry::Removed) => None,
            None => self.prototype().get_raw(key),
        }
    }

    /// Gets the effective value of a component, considering the patch and prototype.
    /// Returns `None` if the component is not present or has been removed.
    #[must_use]
    pub fn get<T: Component + DowncastType>(&self, component: DataComponentType<T>) -> Option<&T> {
        let data = self.get_effective_value_raw(&component.key)?;
        data.downcast_ref::<T>()
    }

    /// Gets the effective value of a component, or returns the default value if not present.
    #[must_use]
    pub fn get_or_default<T: Component + DowncastType + Clone>(
        &self,
        component: DataComponentType<T>,
        default: T,
    ) -> T {
        self.get(component).cloned().unwrap_or(default)
    }

    /// Sets a component value in this item's patch, overriding the prototype.
    pub fn set<T: Component + DowncastType>(&mut self, component: DataComponentType<T>, value: T) {
        let value = ComponentData::new(value);
        let is_default = self.prototype().get_raw(&component.key) == Some(&value);
        if is_default {
            self.patch.clear(component);
        } else {
            self.patch.set_component_data(component.key, value);
        }
    }

    /// Removes a component from this item (marks it as removed in the patch).
    /// This will hide the component even if it exists in the prototype.
    pub fn remove<T: 'static>(&mut self, component: DataComponentType<T>) {
        if self.prototype().get_raw(&component.key).is_some() {
            self.patch.remove(component);
        } else {
            self.patch.clear(component);
        }
    }

    /// Clears any patch entry for this component (neither set nor removed).
    /// The prototype value will be visible again.
    pub fn clear<T: 'static>(&mut self, component: DataComponentType<T>) {
        self.patch.clear(component);
    }

    /// Returns a reference to the component patch. Empty stacks have an empty patch.
    #[must_use]
    pub fn patch(&self) -> &DataComponentPatch {
        self.components_patch()
    }

    /// Gets the Tool component if present.
    #[must_use]
    pub fn get_tool(&self) -> Option<&Tool> {
        self.get(TOOL)
    }

    /// Gets the Weapon component if present.
    #[must_use]
    pub fn get_weapon(&self) -> Option<&Weapon> {
        self.get(WEAPON)
    }

    /// Gets the `AttackRange` component if present.
    #[must_use]
    pub fn get_attack_range(&self) -> Option<&AttackRange> {
        self.get(ATTACK_RANGE)
    }

    /// Returns vanilla `DataComponents.MINIMUM_ATTACK_CHARGE`, defaulting to 0.
    #[must_use]
    pub fn minimum_attack_charge(&self) -> f32 {
        self.get(MINIMUM_ATTACK_CHARGE).copied().unwrap_or(0.0)
    }

    /// Gets the vanilla damage type component if present.
    #[must_use]
    pub fn get_damage_type(&self) -> Option<DamageTypeRef> {
        self.get(DAMAGE_TYPE).map(|component| component.damage_type)
    }

    /// Returns vanilla `ItemStack.canBeHurtBy` for a damage type.
    #[must_use]
    pub fn can_be_hurt_by(&self, damage_type: DamageTypeRef) -> bool {
        self.get(DAMAGE_RESISTANT)
            .is_none_or(|resistance| !resistance.is_resistant_to(damage_type))
    }

    #[must_use]
    pub fn is_valid_repair_item(&self, repair_item: &Self) -> bool {
        self.get(REPAIRABLE)
            .is_some_and(|repairable| repairable.is_valid_repair_item(repair_item))
    }

    /// Returns whether this item has the vanilla piercing weapon component.
    #[must_use]
    pub fn is_piercing_weapon(&self) -> bool {
        self.has(PIERCING_WEAPON)
    }

    /// Gets the `PiercingWeapon` component if present.
    #[must_use]
    pub fn get_piercing_weapon(&self) -> Option<&PiercingWeapon> {
        self.get(PIERCING_WEAPON)
    }

    /// Returns the mining speed for the given block state ID.
    /// If no Tool component is present, returns 1.0 (hand speed).
    #[must_use]
    pub fn get_destroy_speed(&self, block_state_id: steel_utils::BlockStateId) -> f32 {
        self.get_tool()
            .map_or(1.0, |tool| tool.get_mining_speed(block_state_id))
    }

    /// Returns true if this tool is correct for getting drops from the block.
    #[must_use]
    pub fn is_correct_tool_for_drops(&self, block_state_id: steel_utils::BlockStateId) -> bool {
        self.get_tool()
            .is_some_and(|tool| tool.is_correct_for_drops(block_state_id))
    }

    /// Returns the damage per block for this tool (how much durability is consumed per block mined).
    /// Returns 0 if no Tool component is present.
    #[must_use]
    pub fn get_tool_damage_per_block(&self) -> i32 {
        self.get_tool().map_or(0, |tool| tool.damage_per_block)
    }

    /// Returns true if this tool can destroy blocks in creative mode.
    /// Returns true if no Tool component is present (default behavior).
    #[must_use]
    pub fn can_destroy_blocks_in_creative(&self) -> bool {
        self.get_tool()
            .is_none_or(|tool| tool.can_destroy_blocks_in_creative)
    }

    #[must_use]
    pub fn get_enchantment_level(&self, enchantment: &Identifier) -> i32 {
        self.get_enchantments()
            .map_or(0, |e| e.get_level(enchantment) as i32)
    }

    #[must_use]
    pub fn get_enchantments(&self) -> Option<&ItemEnchantments> {
        self.get(ENCHANTMENTS)
    }

    /// Vanilla `EnchantmentHelper.getEnchantmentsForCrafting`: enchanted books
    /// expose `STORED_ENCHANTMENTS` to crafting operations, while every other
    /// item exposes `ENCHANTMENTS`.
    #[must_use]
    pub fn get_enchantments_for_crafting(&self) -> Option<&ItemEnchantments> {
        self.get(self.enchantment_component())
    }

    #[must_use]
    fn enchantment_component(&self) -> DataComponentType<ItemEnchantments> {
        if self.is(&vanilla_items::ENCHANTED_BOOK) {
            STORED_ENCHANTMENTS
        } else {
            ENCHANTMENTS
        }
    }

    /// Mirrors Vanilla's component-based `ItemStack.isEnchantable` check.
    #[must_use]
    pub fn is_enchantable(&self) -> bool {
        self.has(ENCHANTABLE)
            && self
                .get(ENCHANTMENTS)
                .is_some_and(ItemEnchantments::is_empty)
    }

    /// Checks if `ItemStack` has an enchantment, not including enchanted books
    #[must_use]
    pub fn is_enchanted(&self) -> bool {
        self.get_enchantments().is_some_and(|e| !e.is_empty())
    }

    #[must_use]
    pub fn has_enchantment_effect(&self, component: EnchantmentEffectComponent) -> bool {
        let Some(enchantments) = self.get_enchantments() else {
            return false;
        };

        for (key, level) in enchantments.iter() {
            if *level == 0 {
                continue;
            }
            let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
                continue;
            };
            if enchantment.effects.has(component) {
                return true;
            }
        }

        false
    }

    #[must_use]
    pub fn apply_unconditional_enchantment_value_effects(
        &self,
        component: EnchantmentEffectComponent,
        input: f32,
    ) -> f32 {
        let Some(enchantments) = self.get_enchantments() else {
            return input;
        };

        let mut value = input;
        for (key, level) in enchantments.iter() {
            if *level == 0 {
                continue;
            }
            let Some(enchantment) = REGISTRY.enchantments.by_key(key) else {
                continue;
            };
            let level = *level as i32;

            for effect in enchantment.effects.value_effects(component) {
                if !effect.is_unconditional() {
                    continue;
                }
                if let Some(updated) = effect.effect.process_without_random(level, value) {
                    value = updated;
                }
            }

            let Some(effect) = enchantment.effects.single_value_effect(component) else {
                continue;
            };
            if let Some(updated) = effect.process_without_random(level, value) {
                value = updated;
            }
        }

        value
    }

    /// Sets the damage/durability as a fraction (0.0 = broken, 1.0 = full).
    /// If `add` is true, adds to current damage instead of setting.
    ///
    /// Vanilla `SetItemDamageFunction.run`: the fraction is a durability
    /// percentage, so the stored damage is `floor((1 - fraction) * maxDamage)`.
    pub fn set_damage_fraction(&mut self, fraction: f32, add: bool) {
        if !self.is_damageable_item() {
            log::warn!("Couldn't set damage of loot item {self}");
            return;
        }
        let max_damage = self.get_max_damage();
        let base = if add {
            1.0 - self.get_damage_value() as f32 / max_damage as f32
        } else {
            0.0
        };
        let pct = 1.0 - (fraction + base).clamp(0.0, 1.0);
        self.set_damage_value((pct * max_damage as f32).floor() as i32);
    }

    /// Vanilla `EnchantRandomlyFunction.run` / `enchantItem`:
    /// picks a uniform random enchantment from `options` (books ignore the
    /// `only_compatible` filter), rolls a level in
    /// `[getMinLevel(), getMaxLevel()]` and applies it with `ItemStack.enchant`.
    pub fn enchant_randomly<R: rand::Rng>(
        &mut self,
        options: &crate::loot_table::EnchantmentOptions,
        only_compatible: bool,
        include_additional_cost_component: bool,
        rng: &mut R,
    ) {
        let mut candidates: Vec<EnchantmentRef> =
            crate::enchantment::table::enchantments_in_options(options);
        if !self.is(&crate::vanilla_items::BOOK) && only_compatible {
            candidates.retain(|enchantment| enchantment.can_enchant_stack(self));
        }

        if candidates.is_empty() {
            // Vanilla `EnchantRandomlyFunction` logs instead of enchanting here.
            return;
        }

        let mut random = crate::enchantment::table::RandRngAdapter(rng);
        let index = random.next_int(candidates.len() as i32) as usize;
        let enchantment = candidates[index];
        let level = crate::enchantment::table::random_enchantment_level(&mut random, enchantment);

        if self.is(&crate::vanilla_items::BOOK) {
            self.set_item(&crate::vanilla_items::ENCHANTED_BOOK.key);
        }
        self.upgrade_enchantment(enchantment.key.clone(), level as u32);

        if include_additional_cost_component {
            crate::enchantment::table::apply_additional_trade_cost(self, level, &mut random);
        }
    }

    /// Vanilla `EnchantWithLevelsFunction.run` /
    /// `EnchantmentHelper.enchantItem(random, itemStack, levels, options)`:
    /// applies `selectEnchantment` results (empty when the item is not
    /// enchantable) and, when requested, the additional-cost component
    /// (`include_additional_cost_component`).
    ///
    /// **Deviation from vanilla:** Steel's `LootContext` does not model
    /// `LootContextParams.ADDITIONAL_COST_COMPONENT_ALLOWED`, and `NumberProvider`
    /// trade data always allows the modified villager cost when the loot entry
    /// requests the component, so the write happens unconditionally here.
    pub fn enchant_with_levels<R: rand::Rng>(
        &mut self,
        level: i32,
        options: &crate::loot_table::EnchantmentOptions,
        include_additional_cost_component: bool,
        rng: &mut R,
    ) {
        let source = crate::enchantment::table::enchantments_in_options(options);
        *self = crate::enchantment::table::enchant_item(
            &mut crate::enchantment::table::RandRngAdapter(rng),
            self.clone(),
            level,
            &source,
        );

        if include_additional_cost_component && !self.is_empty() && level > 0 {
            self.set(
                crate::data_components::vanilla_components::ADDITIONAL_TRADE_COST,
                level,
            );
        }
    }

    /// Copies components from a source (block entity, attacker, etc.) to this item.
    ///
    /// Vanilla `CopyComponentsFunction.run`: an empty `include` list copies every
    /// component of the source, otherwise only the listed keys are copied.
    pub fn copy_components<R: rand::Rng>(
        &mut self,
        source: crate::loot_table::CopySource,
        include: &[Identifier],
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::loot_table::LootContextEntity;

        let Some(hooks) = ctx.item_source else {
            return;
        };
        let components = match source {
            crate::loot_table::CopySource::BlockEntity => hooks.block_entity_components(),
            crate::loot_table::CopySource::This => hooks.entity_components(LootContextEntity::This),
            crate::loot_table::CopySource::Attacker => {
                hooks.entity_components(LootContextEntity::Killer)
            }
            crate::loot_table::CopySource::DirectAttacker => {
                hooks.entity_components(LootContextEntity::DirectKiller)
            }
        };
        let Some(components) = components else {
            return;
        };

        let keys: Vec<Identifier> = if include.is_empty() {
            components.keys().cloned().collect()
        } else {
            include.to_vec()
        };
        for key in keys {
            if let Some(data) = components.get_raw(&key) {
                self.patch.set_component_data(key, data.clone());
            }
        }
        self.patch.sanitize_against(&self.item.components);
    }

    /// Copies block state properties to this item (for blocks like `note_block`).
    ///
    /// Vanilla `CopyBlockState.run`: only properties declared by `block` and
    /// present on the looted state are copied.
    pub fn copy_block_state<R: rand::Rng>(
        &mut self,
        block: &Identifier,
        properties: &[&str],
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::data_components::vanilla_components::{BLOCK_STATE, BlockItemStateProperties};

        let Some(state) = ctx.block_state else {
            return;
        };
        let Some(block_ref) = REGISTRY.blocks.by_key(block) else {
            return;
        };
        let mut item_state = self
            .get(BLOCK_STATE)
            .cloned()
            .unwrap_or_else(BlockItemStateProperties::empty);
        for name in properties {
            if !block_ref
                .properties
                .iter()
                .any(|property| property.get_name() == *name)
            {
                continue;
            }
            let Some(value) = REGISTRY.blocks.get_property_str(state, name) else {
                continue;
            };
            item_state = item_state.with_property((*name).to_owned(), value.to_owned());
        }
        if !item_state.is_empty() {
            self.set(BLOCK_STATE, item_state);
        }
    }

    /// Sets components from a JSON string representation.
    ///
    /// Vanilla `SetComponentsFunction`: the JSON payload is a
    /// `DataComponentPatch` decoded through `JsonOps`, then applied and
    /// validated against the stack's constraints.
    pub fn set_components_from_json(&mut self, components: &str) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(components) else {
            return;
        };
        let Ok(tag) = crate::structure::processor::json_to_nbt_tag(&value) else {
            return;
        };
        let Some(patch) = DataComponentPatch::from_nbt_tag(tag.as_tag()) else {
            return;
        };
        let mut applied = self.patch.clone();
        applied.apply(&patch);
        let candidate = Self::with_count_and_patch(self.item, self.count, applied);
        if candidate.validate_strict().is_ok() {
            self.patch = candidate.patch;
        }
    }

    /// Merges custom NBT data into this item's `custom_data` component.
    pub fn set_custom_data(&mut self, value: &CustomData) {
        let merged = self
            .get(CUSTOM_DATA)
            .cloned()
            .unwrap_or_default()
            .merged_with(value);
        if merged.is_empty() {
            self.remove(CUSTOM_DATA);
        } else {
            self.set(CUSTOM_DATA, merged);
        }
    }

    /// Applies furnace smelting to convert this item (e.g., raw iron -> iron ingot).
    pub fn apply_furnace_smelt(&mut self, use_input_count: bool) {
        let input = crate::recipe::SingleItemRecipeInput::new(self.copy_with_count(self.count()));
        let Some(recipe) = REGISTRY
            .recipes
            .find_match(&crate::recipe::vanilla_recipe_types::SMELTING, &input)
        else {
            return;
        };
        let count = if use_input_count { self.count() } else { 1 };
        let mut result = recipe.data().result.create();
        result.set_count(
            count
                .saturating_mul(result.count())
                .min(result.max_stack_size()),
        );
        *self = result;
    }

    /// Creates an exploration map pointing to a structure.
    ///
    /// Vanilla `ExplorationMapFunction.run`: the host hook claims the structure,
    /// allocates the map's saved data, and returns the target position plus the
    /// new map id. **Deviation:** the biome preview rendering
    /// (`MapItem.renderBiomePreviewMap`) runs against live level chunks and is
    /// left to the host when it populates the new map's saved data.
    pub fn create_exploration_map<R: rand::Rng>(
        &mut self,
        destination: &Identifier,
        decoration: &Identifier,
        zoom: i32,
        search_radius: i32,
        skip_existing_chunks: bool,
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::data_components::vanilla_components::{
            MAP_DECORATIONS, MAP_ID, MapDecorationEntry, MapDecorations, MapId,
        };

        if self.is_empty() {
            return;
        }
        let Some(origin) = ctx.origin else {
            return;
        };
        let Some(hooks) = ctx.item_source else {
            return;
        };
        let Some(((x, z), map_id)) = hooks.create_exploration_map(
            destination,
            origin,
            zoom,
            search_radius,
            skip_existing_chunks,
        ) else {
            return;
        };
        let Some(decoration_type) = REGISTRY.map_decoration_types.by_key(decoration) else {
            return;
        };

        self.set(MAP_ID, MapId::new(map_id));
        let decorations = self
            .get(MAP_DECORATIONS)
            .cloned()
            .unwrap_or_else(MapDecorations::empty);
        self.set(
            MAP_DECORATIONS,
            decorations.with_decoration(
                "+".to_owned(),
                MapDecorationEntry::new(
                    crate::RegistryReference::new(decoration_type),
                    f64::from(x),
                    f64::from(z),
                    180.0,
                ),
            ),
        );
    }

    /// Sets the custom name or item name of this item.
    ///
    /// Vanilla `SetNameFunction`: `name` is the JSON form of a text component.
    pub fn set_name(&mut self, name: &str, target: crate::loot_table::NameTarget) {
        use crate::data_components::vanilla_components::{CUSTOM_NAME, ITEM_NAME};

        let component = parse_text_component(name);
        match target {
            crate::loot_table::NameTarget::CustomName => self.set(CUSTOM_NAME, component),
            crate::loot_table::NameTarget::ItemName => self.set(ITEM_NAME, component),
        }
    }

    /// Sets the ominous bottle amplifier component.
    pub fn set_ominous_bottle_amplifier(&mut self, amplifier: i32) {
        self.set(
            OMINOUS_BOTTLE_AMPLIFIER,
            OminousBottleAmplifier::new(amplifier),
        );
    }

    /// Sets the potion type for this item.
    ///
    /// Vanilla `SetPotionFunction`: replaces only the base potion, keeping any
    /// custom color, effects, and name.
    pub fn set_potion(&mut self, id: &Identifier) {
        use crate::data_components::vanilla_components::{POTION_CONTENTS, PotionContents};

        let Some(potion) = REGISTRY.potions.by_key(id) else {
            return;
        };
        let contents = self
            .get(POTION_CONTENTS)
            .cloned()
            .unwrap_or_else(PotionContents::empty);
        self.set(
            POTION_CONTENTS,
            contents.with_potion(crate::RegistryReference::new(potion)),
        );
    }

    /// Sets the suspicious stew effects for this item.
    ///
    /// Vanilla `SetStewEffectFunction.run`: picks one entry uniformly, rolls its
    /// duration, and multiplies non-instantaneous durations by 20 ticks.
    pub fn set_stew_effects<R: rand::Rng>(
        &mut self,
        effects: &[crate::loot_table::StewEffect],
        rng: &mut R,
    ) {
        use crate::data_components::vanilla_components::{
            SUSPICIOUS_STEW_EFFECTS, SuspiciousStewEffect, SuspiciousStewEffects,
        };

        if !self.is(&vanilla_items::SUSPICIOUS_STEW) || effects.is_empty() {
            return;
        }
        let index = rng.random_range(0..effects.len());
        let entry = &effects[index];
        let Some(effect) = REGISTRY.mob_effects.by_key(&entry.effect_type) else {
            return;
        };
        let mut duration = entry.duration.get_int(rng);
        if !effect.is_instantaneous() {
            duration *= 20;
        }
        let current = self
            .get(SUSPICIOUS_STEW_EFFECTS)
            .cloned()
            .unwrap_or_else(SuspiciousStewEffects::empty);
        self.set(
            SUSPICIOUS_STEW_EFFECTS,
            current.with_effect_added(SuspiciousStewEffect::new(effect, duration)),
        );
    }

    pub fn set_enchantments(&mut self, enchantments: &[(Identifier, u32)], add: bool) {
        let mut current = self
            .get(self.enchantment_component())
            .cloned()
            .unwrap_or_else(ItemEnchantments::empty);

        for (key, level) in enchantments {
            if add {
                let existing = current.get_level(key);
                current.set(key.clone(), existing + *level);
            } else {
                current.set(key.clone(), *level);
            }
        }

        self.set(self.enchantment_component(), current);
    }

    /// Vanilla `ItemStack.enchant` → `Mutable.upgrade`: keeps the higher of existing vs new level.
    pub fn upgrade_enchantment(&mut self, enchantment: Identifier, level: u32) {
        let mut current = self
            .get(self.enchantment_component())
            .cloned()
            .unwrap_or_else(ItemEnchantments::empty);
        current.upgrade(enchantment, level);
        self.set(self.enchantment_component(), current);
    }

    /// Changes the item type entirely.
    pub fn set_item(&mut self, new_item: &Identifier) {
        if let Some(item_ref) = REGISTRY.items.by_key(new_item) {
            self.item = item_ref;
            self.patch.sanitize_against(&item_ref.components);
        }
    }

    /// Copies the name from a source entity/block to this item.
    ///
    /// Vanilla `CopyNameFunction`: copies a `Nameable` source's custom name.
    pub fn copy_name<R: rand::Rng>(
        &mut self,
        source: crate::loot_table::CopySource,
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::data_components::vanilla_components::CUSTOM_NAME;
        use crate::loot_table::LootContextEntity;

        let name = match source {
            crate::loot_table::CopySource::BlockEntity => {
                ctx.block_entity.and_then(|block| block.custom_name)
            }
            crate::loot_table::CopySource::This => {
                entity_ref(ctx, LootContextEntity::This).and_then(|entity| entity.custom_name)
            }
            crate::loot_table::CopySource::Attacker => {
                entity_ref(ctx, LootContextEntity::Killer).and_then(|entity| entity.custom_name)
            }
            crate::loot_table::CopySource::DirectAttacker => {
                entity_ref(ctx, LootContextEntity::DirectKiller)
                    .and_then(|entity| entity.custom_name)
            }
        };
        if let Some(name) = name {
            self.set(CUSTOM_NAME, TextComponent::plain(name.to_owned()));
        }
    }

    /// Sets lore lines on this item.
    ///
    /// Vanilla `SetLoreFunction`: each line is the JSON form of a text component
    /// and `mode` is applied to the existing lines.
    pub fn set_lore(&mut self, lore: &[&str], mode: crate::loot_table::ListOperation) {
        use crate::data_components::vanilla_components::{ItemLore, LORE};

        let replacement: Vec<TextComponent> =
            lore.iter().map(|line| parse_text_component(line)).collect();
        let current = self.get(LORE).cloned().unwrap_or_else(ItemLore::empty);
        let lines = mode.apply(current.lines(), &replacement, ItemLore::MAX_LINES);
        match ItemLore::new(lines) {
            Ok(lore) => self.set(LORE, lore),
            Err(error) => log::warn!("Could not set lore: {error}"),
        }
    }

    /// Sets container inventory contents.
    ///
    /// Vanilla `SetContainerContents`: expands the entries and stores the result
    /// in the component named by `component_type`.
    pub fn set_contents<R: rand::Rng>(
        &mut self,
        entries: &[crate::loot_table::LootEntry],
        component_type: &Identifier,
        ctx: &mut crate::loot_table::LootContext<'_, R>,
    ) {
        if self.is_empty() {
            return;
        }
        let mut items = Vec::new();
        for entry in entries {
            items.extend(entry.expand_items(ctx));
        }
        self.store_container_contents(component_type, &items);
    }

    /// Modifies existing container contents.
    ///
    /// Vanilla `ModifyContainerContents`: applies the modifier functions to every
    /// contained stack and clamps each result to its max stack size.
    pub fn modify_contents<R: rand::Rng>(
        &mut self,
        modifier: &[crate::loot_table::ConditionalLootFunction],
        component_type: &Identifier,
        ctx: &mut crate::loot_table::LootContext<'_, R>,
    ) {
        let Some(mut items) = self.container_contents(component_type) else {
            return;
        };
        for item in &mut items {
            if item.is_empty() {
                continue;
            }
            for function in modifier {
                function.apply(item, ctx);
            }
            let max_stack_size = item.max_stack_size();
            if item.count > max_stack_size {
                item.count = max_stack_size;
            }
        }
        self.store_container_contents(component_type, &items);
    }

    /// Sets the container's loot table reference.
    pub fn set_loot_table(&mut self, loot_table: &Identifier, seed: Option<i64>) {
        use crate::data_components::vanilla_components::{CONTAINER_LOOT, SeededContainerLoot};

        if self.is_empty() {
            return;
        }
        self.set(
            CONTAINER_LOOT,
            SeededContainerLoot::new(loot_table.clone(), seed.unwrap_or(0)),
        );
    }

    /// Sets attribute modifiers on this item.
    ///
    /// Vanilla `SetAttributesFunction`: `replace` drops the item's existing
    /// modifiers, otherwise the rolled modifiers are appended.
    pub fn set_attributes<R: rand::Rng>(
        &mut self,
        modifiers: &[crate::loot_table::AttributeModifier],
        replace: bool,
        rng: &mut R,
    ) {
        use crate::data_components::vanilla_components::{
            ATTRIBUTE_MODIFIERS, ItemAttributeModifierDisplay, ItemAttributeModifierEntry,
            ItemAttributeModifiers,
        };

        let mut current = if replace {
            ItemAttributeModifiers::empty()
        } else {
            self.get(ATTRIBUTE_MODIFIERS)
                .cloned()
                .unwrap_or_else(ItemAttributeModifiers::empty)
        };
        for modifier in modifiers {
            let Some(attribute) = REGISTRY.attributes.by_key(&modifier.attribute) else {
                continue;
            };
            let operation = match modifier.operation {
                crate::loot_table::AttributeOperation::AddValue => {
                    crate::attribute::AttributeModifierOperation::AddValue
                }
                crate::loot_table::AttributeOperation::AddMultipliedBase => {
                    crate::attribute::AttributeModifierOperation::AddMultipliedBase
                }
                crate::loot_table::AttributeOperation::AddMultipliedTotal => {
                    crate::attribute::AttributeModifierOperation::AddMultipliedTotal
                }
            };
            // Vanilla `ItemAttributeModifiers.withModifierAdded` drops an existing
            // entry with the same attribute and modifier id before appending.
            current
                .modifiers
                .retain(|entry| !(entry.attribute.key == attribute.key && entry.id == modifier.id));
            current.modifiers.push(ItemAttributeModifierEntry {
                attribute,
                id: modifier.id.clone(),
                amount: f64::from(modifier.amount.get_simple(rng)),
                operation,
                slot: modifier.slot,
                display: ItemAttributeModifierDisplay::Default,
            });
        }
        self.set(ATTRIBUTE_MODIFIERS, current);
    }

    /// Fills a player head with texture from an entity.
    ///
    /// Vanilla `FillPlayerHead`: only `player_head` stacks receive a resolved
    /// profile, and only when the referenced entity is a player.
    pub fn fill_player_head<R: rand::Rng>(
        &mut self,
        entity: crate::loot_table::LootContextEntity,
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::data_components::vanilla_components::PROFILE;

        if !self.is(&vanilla_items::PLAYER_HEAD) {
            return;
        }
        let Some(hooks) = ctx.item_source else {
            return;
        };
        let Some(profile) = hooks.player_profile(entity) else {
            return;
        };
        self.set(
            PROFILE,
            crate::resolvable_profile::ResolvableProfile::from(profile.clone()),
        );
    }

    /// Copies custom NBT data from a source.
    ///
    /// Vanilla `CopyCustomDataFunction`. **Deviation:** for paths that select
    /// several targets, the `append` and `merge` strategies combine the source
    /// values into one tag and write it to every target instead of updating each
    /// target in place, because `NbtPath` exposes no multi-target mutation hook.
    pub fn copy_custom_data<R: rand::Rng>(
        &mut self,
        source: crate::loot_table::CopySource,
        operations: &[crate::loot_table::CopyDataOperation],
        ctx: &crate::loot_table::LootContext<'_, R>,
    ) {
        use crate::data_components::CustomData;
        use crate::data_components::vanilla_components::CUSTOM_DATA;
        use crate::loot_table::{CopyDataOp, LootContextEntity};
        use simdnbt::owned::NbtTag;
        use steel_utils::nbt::parse_nbt_path;

        let Some(hooks) = ctx.item_source else {
            return;
        };
        let source_tag = match source {
            crate::loot_table::CopySource::BlockEntity => hooks.block_entity_nbt(),
            crate::loot_table::CopySource::This => hooks.entity_nbt(LootContextEntity::This),
            crate::loot_table::CopySource::Attacker => hooks.entity_nbt(LootContextEntity::Killer),
            crate::loot_table::CopySource::DirectAttacker => {
                hooks.entity_nbt(LootContextEntity::DirectKiller)
            }
        };
        let Some(source_tag) = source_tag else {
            return;
        };

        let mut target = self
            .get(CUSTOM_DATA)
            .map(CustomData::copy_tag)
            .unwrap_or_default();
        let mut tag = NbtTag::Compound(target.clone());
        for operation in operations {
            let (Ok(source_path), Ok(target_path)) = (
                parse_nbt_path(operation.source_path),
                parse_nbt_path(operation.target_path),
            ) else {
                continue;
            };
            let values = source_path.get(&NbtTag::Compound(source_tag.clone()));
            let Some(last) = values.last().cloned() else {
                continue;
            };
            let merged = match operation.op {
                CopyDataOp::Replace => last,
                CopyDataOp::Append => {
                    let mut list = match target_path.get(&tag).into_iter().next() {
                        Some(NbtTag::List(list)) => list,
                        _ => simdnbt::owned::NbtList::default(),
                    };
                    for value in &values {
                        list.push(value.clone());
                    }
                    NbtTag::List(list)
                }
                CopyDataOp::Merge => {
                    let mut compound = match target_path.get(&tag).into_iter().next() {
                        Some(NbtTag::Compound(compound)) => compound,
                        _ => simdnbt::owned::NbtCompound::new(),
                    };
                    for value in &values {
                        if let NbtTag::Compound(source) = value {
                            for (key, value) in source.iter() {
                                compound.insert(key.to_str().as_ref(), value.clone());
                            }
                        }
                    }
                    NbtTag::Compound(compound)
                }
            };
            let _ = target_path.set(&mut tag, merged);
        }

        if let NbtTag::Compound(compound) = tag {
            target = compound;
        }
        match CustomData::try_from_compound(target) {
            Some(data) if !data.is_empty() => self.set(CUSTOM_DATA, data),
            _ => self.remove(CUSTOM_DATA),
        }
    }

    /// Sets banner pattern layers.
    ///
    /// Vanilla `SetBannerPatternFunction`: `append` keeps the existing layers.
    pub fn set_banner_pattern(
        &mut self,
        patterns: &[crate::loot_table::BannerPattern],
        append: bool,
    ) {
        use crate::RegistryHolder;
        use crate::data_components::vanilla_components::{
            BANNER_PATTERNS, BannerPatternLayer, BannerPatternLayers,
        };

        let current = if append {
            self.get(BANNER_PATTERNS)
                .cloned()
                .unwrap_or_else(BannerPatternLayers::empty)
        } else {
            BannerPatternLayers::empty()
        };
        let mut layers = current.layers().to_vec();
        for pattern in patterns {
            let Some(entry) = REGISTRY.banner_patterns.by_key(&pattern.pattern) else {
                continue;
            };
            layers.push(BannerPatternLayer::new(
                RegistryHolder::reference(entry),
                pattern.color,
            ));
        }
        self.set(BANNER_PATTERNS, BannerPatternLayers::new(layers));
    }

    /// Sets firework rocket properties.
    ///
    /// Vanilla `SetFireworksFunction`: absent fields keep the current values.
    pub fn set_fireworks(
        &mut self,
        explosions: Option<&[crate::loot_table::FireworkExplosion]>,
        flight_duration: Option<i32>,
    ) {
        use crate::data_components::vanilla_components::FIREWORKS;

        let current = self
            .get(FIREWORKS)
            .cloned()
            .unwrap_or_else(|| crate::data_components::vanilla_components::Fireworks::empty());
        let flight_duration = flight_duration.unwrap_or_else(|| current.flight_duration());
        let explosions = match explosions {
            Some(explosions) => explosions
                .iter()
                .map(convert_loot_firework_explosion)
                .collect(),
            None => current.explosions().to_vec(),
        };
        match crate::data_components::vanilla_components::Fireworks::new(
            flight_duration,
            explosions,
        ) {
            Ok(fireworks) => self.set(FIREWORKS, fireworks),
            Err(error) => log::warn!("Could not set fireworks: {error}"),
        }
    }

    /// Sets firework star explosion properties.
    pub fn set_firework_explosion(&mut self, explosion: &crate::loot_table::FireworkExplosion) {
        use crate::data_components::vanilla_components::FIREWORK_EXPLOSION;

        self.set(
            FIREWORK_EXPLOSION,
            convert_loot_firework_explosion(explosion),
        );
    }

    /// Sets book cover (title/author for written books).
    ///
    /// Vanilla `SetBookCoverFunction`: absent fields keep the current values.
    pub fn set_book_cover(
        &mut self,
        title: Option<&str>,
        author: Option<&str>,
        generation: Option<i32>,
    ) {
        use crate::data_components::vanilla_components::{
            Filterable, WRITTEN_BOOK_CONTENT, WrittenBookContent,
        };

        let current = self
            .get(WRITTEN_BOOK_CONTENT)
            .cloned()
            .unwrap_or_else(WrittenBookContent::empty);
        let title = title.map_or_else(
            || current.title().clone(),
            |title| Filterable::pass_through(title.to_owned()),
        );
        let author = author.map_or_else(|| current.author().to_owned(), str::to_owned);
        let generation = generation.unwrap_or_else(|| current.generation());
        self.set(
            WRITTEN_BOOK_CONTENT,
            current.with_cover(title, author, generation),
        );
    }

    /// Sets written book page contents.
    ///
    /// Vanilla `SetWrittenBookPagesFunction`: `mode` is applied to the current
    /// pages; pages are JSON text components.
    pub fn set_written_book_pages(
        &mut self,
        pages: &[&str],
        mode: crate::loot_table::ListOperation,
    ) {
        use crate::data_components::vanilla_components::{
            Filterable, WRITTEN_BOOK_CONTENT, WrittenBookContent,
        };

        let current = self
            .get(WRITTEN_BOOK_CONTENT)
            .cloned()
            .unwrap_or_else(WrittenBookContent::empty);
        let replacement: Vec<Filterable<TextComponent>> = pages
            .iter()
            .map(|page| Filterable::pass_through(parse_text_component(page)))
            .collect();
        let new_pages = mode.apply(current.pages(), &replacement, usize::MAX);
        self.set(WRITTEN_BOOK_CONTENT, current.with_replaced_pages(new_pages));
    }

    /// Sets writable book page contents.
    ///
    /// Vanilla `SetWritableBookPagesFunction`: `mode` is applied to the current
    /// pages, capped at the component's 100-page limit.
    pub fn set_writable_book_pages(
        &mut self,
        pages: &[&str],
        mode: crate::loot_table::ListOperation,
    ) {
        use crate::data_components::vanilla_components::{
            Filterable, WRITABLE_BOOK_CONTENT, WritableBookContent,
        };

        let current = self
            .get(WRITABLE_BOOK_CONTENT)
            .cloned()
            .unwrap_or_else(WritableBookContent::empty);
        let replacement: Vec<Filterable<String>> = pages
            .iter()
            .map(|page| Filterable::pass_through((*page).to_owned()))
            .collect();
        let new_pages = mode.apply(
            current.pages(),
            &replacement,
            WritableBookContent::MAX_PAGES,
        );
        match WritableBookContent::new(new_pages) {
            Ok(contents) => self.set(WRITABLE_BOOK_CONTENT, contents),
            Err(error) => log::warn!("Could not set writable book pages: {error}"),
        }
    }

    /// Reads the item stacks stored in a container component.
    fn container_contents(&self, component_type: &Identifier) -> Option<Vec<Self>> {
        use crate::data_components::vanilla_components::{
            BUNDLE_CONTENTS, CHARGED_PROJECTILES, CONTAINER,
        };

        if component_type == &CONTAINER.key {
            let container = self.get(CONTAINER)?;
            return Some(
                container
                    .items()
                    .iter()
                    .map(|slot| {
                        slot.as_ref()
                            .map_or_else(Self::empty, ItemStackTemplate::create)
                    })
                    .collect(),
            );
        }
        if component_type == &BUNDLE_CONTENTS.key {
            let bundle = self.get(BUNDLE_CONTENTS)?;
            return Some(
                bundle
                    .items()
                    .iter()
                    .map(ItemStackTemplate::create)
                    .collect(),
            );
        }
        if component_type == &CHARGED_PROJECTILES.key {
            let projectiles = self.get(CHARGED_PROJECTILES)?;
            return Some(
                projectiles
                    .items()
                    .iter()
                    .map(ItemStackTemplate::create)
                    .collect(),
            );
        }
        None
    }

    /// Stores item stacks in a container component (vanilla
    /// `ContainerComponentManipulator.setContents`).
    fn store_container_contents(&mut self, component_type: &Identifier, items: &[Self]) {
        use crate::data_components::vanilla_components::{
            BUNDLE_CONTENTS, BundleContents, CHARGED_PROJECTILES, CONTAINER, ChargedProjectiles,
            ItemContainerContents,
        };

        if component_type == &CONTAINER.key {
            let templates: Vec<Option<ItemStackTemplate>> = items
                .iter()
                .map(|item| ItemStackTemplate::from_stack(item).ok())
                .collect();
            let last = templates.iter().rposition(Option::is_some);
            let trimmed = last.map_or_else(Vec::new, |last| templates[..=last].to_vec());
            match ItemContainerContents::new(trimmed) {
                Ok(container) => self.set(CONTAINER, container),
                Err(error) => log::warn!("Could not set container contents: {error}"),
            }
            return;
        }
        if component_type == &BUNDLE_CONTENTS.key {
            let bundle = self
                .get(BUNDLE_CONTENTS)
                .cloned()
                .unwrap_or_else(BundleContents::empty);
            self.set(BUNDLE_CONTENTS, bundle.with_contents(items.iter().cloned()));
            return;
        }
        if component_type == &CHARGED_PROJECTILES.key {
            let templates: Vec<ItemStackTemplate> = items
                .iter()
                .filter_map(|item| ItemStackTemplate::from_stack(item).ok())
                .collect();
            match ChargedProjectiles::new(templates) {
                Ok(projectiles) => self.set(CHARGED_PROJECTILES, projectiles),
                Err(error) => log::warn!("Could not set charged projectiles: {error}"),
            }
        }
    }

    /// Runs vanilla `ToggleTooltips`: each boolean says whether the component is shown.
    pub fn toggle_tooltips(&mut self, toggles: &[(Identifier, bool)]) {
        use crate::data_components::vanilla_components::{TOOLTIP_DISPLAY, TooltipDisplay};

        let mut display = self
            .get(TOOLTIP_DISPLAY)
            .cloned()
            .unwrap_or(TooltipDisplay::DEFAULT);
        for (component, shown) in toggles {
            display = display.with_hidden_key(component.clone(), !shown);
        }
        self.set(TOOLTIP_DISPLAY, display);
    }

    #[must_use]
    pub fn components_equal(&self, other: &Self) -> bool {
        let mut all_keys = rustc_hash::FxHashSet::default();

        for key in self.prototype().keys() {
            if !self.patch.is_removed(key) {
                all_keys.insert(key);
            }
        }
        for (key, entry) in self.patch.iter() {
            if matches!(entry, ComponentPatchEntry::Set(_)) {
                all_keys.insert(key);
            }
        }
        for key in other.prototype().keys() {
            if !other.patch.is_removed(key) {
                all_keys.insert(key);
            }
        }
        for (key, entry) in other.patch.iter() {
            if matches!(entry, ComponentPatchEntry::Set(_)) {
                all_keys.insert(key);
            }
        }
        for key in all_keys {
            let val_a = self.get_effective_value_raw(key);
            let val_b = other.get_effective_value_raw(key);

            match (val_a, val_b) {
                (Some(a), Some(b)) => {
                    if a != b {
                        return false;
                    }
                }
                (None, None) => {}
                _ => return false,
            }
        }

        true
    }

    /// Vanilla `ItemStack.getCustomName`: an explicit custom name, or a
    /// nonblank written-book title.
    #[must_use]
    pub fn custom_name(&self) -> Option<Cow<'_, TextComponent>> {
        if let Some(name) = self.get(CUSTOM_NAME) {
            return Some(Cow::Borrowed(name));
        }

        let title = self.get(WRITTEN_BOOK_CONTENT)?.title().raw();
        (!java::is_blank(title)).then(|| Cow::Owned(TextComponent::plain(title.to_owned())))
    }

    /// Returns the custom name if set, otherwise the effective `ITEM_NAME`
    /// component.
    ///
    /// This does not apply item-class name overrides such as potion contents;
    /// callers with access to item behaviors should use their behavior-aware
    /// hover-name API.
    #[must_use]
    pub fn custom_or_component_name(&self) -> Cow<'_, TextComponent> {
        self.custom_name()
            .or_else(|| self.get(ITEM_NAME).map(Cow::Borrowed))
            .unwrap_or(Cow::Borrowed(&EMPTY_NAME))
    }
}

static EMPTY_NAME: TextComponent = TextComponent::new();

/// Parses a loot JSON text component, falling back to literal text.
///
/// Vanilla loot functions carry text components in their JSON form; Steel's
/// generated loot data stores them as JSON strings.
fn parse_text_component(text: &str) -> TextComponent {
    serde_json::from_str::<TextComponent>(text)
        .unwrap_or_else(|_| TextComponent::plain(text.to_owned()))
}

/// Resolves a loot context entity target, mirroring `LootContext::get_entity`.
fn entity_ref<'a, R: rand::Rng>(
    ctx: &crate::loot_table::LootContext<'a, R>,
    target: crate::loot_table::LootContextEntity,
) -> Option<crate::loot_table::EntityRef<'a>> {
    use crate::loot_table::LootContextEntity;

    match target {
        LootContextEntity::This => ctx.this_entity,
        LootContextEntity::Killer => ctx.killer_entity,
        LootContextEntity::DirectKiller => ctx.direct_killer_entity,
        LootContextEntity::KillerPlayer => ctx.last_damage_player,
        LootContextEntity::Interacting => ctx.interacting_entity,
    }
}

/// Converts a loot-table firework explosion into its item component value.
fn convert_loot_firework_explosion(
    explosion: &crate::loot_table::FireworkExplosion,
) -> crate::data_components::vanilla_components::FireworkExplosion {
    use crate::data_components::vanilla_components::{FireworkExplosion, FireworkExplosionShape};
    use crate::loot_table::FireworkShape;

    let shape = match explosion.shape {
        FireworkShape::SmallBall => FireworkExplosionShape::SmallBall,
        FireworkShape::LargeBall => FireworkExplosionShape::LargeBall,
        FireworkShape::Star => FireworkExplosionShape::Star,
        FireworkShape::Creeper => FireworkExplosionShape::Creeper,
        FireworkShape::Burst => FireworkExplosionShape::Burst,
    };
    FireworkExplosion::new(
        shape,
        explosion.colors.to_vec(),
        explosion.fade_colors.to_vec(),
        explosion.has_trail,
        explosion.has_twinkle,
    )
}

fn validate_contained_item_sizes<'a>(
    items: impl IntoIterator<Item = &'a ItemStackTemplate>,
) -> Result<()> {
    for item in items {
        let max_stack_size = item.max_stack_size();
        if item.count() > max_stack_size {
            return Err(std::io::Error::other(format!(
                "Item stack with count of {} was larger than maximum: {max_stack_size}",
                item.count()
            )));
        }
    }
    Ok(())
}

impl From<&ItemStack> for ItemStack {
    fn from(stack: &Self) -> Self {
        stack.to_owned()
    }
}

impl From<ItemRef> for ItemStack {
    fn from(item: ItemRef) -> Self {
        Self::new(item)
    }
}

impl From<&'static std::sync::LazyLock<Item>> for ItemStack {
    fn from(item: &'static std::sync::LazyLock<Item>) -> Self {
        Self::new(item)
    }
}

impl std::fmt::Display for ItemStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_empty() {
            write!(f, "Empty")
        } else {
            write!(f, "{} {}", self.count, self.item.key)
        }
    }
}

impl WriteTo for ItemStack {
    fn write(&self, writer: &mut impl Write) -> Result<()> {
        if self.is_empty() {
            VarInt(0).write(writer)?;
        } else {
            VarInt(self.count).write(writer)?;
            // Write item ID as VarInt
            VarInt(self.item.id() as i32).write(writer)?;
            // Write DataComponentPatch
            self.patch.write(writer)?;
        }
        Ok(())
    }
}

impl ReadFrom for ItemStack {
    fn read(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let count = VarInt::read(data)?.0;
        if count <= 0 {
            return Ok(Self::empty());
        }

        let item_id = VarInt::read(data)?.0;
        let item_id = usize::try_from(item_id)
            .map_err(|_| std::io::Error::other(format!("Negative item id: {item_id}")))?;
        let item = REGISTRY
            .items
            .by_id(item_id)
            .ok_or_else(|| std::io::Error::other(format!("Unknown item id: {item_id}")))?;

        // Read DataComponentPatch
        let patch = DataComponentPatch::read(data)?;

        Ok(Self::with_count_and_patch(item, count, patch))
    }
}

impl ItemStack {
    /// Reads an item stack using the delimited (untrusted) component format.
    ///
    /// Vanilla uses this for serverbound packets where component data is
    /// length-prefixed (e.g., `ServerboundSetCreativeModeSlotPacket`).
    pub fn read_untrusted(data: &mut Cursor<&[u8]>) -> Result<Self> {
        let count = VarInt::read(data)?.0;
        if count <= 0 {
            return Ok(Self::empty());
        }

        let item_id = VarInt::read(data)?.0;
        let item_id = usize::try_from(item_id)
            .map_err(|_| std::io::Error::other(format!("Negative item id: {item_id}")))?;
        let item = REGISTRY
            .items
            .by_id(item_id)
            .ok_or_else(|| std::io::Error::other(format!("Unknown item id: {item_id}")))?;
        let patch = DataComponentPatch::read_delimited(data)?;

        let stack = Self::with_count_and_patch(item, count, patch);
        stack.validate_persistent_encoding()?;
        Ok(stack)
    }
}

use simdnbt::{
    FromNbtTag, ToNbtTag,
    borrow::{NbtCompound as NbtCompoundView, NbtTag as BorrowedNbtTag},
    owned::NbtCompound,
};
use steel_utils::nbt::NbtNumeric as _;

impl ToNbtTag for ItemStack {
    /// Converts this item stack to an NBT tag for persistent storage.
    ///
    /// Format (matching vanilla Minecraft):
    /// ```text
    /// {
    ///     id: "minecraft:stone",
    ///     count: 64,
    ///     components: { ... }  // Only present if patch is non-empty
    /// }
    /// ```
    fn to_nbt_tag(self) -> simdnbt::owned::NbtTag {
        self.to_nbt_tag_ref()
    }
}

impl ItemStack {
    /// Checks that this stack can be encoded by Vanilla's persistent
    /// `ItemStack.CODEC` before untrusted network data enters server state.
    ///
    /// This is an ingress check rather than a type invariant: programmatic
    /// component mutation can still create values whose save codec reports
    /// and omits invalid fields.
    pub fn validate_persistent_encoding(&self) -> Result<()> {
        if self.is_empty() {
            return Ok(());
        }
        if !(1..=99).contains(&self.count) {
            return Err(std::io::Error::other(format!(
                "Item stack count {} is outside the persistent range 1..=99",
                self.count
            )));
        }
        self.patch.try_to_nbt_tag_ref().map(|_| ())
    }

    /// Converts this item stack to an NBT tag for persistent storage without consuming it.
    #[must_use]
    pub fn to_nbt_tag_ref(&self) -> simdnbt::owned::NbtTag {
        if self.is_empty() {
            // Empty stacks are represented as an empty compound
            return simdnbt::owned::NbtTag::Compound(NbtCompound::new());
        }

        let mut compound = NbtCompound::new();

        // id: The item identifier
        compound.insert("id", self.item.key.to_string());

        compound.insert("count", self.count);

        // components: The component patch (only if non-empty)
        if !self.patch.is_empty() {
            compound.insert("components", self.patch.to_nbt_tag_ref());
        }

        simdnbt::owned::NbtTag::Compound(compound)
    }
}

impl FromNbtTag for ItemStack {
    /// Parses an item stack from an NBT tag.
    ///
    /// Accepts the vanilla format:
    /// ```text
    /// {
    ///     id: "minecraft:stone",
    ///     count: 64,
    ///     components: { ... }
    /// }
    /// ```
    fn from_nbt_tag(tag: BorrowedNbtTag) -> Option<Self> {
        let compound = tag.compound()?;

        // Get the item ID
        let id_str = compound.get("id")?.string()?.to_str();
        let id = id_str.parse::<Identifier>().ok()?;

        // Look up the item in the registry
        let item = REGISTRY.items.by_key(&id)?;

        let count = decode_persistent_count(compound.get("count"))?;

        let patch = match compound.get("components") {
            Some(tag) => DataComponentPatch::from_nbt_tag(tag)?,
            None => DataComponentPatch::new(),
        };

        Some(Self::with_count_and_patch(item, count, patch))
    }
}

impl ItemStack {
    /// Parses an `ItemStack` from a borrowed `NbtCompoundView`.
    ///
    /// This is useful for loading items from disk where we have borrowed NBT data
    /// and want to avoid the overhead of converting to an owned tag first.
    #[must_use]
    pub fn from_borrowed_compound(compound: &NbtCompoundView<'_, '_>) -> Option<Self> {
        // Get the item ID
        let id_str = compound.string("id")?.to_str();
        let id = id_str.parse::<Identifier>().ok()?;

        // Look up the item in the registry
        let item = REGISTRY.items.by_key(&id)?;

        let count = decode_persistent_count(compound.get("count"))?;

        let patch = match compound.get("components") {
            Some(tag) => DataComponentPatch::from_nbt_tag(tag)?,
            None => DataComponentPatch::new(),
        };

        Some(Self::with_count_and_patch(item, count, patch))
    }
}

fn decode_persistent_count(tag: Option<BorrowedNbtTag<'_, '_>>) -> Option<i32> {
    let count = match tag {
        Some(tag) => tag.codec_i32()?,
        None => 1,
    };
    (1..=99).contains(&count).then_some(count)
}

#[cfg(test)]
mod consume_tests {
    use super::ItemStack;
    use crate::{init_vanilla_registry, vanilla_items};

    #[test]
    fn consume_shrinks_only_without_infinite_materials() {
        init_vanilla_registry();

        let mut survival = ItemStack::with_count(&vanilla_items::GLOWSTONE, 2);
        survival.consume_one(false);
        assert_eq!(survival.count(), 1);

        let mut creative = ItemStack::with_count(&vanilla_items::GLOWSTONE, 2);
        creative.consume_one(true);
        assert_eq!(creative.count(), 2);
    }

    #[test]
    fn consume_and_return_splits_before_shrinking() {
        init_vanilla_registry();

        let mut survival = ItemStack::with_count(&vanilla_items::GLOWSTONE, 2);
        let taken = survival.consume_and_return(1, false);
        assert_eq!(taken.count(), 1);
        assert_eq!(survival.count(), 1);

        let mut creative = ItemStack::with_count(&vanilla_items::GLOWSTONE, 2);
        let taken = creative.consume_and_return(1, true);
        assert_eq!(taken.count(), 1);
        assert_eq!(creative.count(), 2);
    }
}

#[cfg(test)]
mod enchantment_tests {
    use super::ItemStack;
    use crate::{init_vanilla_registry, vanilla_enchantments, vanilla_items};

    #[test]
    fn stored_book_enchantments_are_not_active_item_enchantments() {
        init_vanilla_registry();
        let mut book = ItemStack::new(&vanilla_items::ENCHANTED_BOOK);
        book.upgrade_enchantment(vanilla_enchantments::SHARPNESS.key.clone(), 3);

        assert_eq!(
            book.get_enchantment_level(&vanilla_enchantments::SHARPNESS.key),
            0
        );
        assert_eq!(
            book.get_enchantments_for_crafting().map(|enchantments| {
                enchantments.get_level(&vanilla_enchantments::SHARPNESS.key)
            }),
            Some(3)
        );
    }
}

#[cfg(test)]
mod name_tests {
    use text_components::TextComponent;

    use super::ItemStack;
    use crate::data_components::components::{Filterable, WrittenBookContent};
    use crate::data_components::vanilla_components::{CUSTOM_NAME, WRITTEN_BOOK_CONTENT};
    use crate::init_vanilla_registry;
    use crate::vanilla_items;

    fn written_book(raw_title: &str, filtered_title: Option<&str>) -> ItemStack {
        let content = WrittenBookContent::new(
            Filterable::new(raw_title.to_owned(), filtered_title.map(ToOwned::to_owned)),
            "Author".to_owned(),
            0,
            Vec::new(),
            true,
        );
        let Ok(content) = content else {
            panic!("test written-book content should be valid");
        };
        let mut book = ItemStack::new(&vanilla_items::WRITTEN_BOOK);
        book.set(WRITTEN_BOOK_CONTENT, content);
        book
    }

    #[test]
    fn written_book_raw_title_is_its_custom_and_hover_name() {
        init_vanilla_registry();
        let book = written_book("Raw title", Some("Filtered title"));
        let expected = TextComponent::plain("Raw title");

        assert_eq!(book.custom_name().as_deref(), Some(&expected));
        assert_eq!(book.custom_or_component_name().as_ref(), &expected);
    }

    #[test]
    fn explicit_custom_name_takes_precedence_over_written_book_title() {
        init_vanilla_registry();
        let mut book = written_book("Book title", None);
        let explicit = TextComponent::plain("Explicit name");
        book.set(CUSTOM_NAME, explicit.clone());

        assert_eq!(book.custom_name().as_deref(), Some(&explicit));
        assert_eq!(book.custom_or_component_name().as_ref(), &explicit);
    }

    #[test]
    fn written_book_title_uses_java_blank_rules() {
        init_vanilla_registry();
        let blank = written_book("\u{00a0}\u{202f}", None);
        assert!(blank.custom_name().is_none());
        let Some(default_name) = blank.get(crate::data_components::vanilla_components::ITEM_NAME)
        else {
            panic!("written book should have a default item name");
        };
        assert_eq!(blank.custom_or_component_name().as_ref(), default_name);

        let next_line = written_book("\u{0085}", None);
        assert_eq!(
            next_line.custom_name().as_deref(),
            Some(&TextComponent::plain("\u{0085}"))
        );
    }
}

#[cfg(test)]
mod durability_tests {
    use steel_utils::random::xoroshiro::Xoroshiro;

    use super::ItemStack;
    use crate::data_components::vanilla_components::{ENCHANTMENTS, ItemEnchantments};
    use crate::init_vanilla_registry;
    use crate::{vanilla_enchantments, vanilla_items};

    fn with_unbreaking(item: crate::items::ItemRef, level: u32) -> ItemStack {
        let mut stack = ItemStack::new(item);
        let mut enchantments = ItemEnchantments::empty();
        enchantments.set(vanilla_enchantments::UNBREAKING.key.clone(), level);
        stack.set(ENCHANTMENTS, enchantments);
        stack
    }

    #[test]
    fn item_damage_uses_generated_unbreaking_tool_requirements() {
        init_vanilla_registry();
        let mut armor = with_unbreaking(&vanilla_items::DIAMOND_CHESTPLATE, 3);
        let mut tool = with_unbreaking(&vanilla_items::DIAMOND_PICKAXE, 3);
        let mut armor_random = Xoroshiro::from_seed_unmixed(42);
        let mut tool_random = Xoroshiro::from_seed_unmixed(42);

        assert!(!armor.hurt_and_break_with_random(100, false, &mut armor_random));
        assert!(!tool.hurt_and_break_with_random(100, false, &mut tool_random));
        assert_eq!(armor.get_damage_value(), 68);
        assert_eq!(tool.get_damage_value(), 18);

        let effects = vanilla_enchantments::UNBREAKING.effects.item_damage;
        assert_eq!(
            effects[0].requirements.and_then(|requirements| {
                requirements.matches_item_context(&vanilla_items::DIAMOND_CHESTPLATE)
            }),
            Some(true)
        );
        assert_eq!(
            effects[1].requirements.and_then(|requirements| {
                requirements.matches_item_context(&vanilla_items::DIAMOND_CHESTPLATE)
            }),
            Some(false)
        );
        assert_eq!(
            effects[0].requirements.and_then(|requirements| {
                requirements.matches_item_context(&vanilla_items::DIAMOND_PICKAXE)
            }),
            Some(false)
        );
        assert_eq!(
            effects[1].requirements.and_then(|requirements| {
                requirements.matches_item_context(&vanilla_items::DIAMOND_PICKAXE)
            }),
            Some(true)
        );
    }

    #[test]
    fn match_tool_supports_generated_direct_item_sets() {
        init_vanilla_registry();
        let requirements = vanilla_enchantments::INFINITY.effects.ammo_use[0]
            .requirements
            .expect("Infinity ammo use should have a match_tool requirement");

        assert_eq!(
            requirements.matches_item_context(&vanilla_items::ARROW),
            Some(true)
        );
        assert_eq!(
            requirements.matches_item_context(&vanilla_items::SPECTRAL_ARROW),
            Some(false)
        );
    }
}

#[cfg(test)]
mod persistence_tests {
    use std::io::Cursor;

    use simdnbt::FromNbtTag;
    use simdnbt::borrow::{NbtTag as BorrowedNbtTag, read_tag};
    use simdnbt::owned::{NbtCompound, NbtTag};
    use steel_utils::codec::VarInt;
    use steel_utils::serial::WriteTo;

    use super::ItemStack;
    use crate::data_components::vanilla_components::{
        CUSTOM_DATA, DEATH_PROTECTION, JUKEBOX_PLAYABLE, LORE, MAX_DAMAGE, MAX_STACK_SIZE,
        TOOLTIP_DISPLAY,
    };
    use crate::data_components::{CustomData, JukeboxPlayable};
    use crate::init_vanilla_registry;
    use crate::{REGISTRY, RegistryEntry, RegistryExt, vanilla_items, vanilla_jukebox_songs};

    fn with_borrowed_tag<R>(tag: NbtTag, visitor: impl FnOnce(BorrowedNbtTag<'_, '_>) -> R) -> R {
        let mut bytes = Vec::new();
        tag.write(&mut bytes);
        let borrowed =
            read_tag(&mut Cursor::new(bytes.as_slice())).expect("owned test tag should parse");
        visitor(borrowed.as_tag())
    }

    fn parse_stack(compound: NbtCompound) -> Option<ItemStack> {
        with_borrowed_tag(NbtTag::Compound(compound), ItemStack::from_nbt_tag)
    }

    fn stone_stack_nbt() -> NbtCompound {
        let mut compound = NbtCompound::new();
        compound.insert("id", "minecraft:stone");
        compound
    }

    fn untrusted_stack_bytes(
        count: i32,
        component: Option<(&steel_utils::Identifier, Vec<u8>)>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        VarInt(count)
            .write(&mut bytes)
            .expect("test stack count should encode");
        VarInt(vanilla_items::STONE.id() as i32)
            .write(&mut bytes)
            .expect("test item id should encode");

        if let Some((component, value)) = component {
            VarInt(1)
                .write(&mut bytes)
                .expect("added component count should encode");
            VarInt(0)
                .write(&mut bytes)
                .expect("removed component count should encode");
            let component_id = REGISTRY
                .data_components
                .id_from_key(component)
                .expect("test component should be registered");
            VarInt(component_id as i32)
                .write(&mut bytes)
                .expect("component id should encode");
            VarInt(value.len() as i32)
                .write(&mut bytes)
                .expect("component length should encode");
            bytes.extend_from_slice(&value);
        } else {
            VarInt(0)
                .write(&mut bytes)
                .expect("added component count should encode");
            VarInt(0)
                .write(&mut bytes)
                .expect("removed component count should encode");
        }
        bytes
    }

    #[test]
    fn persistent_item_count_uses_vanilla_integer_codec() {
        init_vanilla_registry();
        let mut compound = stone_stack_nbt();
        compound.insert("count", 5.9_f64);
        assert_eq!(parse_stack(compound).map(|stack| stack.count()), Some(5));

        let mut compound = stone_stack_nbt();
        compound.insert("count", 100);
        assert!(parse_stack(compound).is_none());

        let mut compound = stone_stack_nbt();
        compound.insert("count", "5");
        assert!(parse_stack(compound).is_none());
    }

    #[test]
    fn malformed_present_component_patch_rejects_the_item_stack() {
        init_vanilla_registry();
        let mut components = NbtCompound::new();
        components.insert("minecraft:max_stack_size", 0);
        let mut compound = stone_stack_nbt();
        compound.insert("components", components);

        assert!(parse_stack(compound).is_none());
    }

    #[test]
    fn shrunk_to_empty_stack_exposes_no_components() {
        init_vanilla_registry();
        let mut totem = ItemStack::new(&vanilla_items::TOTEM_OF_UNDYING);
        assert!(totem.has(DEATH_PROTECTION));

        totem.shrink(1);

        assert!(!totem.has(DEATH_PROTECTION));
        assert!(totem.get(DEATH_PROTECTION).is_none());
    }

    #[test]
    fn component_patches_stay_sanitized_against_the_item_prototype() {
        init_vanilla_registry();
        let mut patch = crate::data_components::DataComponentPatch::new();
        patch.set(MAX_STACK_SIZE, 64);
        patch.remove(CUSTOM_DATA);
        let mut stack = ItemStack::with_count_and_patch(&vanilla_items::STONE, 1, patch);
        assert!(stack.components_patch().is_empty());

        stack.set(MAX_STACK_SIZE, 16);
        assert_eq!(stack.components_patch().len(), 1);
        stack.set(MAX_STACK_SIZE, 64);
        assert!(stack.components_patch().is_empty());

        stack.remove(MAX_STACK_SIZE);
        assert!(stack.components_patch().is_removed(&MAX_STACK_SIZE.key));
        stack.set(MAX_STACK_SIZE, 64);
        assert!(stack.components_patch().is_empty());

        stack.remove(CUSTOM_DATA);
        assert!(stack.components_patch().is_empty());

        stack.set(MAX_STACK_SIZE, 16);
        stack.set_item(&vanilla_items::ENDER_PEARL.key);
        assert_eq!(stack.max_stack_size(), 16);
        assert!(stack.components_patch().is_empty());
    }

    #[test]
    fn strict_validation_ignores_components_of_empty_stacks() {
        init_vanilla_registry();
        let mut patch = crate::data_components::DataComponentPatch::new();
        patch.set(MAX_DAMAGE, 1);
        let stack = ItemStack::with_count_and_patch(&vanilla_items::STONE, 0, patch);

        assert!(stack.is_empty());
        assert!(stack.validate_strict().is_ok());
    }

    #[test]
    fn default_count_is_always_present_in_persistent_encoding() {
        init_vanilla_registry();
        let stack = ItemStack::new(&vanilla_items::STONE);
        let NbtTag::Compound(compound) = stack.to_nbt_tag_ref() else {
            panic!("item stack should encode as a compound");
        };

        assert_eq!(compound.get("count"), Some(&NbtTag::Int(1)));
    }

    #[test]
    fn untrusted_stack_rejects_direct_jukebox_holders() {
        init_vanilla_registry();
        let mut component_bytes = Vec::new();
        VarInt(0)
            .write(&mut component_bytes)
            .expect("direct holder discriminator should encode");
        let bytes = untrusted_stack_bytes(1, Some((&JUKEBOX_PLAYABLE.key, component_bytes)));

        assert!(ItemStack::read_untrusted(&mut Cursor::new(bytes.as_slice())).is_err());
    }

    #[test]
    fn untrusted_stack_accepts_persistable_registry_holders() {
        init_vanilla_registry();
        let reference = JukeboxPlayable::new(&vanilla_jukebox_songs::CAT);
        let mut component_bytes = Vec::new();
        reference
            .write(&mut component_bytes)
            .expect("registry holder should have a network representation");
        let bytes = untrusted_stack_bytes(1, Some((&JUKEBOX_PLAYABLE.key, component_bytes)));

        let stack = ItemStack::read_untrusted(&mut Cursor::new(bytes.as_slice()))
            .expect("persistable untrusted stack should decode");
        assert_eq!(stack.get(JUKEBOX_PLAYABLE), Some(&reference));
    }

    #[test]
    fn untrusted_stack_uses_persistent_count_range() {
        init_vanilla_registry();
        let bytes = untrusted_stack_bytes(100, None);

        assert!(ItemStack::read_untrusted(&mut Cursor::new(bytes.as_slice())).is_err());
    }

    #[test]
    fn untrusted_stack_validates_component_persistent_constraints() {
        init_vanilla_registry();
        let mut component_bytes = Vec::new();
        VarInt(0)
            .write(&mut component_bytes)
            .expect("max stack size should encode on the network");
        let bytes = untrusted_stack_bytes(
            1,
            Some((
                &crate::data_components::vanilla_components::MAX_STACK_SIZE.key,
                component_bytes,
            )),
        );

        assert!(ItemStack::read_untrusted(&mut Cursor::new(bytes.as_slice())).is_err());
    }

    #[test]
    fn save_omits_invalid_component_value_but_keeps_present_patch_field() {
        init_vanilla_registry();
        let mut stack = ItemStack::new(&vanilla_items::STONE);
        stack.set(MAX_STACK_SIZE, 0);

        assert!(stack.validate_persistent_encoding().is_err());
        let NbtTag::Compound(compound) = stack.to_nbt_tag_ref() else {
            panic!("item stack should still encode as a compound");
        };
        assert_eq!(
            compound.string("id").map(|value| value.to_str()),
            Some("minecraft:stone".into())
        );
        assert!(
            compound
                .compound("components")
                .is_some_and(simdnbt::owned::NbtCompound::is_empty)
        );
    }

    #[test]
    fn toggle_tooltips_updates_the_typed_display_component() {
        init_vanilla_registry();
        let mut stack = ItemStack::new(&vanilla_items::STONE);

        stack.toggle_tooltips(&[(LORE.key, false)]);
        let display = stack
            .get(TOOLTIP_DISPLAY)
            .expect("tooltip display should be set");
        assert!(!display.shows(LORE));

        stack.toggle_tooltips(&[(LORE.key, true)]);
        assert!(
            stack
                .get(TOOLTIP_DISPLAY)
                .expect("tooltip display should remain set")
                .shows(LORE)
        );
    }

    #[test]
    fn set_custom_data_recursively_merges_and_removes_empty_values() {
        init_vanilla_registry();
        let mut stack = ItemStack::new(&vanilla_items::STONE);
        let empty = CustomData::default();
        stack.set_custom_data(&empty);
        assert!(stack.get(CUSTOM_DATA).is_none());

        let mut nested = NbtCompound::new();
        nested.insert("kept", 1);
        nested.insert("changed", 1);
        let mut first = NbtCompound::new();
        first.insert("nested", nested);
        stack.set_custom_data(
            &CustomData::try_from_compound(first).expect("first value should be valid"),
        );

        let mut nested = NbtCompound::new();
        nested.insert("changed", 2);
        let mut second = NbtCompound::new();
        second.insert("nested", nested);
        stack.set_custom_data(
            &CustomData::try_from_compound(second).expect("second value should be valid"),
        );

        let nested = stack
            .get(CUSTOM_DATA)
            .and_then(|data| data.as_compound().compound("nested"))
            .expect("nested custom data should remain");
        assert_eq!(nested.int("kept"), Some(1));
        assert_eq!(nested.int("changed"), Some(2));
    }
}
