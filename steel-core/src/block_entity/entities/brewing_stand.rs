//! Brewing stand block entity implementation.
//!
//! Vanilla parity with `BrewingStandBlockEntity`: five slots (three bottles,
//! ingredient, fuel), fuel refilled from blaze powder, a 400-tick brew cycle
//! resolved against the `minecraft:brewing` recipe set, and `HAS_BOTTLE_*`
//! block-state updates mirroring bottle occupancy.

use std::array;
use std::mem;
use std::sync::{Arc, Weak};

use simdnbt::ToNbtTag;
use simdnbt::borrow::{BaseNbtCompound as BorrowedNbtCompound, NbtCompound as NbtCompoundView};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::blocks::block_state_ext::BlockStateExt;
use steel_registry::blocks::properties::{BlockStateProperties, Direction};
use steel_registry::data_components::vanilla_components;
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::recipe::{BrewingInput, vanilla_recipe_types};
use steel_registry::{
    REGISTRY, RegistryExt as _, TaggedRegistryExt, level_events, vanilla_block_entity_types,
    vanilla_blocks, vanilla_context_float_providers, vanilla_context_int_providers,
    vanilla_item_tags, vanilla_items,
};
use steel_utils::types::UpdateFlags;
use steel_utils::{
    BlockPos, BlockStateId, DowncastType, DowncastTypeKey, Identifier, locks::SyncMutex,
};

use crate::block_entity::{BlockEntity, BlockEntityBase};
use crate::inventory::container::Container;
use crate::inventory::lock::{ContainerRef, SharedContainer};
use crate::world::World;

/// Number of slots in a brewing stand (3 bottles + ingredient + fuel).
pub const BREWING_STAND_SLOTS: usize = 5;
/// First bottle slot.
pub const SLOT_BOTTLE_START: usize = 0;
/// Ingredient slot.
pub const SLOT_INGREDIENT: usize = 3;
/// Fuel slot.
pub const SLOT_FUEL: usize = 4;

const SLOTS_FOR_UP: &[usize] = &[SLOT_INGREDIENT];
const SLOTS_FOR_DOWN: &[usize] = &[SLOT_BOTTLE_START, 1, 2, SLOT_INGREDIENT];
const SLOTS_FOR_SIDES: &[usize] = &[SLOT_BOTTLE_START, 1, 2, SLOT_FUEL];

/// Fuel uses granted by vanilla's `minecraft:brewing/uses_default` provider.
///
/// The `minecraft:brewing_fuel` component names this provider key; the value is
/// read from the context-int-provider registry so a datapack override is honored.
fn default_fuel_uses() -> i32 {
    vanilla_context_int_providers::BREWING_USES_DEFAULT
        .provider
        .as_constant()
        .expect("minecraft:brewing/uses_default is a constant provider")
}

/// Brew time in ticks (`BrewingStandBlockEntity.BREWING_TIME_SECONDS * 20`).
const DEFAULT_BREW_TIME: i32 = 400;

/// Vanilla's `minecraft:brewing/speed_default` provider value.
///
/// Resolved through the context-float-provider registry for the same reason as
/// [`default_fuel_uses`].
fn default_speed_multiplier() -> f32 {
    vanilla_context_float_providers::BREWING_SPEED_DEFAULT
        .provider
        .as_constant()
        .expect("minecraft:brewing/speed_default is a constant provider")
}

/// Resolves a `minecraft:brewing_fuel` int-provider key to its value.
///
/// Mirrors vanilla, where the component names a context provider and the value
/// is resolved from the registry, so a datapack override of the provider value
/// is honored. Brewing fuel providers are context-free constants in vanilla.
fn resolve_fuel_uses(key: &Identifier) -> Option<i32> {
    if let Some(entry) = REGISTRY.context_int_providers.by_key(key) {
        if let Some(value) = entry.provider.as_constant() {
            return Some(value);
        }
        log::warn!(
            "brewing fuel int provider {} is not a constant provider",
            key.path
        );
        return None;
    }
    (key.path.as_ref() == "brewing/uses_default").then(default_fuel_uses)
}

/// Resolves a `minecraft:brewing_fuel` float-provider key to its value.
///
/// See [`resolve_fuel_uses`].
fn resolve_speed_multiplier(key: &Identifier) -> Option<f32> {
    if let Some(entry) = REGISTRY.context_float_providers.by_key(key) {
        if let Some(value) = entry.provider.as_constant() {
            return Some(value);
        }
        log::warn!(
            "brewing fuel float provider {} is not a constant provider",
            key.path
        );
        return None;
    }
    (key.path.as_ref() == "brewing/speed_default").then(default_speed_multiplier)
}

/// Brewing stand block entity.
///
/// Holds five slots (bottles 0-2, ingredient 3, fuel 4) plus brew progress
/// (`BrewTime`), fuel uses (`Fuel`), and a speed multiplier carried through
/// saves.
pub struct BrewingStandBlockEntity {
    base: Arc<BlockEntityBase>,
    container: Arc<SyncMutex<BrewingStandContainer>>,
    container_ref: ContainerRef,
}

/// Independently lockable brewing stand inventory and progress data.
pub struct BrewingStandContainer {
    items: [ItemStack; BREWING_STAND_SLOTS],
    brew_time: i32,
    total_brew_time: i32,
    fuel: i32,
    total_fuel: i32,
    speed_multiplier: f32,
    last_potion_bits: [bool; 3],
    /// Item that started the running batch, used to abort on ingredient changes.
    ingredient: Option<ItemRef>,
    /// Remainders that must be dropped outside the container lock.
    pending_drops: Vec<ItemStack>,
    /// Whether the last tick mutated persisted state.
    dirty: bool,
}

// SAFETY: This key is owned by Steel and uniquely identifies `BrewingStandBlockEntity`.
unsafe impl DowncastType for BrewingStandBlockEntity {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:block_entity/brewing_stand");
}

// SAFETY: This key is owned by Steel and uniquely identifies the independently
// lockable inventory data used by a brewing stand block entity.
unsafe impl DowncastType for BrewingStandContainer {
    const TYPE_KEY: DowncastTypeKey = DowncastTypeKey::new("steel:container/brewing_stand");
}

/// Side effects produced by one container tick that must run unlocked.
enum BrewingTick {
    /// Nothing to publish.
    Idle,
    /// Bottle occupancy changed; the caller updates `HAS_BOTTLE_*`.
    BottleBits([bool; 3]),
    /// A batch finished brewing without changing bottle occupancy.
    Brewed,
    /// A batch finished brewing and the bottle occupancy changed.
    BrewedWithBits([bool; 3]),
}

impl BrewingStandContainer {
    fn new() -> Self {
        Self {
            items: array::from_fn(|_| ItemStack::empty()),
            brew_time: 0,
            total_brew_time: DEFAULT_BREW_TIME,
            fuel: 0,
            total_fuel: default_fuel_uses(),
            speed_multiplier: default_speed_multiplier(),
            last_potion_bits: [false; 3],
            ingredient: None,
            pending_drops: Vec::new(),
            dirty: false,
        }
    }

    /// Data-slot values `[brew_time, fuel, total_brew_time, total_fuel]`.
    #[must_use]
    pub(crate) const fn data(&self) -> [i16; 4] {
        [
            self.brew_time as i16,
            self.fuel as i16,
            self.total_brew_time as i16,
            self.total_fuel as i16,
        ]
    }

    fn potion_bits(items: &[ItemStack; BREWING_STAND_SLOTS]) -> [bool; 3] {
        [0, 1, 2].map(|slot| !items[slot].is_empty())
    }

    /// Resolves the fuel uses and speed multiplier for a fuel stack.
    ///
    /// Vanilla resolves these through the `minecraft:brewing_fuel` component's
    /// context int/float providers; the component names the provider keys and
    /// the values come from the context-provider registries.
    fn fuel_uses(fuel: &ItemStack) -> Option<(i32, f32)> {
        let component = fuel.get(vanilla_components::BREWING_FUEL)?;
        let uses = resolve_fuel_uses(&component.uses)?;
        let speed = resolve_speed_multiplier(&component.speed_multiplier)?;
        Some((uses, speed))
    }

    /// Whether `stack` is accepted by the ingredient slot, i.e. it is the
    /// reagent of at least one `minecraft:brewing` recipe.
    #[must_use]
    pub fn is_brewing_reagent(stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return false;
        }
        REGISTRY
            .recipes
            .by_type(&vanilla_recipe_types::BREWING)
            .iter()
            .any(|recipe| recipe.data().reagent.test(stack))
    }

    /// Whether `stack` may be placed into one of the bottle slots.
    ///
    /// Mirrors vanilla `PotionIngredient.isPotionInput` for the part Steel can
    /// evaluate without a recipe-property registry: the `brewing_potion_inputs`
    /// item tag (potion, splash potion, lingering potion, glass bottle).
    #[must_use]
    pub fn is_potion_input(stack: &ItemStack) -> bool {
        !stack.is_empty()
            && REGISTRY.items.is_in_tag(
                stack.item,
                &vanilla_item_tags::ItemTag::BREWING_POTION_INPUTS,
            )
    }

    /// Whether `stack` is brewing fuel, i.e. it carries the
    /// `minecraft:brewing_fuel` component.
    #[must_use]
    pub fn is_brewing_fuel(stack: &ItemStack) -> bool {
        !stack.is_empty() && stack.get(vanilla_components::BREWING_FUEL).is_some()
    }

    fn is_brewable(&self) -> bool {
        let ingredient = &self.items[SLOT_INGREDIENT];
        if ingredient.is_empty() {
            return false;
        }
        (SLOT_BOTTLE_START..SLOT_BOTTLE_START + 3).any(|slot| {
            !self.items[slot].is_empty()
                && REGISTRY
                    .recipes
                    .find_match(
                        &vanilla_recipe_types::BREWING,
                        &BrewingInput::new(self.items[slot].clone(), ingredient.clone()),
                    )
                    .is_some()
        })
    }

    /// Runs one vanilla `BrewingStandBlockEntity.serverTick` step.
    fn tick(&mut self) -> BrewingTick {
        self.dirty = false;
        // Refuel from a brewing-fuel item when exhausted.
        if self.fuel <= 0
            && let Some((uses, speed)) = Self::fuel_uses(&self.items[SLOT_FUEL])
        {
            self.fuel = uses;
            self.total_fuel = uses;
            self.speed_multiplier = speed;
            self.dirty = true;
            let fuel_item = self.items[SLOT_FUEL].item();
            self.items[SLOT_FUEL].shrink(1);
            if self.items[SLOT_FUEL].is_empty() {
                let remainder = fuel_item.get_crafting_remainder();
                if !remainder.is_empty() {
                    self.items[SLOT_FUEL] = remainder;
                }
            } else {
                let remainder = fuel_item.get_crafting_remainder();
                if !remainder.is_empty() {
                    self.pending_drops.push(remainder);
                }
            }
        }

        let brewable = self.is_brewable();
        let ingredient_matches = self
            .ingredient
            .is_some_and(|item| self.items[SLOT_INGREDIENT].is(item));
        let mut brewed = false;
        if self.brew_time > 0 {
            // Vanilla marks the entity changed on every tick of a running batch,
            // even when the brew is merely counting down.
            self.brew_time -= 1;
            self.dirty = true;
            if self.brew_time == 0 {
                // Vanilla brews with whatever reagent the slot holds now; only the
                // recipe match is re-checked at completion time.
                if brewable {
                    self.brew();
                    brewed = true;
                } else {
                    self.brew_time = 0;
                    self.ingredient = None;
                }
            } else if !brewable || !ingredient_matches {
                self.brew_time = 0;
                self.ingredient = None;
            }
        } else if brewable && self.fuel > 0 {
            let speed = if self.speed_multiplier > 0.0 {
                self.speed_multiplier
            } else {
                default_speed_multiplier()
            };
            self.fuel -= 1;
            self.brew_time = (DEFAULT_BREW_TIME as f32 / speed).ceil() as i32;
            self.total_brew_time = self.brew_time;
            self.ingredient = Some(self.items[SLOT_INGREDIENT].item());
            self.dirty = true;
        }

        let bits = Self::potion_bits(&self.items);
        if bits != self.last_potion_bits {
            self.last_potion_bits = bits;
            if brewed {
                BrewingTick::BrewedWithBits(bits)
            } else {
                BrewingTick::BottleBits(bits)
            }
        } else if brewed {
            BrewingTick::Brewed
        } else {
            BrewingTick::Idle
        }
    }

    /// Applies the brewing recipe to every bottle and consumes one ingredient.
    fn brew(&mut self) {
        let ingredient = self.items[SLOT_INGREDIENT].clone();
        for bottle in &mut self.items[SLOT_BOTTLE_START..SLOT_BOTTLE_START + 3] {
            let input = BrewingInput::new(bottle.clone(), ingredient.clone());
            if let Some(recipe) = REGISTRY
                .recipes
                .find_match(&vanilla_recipe_types::BREWING, &input)
            {
                *bottle = recipe.data().assemble();
            }
        }
        let ingredient_item = self.items[SLOT_INGREDIENT].item();
        self.items[SLOT_INGREDIENT].shrink(1);
        let remainder = ingredient_item.get_crafting_remainder();
        if !remainder.is_empty() {
            if self.items[SLOT_INGREDIENT].is_empty() {
                self.items[SLOT_INGREDIENT] = remainder;
            } else {
                self.pending_drops.push(remainder);
            }
        }
        self.brew_time = 0;
        self.ingredient = None;
    }
}

impl Container for BrewingStandContainer {
    fn items(&self) -> &[ItemStack] {
        &self.items
    }

    fn items_mut(&mut self) -> &mut [ItemStack] {
        &mut self.items
    }

    fn get_container_size(&self) -> usize {
        BREWING_STAND_SLOTS
    }

    fn set_item(&mut self, slot: usize, mut stack: ItemStack) {
        if slot >= BREWING_STAND_SLOTS {
            return;
        }
        let max_size = self.get_max_stack_size_for_item(&stack);
        if !stack.is_empty() && stack.count() > max_size {
            stack.set_count(max_size);
        }
        self.items[slot] = stack;
    }

    fn get_max_stack_size(&self) -> i32 {
        64
    }

    fn set_changed(&mut self) {}

    fn can_place_item(&self, slot: usize, stack: &ItemStack) -> bool {
        match slot {
            SLOT_FUEL => stack.get(vanilla_components::BREWING_FUEL).is_some(),
            SLOT_INGREDIENT => Self::is_brewing_reagent(stack),
            SLOT_BOTTLE_START..=2 => Self::is_potion_input(stack) && self.items[slot].is_empty(),
            _ => false,
        }
    }

    fn slots_for_face(&self, direction: Direction) -> Option<&'static [usize]> {
        Some(match direction {
            Direction::Up => SLOTS_FOR_UP,
            Direction::Down => SLOTS_FOR_DOWN,
            _ => SLOTS_FOR_SIDES,
        })
    }

    fn can_take_item_through_face(
        &self,
        slot: usize,
        stack: &ItemStack,
        _direction: Direction,
    ) -> bool {
        // Vanilla `BrewingStandBlockEntity.canTakeItemThroughFace` only special-cases
        // the reagent slot, for every face that exposes it.
        if slot != SLOT_INGREDIENT {
            return true;
        }
        stack.is(&vanilla_items::GLASS_BOTTLE)
    }
}

impl BrewingStandBlockEntity {
    /// Creates a new brewing stand block entity.
    #[must_use]
    pub fn new(level: Weak<World>, pos: BlockPos, state: BlockStateId) -> Self {
        let base = Arc::new(BlockEntityBase::new(
            &vanilla_block_entity_types::BREWING_STAND,
            level,
            pos,
            state,
        ));
        let container = Arc::new(SyncMutex::new(BrewingStandContainer::new()));
        let shared_container: SharedContainer =
            Arc::<SyncMutex<BrewingStandContainer>>::clone(&container);
        Self {
            container_ref: ContainerRef::owned_by_block_entity(shared_container, Arc::clone(&base)),
            base,
            container,
        }
    }

    /// Tick entry point invoked from the block ticker; mirrors vanilla
    /// `BrewingStandBlockEntity.serverTick`.
    pub(crate) fn server_tick(&self, world: &Arc<World>) {
        let (tick, drops, dirty) = {
            let mut state = self.container.lock();
            let tick = state.tick();
            let dirty = state.dirty;
            state.dirty = false;
            (tick, mem::take(&mut state.pending_drops), dirty)
        };
        if dirty {
            self.base.set_changed();
        }
        for item in drops {
            world.drop_item_stack(self.get_block_pos(), item);
        }
        match tick {
            BrewingTick::Idle => {}
            BrewingTick::BottleBits(bits) => self.sync_bottle_states(world, bits),
            BrewingTick::Brewed => {
                world.level_event(
                    level_events::SOUND_BREWING_STAND_BREW,
                    self.get_block_pos(),
                    0,
                    None,
                );
            }
            BrewingTick::BrewedWithBits(bits) => {
                world.level_event(
                    level_events::SOUND_BREWING_STAND_BREW,
                    self.get_block_pos(),
                    0,
                    None,
                );
                self.sync_bottle_states(world, bits);
            }
        }
    }

    /// Applies vanilla's `HAS_BOTTLE_*` state update for the given occupancy.
    fn sync_bottle_states(&self, world: &Arc<World>, bits: [bool; 3]) {
        let state = self.base.block_state();
        if state.get_block() != &vanilla_blocks::BREWING_STAND {
            return;
        }
        let state = state
            .set_value(&BlockStateProperties::HAS_BOTTLE_0, bits[0])
            .set_value(&BlockStateProperties::HAS_BOTTLE_1, bits[1])
            .set_value(&BlockStateProperties::HAS_BOTTLE_2, bits[2]);
        world.set_block(self.get_block_pos(), state, UpdateFlags::UPDATE_CLIENTS);
    }

    fn load_state(&self, nbt: &BorrowedNbtCompound<'_>) {
        let nbt: NbtCompoundView<'_, '_> = nbt.into();
        let mut state = self.container.lock();
        state.items = array::from_fn(|_| ItemStack::empty());
        if let Some(items) = nbt.list("Items").and_then(|list| list.compounds()) {
            for compound in items {
                let Some(slot) = compound.byte("Slot").map(|slot| slot as usize) else {
                    continue;
                };
                if slot < BREWING_STAND_SLOTS
                    && let Some(stack) = ItemStack::from_borrowed_compound(&compound)
                {
                    state.items[slot] = stack;
                }
            }
        }
        state.brew_time = nbt.int("BrewTime").unwrap_or(0);
        state.total_brew_time = nbt.int("total_brew_time").unwrap_or(DEFAULT_BREW_TIME);
        state.fuel = nbt.int("Fuel").unwrap_or(0);
        state.total_fuel = nbt.int("total_fuel").unwrap_or_else(default_fuel_uses);
        state.speed_multiplier = nbt
            .float("speed_multiplier")
            .unwrap_or_else(default_speed_multiplier);
        state.ingredient = if state.brew_time > 0 {
            Some(state.items[SLOT_INGREDIENT].item())
        } else {
            None
        };
        state.last_potion_bits = BrewingStandContainer::potion_bits(&state.items);
    }

    fn save_state(&self, nbt: &mut NbtCompound) {
        let state = self.container.lock();
        nbt.insert("BrewTime", state.brew_time);
        nbt.insert("total_brew_time", state.total_brew_time);
        nbt.insert("Fuel", state.fuel);
        nbt.insert("total_fuel", state.total_fuel);
        nbt.insert("speed_multiplier", state.speed_multiplier);

        let mut items = Vec::new();
        for (slot, stack) in state.items.iter().enumerate() {
            if !stack.is_empty()
                && let NbtTag::Compound(mut item) = stack.clone().to_nbt_tag()
            {
                item.insert("Slot", slot as i8);
                items.push(item);
            }
        }
        nbt.insert("Items", NbtList::Compound(items));
    }
}

impl BlockEntity for BrewingStandBlockEntity {
    fn base(&self) -> &BlockEntityBase {
        &self.base
    }

    fn pre_remove_side_effects(&self, pos: BlockPos, _state: BlockStateId) {
        let items = {
            let mut state = self.container.lock();
            mem::replace(&mut state.items, array::from_fn(|_| ItemStack::empty()))
        };
        let Some(world) = self.get_level() else {
            return;
        };
        for item in items {
            world.drop_item_stack(pos, item);
        }
    }

    fn load_additional(&self, nbt: &BorrowedNbtCompound<'_>) {
        self.load_state(nbt);
    }

    fn save_additional(&self, nbt: &mut NbtCompound) {
        self.save_state(nbt);
    }

    fn tick(&self, world: &Arc<World>) {
        self.server_tick(world);
    }

    fn container_ref(&self) -> Option<ContainerRef> {
        Some(self.container_ref.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Weak;

    use simdnbt::borrow::read_compound as read_borrowed_compound;
    use steel_registry::data_components::{DataComponentPatch, PotionContents};
    use steel_registry::potion::Potion;
    use steel_registry::{
        init_vanilla_registry, registry_reference::RegistryReference, vanilla_blocks,
        vanilla_items, vanilla_potions,
    };

    use super::*;

    /// Plain potion with a `potion_contents` patch, as dropped items carry.
    fn potion_bottle(potion: RegistryReference<Potion>) -> ItemStack {
        let mut patch = DataComponentPatch::new();
        patch.set(
            vanilla_components::POTION_CONTENTS,
            PotionContents::new(Some(potion), None, Vec::new(), None),
        );
        ItemStack::with_count_and_patch(&vanilla_items::POTION, 1, patch)
    }

    /// A plain water bottle.
    fn water_bottle() -> ItemStack {
        potion_bottle(RegistryReference::new(&vanilla_potions::WATER))
    }

    /// A plain awkward potion.
    fn awkward_bottle() -> ItemStack {
        potion_bottle(RegistryReference::new(&vanilla_potions::AWKWARD))
    }

    fn test_stand() -> BrewingStandBlockEntity {
        init_vanilla_registry();
        BrewingStandBlockEntity::new(
            Weak::new(),
            BlockPos::new(1, 2, 3),
            vanilla_blocks::BREWING_STAND.default_state(),
        )
    }

    #[test]
    fn blaze_powder_provides_default_fuel() {
        init_vanilla_registry();
        let fuel = ItemStack::new(&vanilla_items::BLAZE_POWDER);
        assert_eq!(
            BrewingStandContainer::fuel_uses(&fuel),
            Some((default_fuel_uses(), default_speed_multiplier()))
        );
    }

    #[test]
    fn bottles_only_accept_potion_inputs_into_empty_slots() {
        init_vanilla_registry();
        let stand = test_stand();
        let mut state = stand.container.lock();
        assert!(state.can_place_item(0, &water_bottle()));
        assert!(state.can_place_item(0, &ItemStack::new(&vanilla_items::GLASS_BOTTLE)));
        assert!(!state.can_place_item(0, &ItemStack::new(&vanilla_items::STONE)));
        state.set_item(0, water_bottle());
        assert!(!state.can_place_item(0, &water_bottle()));
    }

    #[test]
    fn fuel_slot_requires_brewing_fuel_component() {
        init_vanilla_registry();
        let stand = test_stand();
        let state = stand.container.lock();
        assert!(state.can_place_item(SLOT_FUEL, &ItemStack::new(&vanilla_items::BLAZE_POWDER)));
        assert!(!state.can_place_item(SLOT_FUEL, &ItemStack::new(&vanilla_items::COAL)));
    }

    #[test]
    fn water_to_awkward_consumes_nether_wart() {
        init_vanilla_registry();
        let stand = test_stand();
        {
            let mut state = stand.container.lock();
            state.set_item(0, water_bottle());
            state.set_item(SLOT_INGREDIENT, ItemStack::new(&vanilla_items::NETHER_WART));
            state.fuel = 20;
        }

        // The first tick starts the batch; the next 400 finish it.
        for _ in 0..=DEFAULT_BREW_TIME {
            let _ = stand.container.lock().tick();
        }

        let state = stand.container.lock();
        assert!(state.items[SLOT_INGREDIENT].is_empty());
        let contents = state.items[0]
            .get(vanilla_components::POTION_CONTENTS)
            .expect("brewed bottle has potion contents");
        assert_eq!(
            contents.potion(),
            Some(RegistryReference::new(&vanilla_potions::AWKWARD))
        );
        assert_eq!(state.fuel, 19);
    }

    #[test]
    fn awkward_to_healing_uses_the_registered_output() {
        init_vanilla_registry();
        let stand = test_stand();
        {
            let mut state = stand.container.lock();
            state.set_item(0, awkward_bottle());
            state.set_item(
                SLOT_INGREDIENT,
                ItemStack::new(&vanilla_items::GLISTERING_MELON_SLICE),
            );
            state.fuel = 20;
        }

        for _ in 0..=DEFAULT_BREW_TIME {
            let _ = stand.container.lock().tick();
        }

        let state = stand.container.lock();
        assert!(
            state.items[SLOT_INGREDIENT].is_empty(),
            "the reagent is consumed"
        );
        assert_eq!(state.fuel, 19, "exactly one fuel use is consumed");
        assert!(
            state.items[0].is(&vanilla_items::POTION),
            "the bottle stays a potion"
        );
        assert_eq!(
            state.items[0]
                .get(vanilla_components::POTION_CONTENTS)
                .and_then(PotionContents::potion),
            Some(RegistryReference::new(&vanilla_potions::HEALING)),
            "the potion comes from the brewing recipe's output"
        );
    }

    #[test]
    fn unmatched_reagent_does_not_start_a_batch() {
        init_vanilla_registry();
        let stand = test_stand();
        {
            let mut state = stand.container.lock();
            // Phantom membrane only extends awkward potions, so it cannot brew
            // a water bottle.
            state.set_item(0, water_bottle());
            state.set_item(
                SLOT_INGREDIENT,
                ItemStack::new(&vanilla_items::PHANTOM_MEMBRANE),
            );
            state.fuel = 20;
        }

        for _ in 0..=DEFAULT_BREW_TIME {
            let _ = stand.container.lock().tick();
        }

        // Vanilla `serverTick` never starts the batch: the bottle, the membrane
        // and the fuel are all untouched, and no brew is scheduled.
        let state = stand.container.lock();
        assert_eq!(state.fuel, 20, "an invalid combination consumes no fuel");
        assert_eq!(state.brew_time, 0, "an invalid combination starts no batch");
        assert!(
            state.items[0]
                .get(vanilla_components::POTION_CONTENTS)
                .is_some_and(|contents| contents.potion()
                    == Some(RegistryReference::new(&vanilla_potions::WATER)))
        );
        assert!(state.items[SLOT_INGREDIENT].is(&vanilla_items::PHANTOM_MEMBRANE));
    }

    #[test]
    fn non_brewable_bottle_input_does_not_start_a_batch() {
        init_vanilla_registry();
        let stand = test_stand();
        {
            let mut state = stand.container.lock();
            // Glass bottles are potion inputs, but no brewing recipe takes one.
            state.set_item(0, ItemStack::new(&vanilla_items::GLASS_BOTTLE));
            state.set_item(SLOT_INGREDIENT, ItemStack::new(&vanilla_items::NETHER_WART));
            state.fuel = 20;
        }

        for _ in 0..=DEFAULT_BREW_TIME {
            let _ = stand.container.lock().tick();
        }

        let state = stand.container.lock();
        assert_eq!(state.fuel, 20, "an invalid combination consumes no fuel");
        assert_eq!(state.brew_time, 0, "an invalid combination starts no batch");
        assert!(state.items[0].is(&vanilla_items::GLASS_BOTTLE));
        assert!(state.items[SLOT_INGREDIENT].is(&vanilla_items::NETHER_WART));
    }

    #[test]
    fn save_round_trips_progress() {
        init_vanilla_registry();
        let stand = test_stand();
        {
            let mut state = stand.container.lock();
            state.fuel = 7;
            state.brew_time = 12;
            state.total_brew_time = DEFAULT_BREW_TIME;
            state.set_item(0, water_bottle());
        }
        let mut nbt = NbtCompound::new();
        stand.save_state(&mut nbt);
        assert_eq!(nbt.get("Fuel").and_then(NbtTag::int), Some(7));
        assert_eq!(nbt.get("BrewTime").and_then(NbtTag::int), Some(12));

        let fresh = test_stand();
        let mut bytes = Vec::new();
        nbt.write(&mut bytes);
        let borrowed =
            read_borrowed_compound(&mut Cursor::new(bytes.as_slice())).expect("test NBT parses");
        fresh.load_state(&borrowed);
        let state = fresh.container.lock();
        assert_eq!(state.fuel, 7);
        assert_eq!(state.brew_time, 12);
        let contents = state.items[0]
            .get(vanilla_components::POTION_CONTENTS)
            .expect("loaded bottle has potion contents");
        assert_eq!(
            contents.potion(),
            Some(RegistryReference::new(&vanilla_potions::WATER))
        );
    }
}
