//! Loom menu.
//!
//! Slot layout (40 total), matching vanilla `LoomMenu`:
//! - Slot 0: banner
//! - Slot 1: dye
//! - Slot 2: pattern
//! - Slot 3: result
//! - Slots 4-39: the player's 36 inventory slots

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::{
    REGISTRY, RegistryHolder, RegistryHolderSet, TaggedRegistryExt,
    banner_pattern::BannerPattern,
    data_components::vanilla_components::{
        BANNER_PATTERNS, BannerPatternLayer, BannerPatternLayers, DYE, PROVIDES_BANNER_PATTERNS,
    },
    item_stack::ItemStack,
    sound_events,
    vanilla_banner_pattern_tags::BannerPatternTag,
    vanilla_blocks,
    vanilla_item_tags::ItemTag,
    vanilla_menu_types,
};
use steel_utils::{
    BlockPos,
    locks::{IntoShared, Shared},
};

use crate::{
    inventory::{
        container::{ResultContainer, SimpleContainer},
        lock::ContainerId,
        prelude::*,
        slots::ResultHandler,
    },
    player::player_inventory::{PlayerInventory, Prediction},
    world::World,
};

/// Builds the loom menu.
#[must_use]
pub fn loom(
    inventory: Shared<PlayerInventory>,
    container_id: u8,
    pos: BlockPos,
    world: &Arc<World>,
) -> Menu {
    let input_container = SimpleContainer::new(3).into_shared();
    let result_container = ResultContainer::new().into_shared();

    let mut builder = MenuBuilder::new(&vanilla_menu_types::LOOM, container_id);

    let mut inputs = builder.split(Arc::clone(&input_container));
    let banner = builder.section_with(
        &mut inputs,
        1,
        SectionKind::restricted(|_, stack| is_banner(stack)),
    );
    let dye = builder.section_with(
        &mut inputs,
        1,
        SectionKind::restricted(|_, stack| is_dye_item(stack)),
    );
    let pattern = builder.section_with(
        &mut inputs,
        1,
        SectionKind::restricted(|_, stack| is_pattern_item(stack)),
    );
    let result = builder.result_slot(LoomResultHandler {
        input_container: Arc::clone(&input_container),
        result_container: Arc::clone(&result_container),
        block_pos: pos,
        world: Arc::clone(world),
        last_sound_time: Arc::new(AtomicI64::new(-1)),
    });
    let player = builder.player_inventory(&inventory);
    let selected_pattern = builder.data_slot(-1);

    builder.route_with_remainder_policy(
        result,
        player.all(),
        FillDirection::Backward,
        FakeResultRemainderPolicy::Discard,
    );
    builder.route(banner, player.all(), FillDirection::Forward);
    builder.route(dye, player.all(), FillDirection::Forward);
    builder.route(pattern, player.all(), FillDirection::Forward);
    builder.drain([banner, dye, pattern]);

    builder.build(LoomKind {
        input_container,
        result_container,
        block_pos: pos,
        world: Arc::clone(world),
        banner,
        dye,
        pattern,
        result,
        player,
        selected_pattern,
        selectable_patterns: Vec::new(),
    })
}

/// Returns whether the item stack is a banner. Vanilla gates this with
/// `instanceof BannerItem`, whose items are exactly the `minecraft:banners` tag.
fn is_banner(stack: &ItemStack) -> bool {
    !stack.is_empty() && REGISTRY.items.is_in_tag(stack.item(), &ItemTag::BANNERS)
}

/// Mirrors vanilla `LoomMenu.isDyeItem`.
fn is_dye_item(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && REGISTRY.items.is_in_tag(stack.item(), &ItemTag::LOOM_DYES)
        && stack.has(DYE)
}

/// Mirrors vanilla `LoomMenu.isPatternItem`.
fn is_pattern_item(stack: &ItemStack) -> bool {
    !stack.is_empty()
        && REGISTRY
            .items
            .is_in_tag(stack.item(), &ItemTag::LOOM_PATTERNS)
        && stack.has(PROVIDES_BANNER_PATTERNS)
}

/// Expands a `provides_banner_patterns` holder set into concrete pattern holders.
fn expand_patterns(set: &RegistryHolderSet<BannerPattern>) -> Vec<RegistryHolder<BannerPattern>> {
    match set {
        RegistryHolderSet::Direct(entries) => entries
            .iter()
            .map(|pattern| RegistryHolder::reference(*pattern))
            .collect(),
        RegistryHolderSet::Tag(tag) => REGISTRY
            .banner_patterns
            .get_tag(tag)
            .unwrap_or_default()
            .into_iter()
            .map(RegistryHolder::reference)
            .collect(),
    }
}

/// Result slot handler for the loom.
struct LoomResultHandler {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    last_sound_time: Arc<AtomicI64>,
}

impl ResultHandler for LoomResultHandler {
    fn result_container(&self) -> ContainerRef {
        ContainerRef::from(Arc::clone(&self.result_container))
    }

    fn dependencies(&self) -> Vec<ContainerRef> {
        vec![ContainerRef::from(Arc::clone(&self.input_container))]
    }

    fn update_result(&self, _guard: &mut ContainerLockGuard) {}

    fn on_result_taken(
        &self,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) -> Option<ItemStack> {
        let input_id = ContainerId::from_arc(&self.input_container);
        if let Some(input) = guard.get_mut(input_id) {
            input.remove_item(0, 1);
            input.remove_item(1, 1);
            input.set_changed();
        }

        let game_time = self.world.game_time();
        if self.last_sound_time.load(Ordering::Relaxed) != game_time {
            self.last_sound_time.store(game_time, Ordering::Relaxed);
            self.world.play_block_sound(
                &sound_events::UI_LOOM_TAKE_RESULT,
                self.block_pos,
                1.0,
                1.0,
                None,
            );
        }
        None
    }

    fn is_result_valid(&self, _guard: &ContainerLockGuard, _player: &Player) -> bool {
        true
    }
}

/// Per-menu loom state: inputs, result, and the selectable banner patterns.
pub struct LoomKind {
    input_container: Shared<SimpleContainer>,
    result_container: Shared<ResultContainer>,
    block_pos: BlockPos,
    world: Arc<World>,
    banner: Section,
    dye: Section,
    pattern: Section,
    result: Section,
    player: PlayerInventorySections,
    /// Client-facing selected pattern index.
    selected_pattern: DataSlot,
    /// Patterns offered by the current pattern item, or the item-free tag.
    selectable_patterns: Vec<RegistryHolder<BannerPattern>>,
}

// SAFETY: This Steel-owned key uniquely identifies the concrete menu kind
// within the process.
unsafe impl steel_utils::DowncastType for LoomKind {
    const TYPE_KEY: steel_utils::DowncastTypeKey =
        steel_utils::DowncastTypeKey::new("steel:menu/loom");
}

impl LoomKind {
    /// Mirrors vanilla `LoomMenu.getSelectablePatterns`.
    fn get_selectable_patterns(
        &self,
        pattern_stack: &ItemStack,
    ) -> Vec<RegistryHolder<BannerPattern>> {
        if pattern_stack.is_empty() {
            return REGISTRY
                .banner_patterns
                .get_tag(&BannerPatternTag::NO_ITEM_REQUIRED)
                .unwrap_or_default()
                .into_iter()
                .map(RegistryHolder::reference)
                .collect();
        }

        pattern_stack
            .get(PROVIDES_BANNER_PATTERNS)
            .map_or_else(Vec::new, expand_patterns)
    }

    fn is_valid_pattern_index(&self, index: i32) -> bool {
        index >= 0 && (index as usize) < self.selectable_patterns.len()
    }

    /// Mirrors vanilla `LoomMenu.setupResultSlot`: copies the banner and appends
    /// the selected pattern with the dye's color.
    fn setup_result_slot(
        &mut self,
        guard: &mut ContainerLockGuard,
        pattern: &RegistryHolder<BannerPattern>,
    ) {
        let input_id = ContainerId::from_arc(&self.input_container);
        let result_id = ContainerId::from_arc(&self.result_container);
        let Some([input, result]) = guard.get_disjoint_mut([input_id, result_id]) else {
            return;
        };

        let banner_stack = input.get_item(0);
        let dye_stack = input.get_item(1);
        let mut result_stack = ItemStack::empty();
        if !banner_stack.is_empty()
            && !dye_stack.is_empty()
            && let Some(color) = dye_stack.get(DYE)
        {
            let mut layers = banner_stack
                .get(BANNER_PATTERNS)
                .map_or_else(Vec::new, |layers| layers.layers().to_vec());
            layers.push(BannerPatternLayer::new(pattern.clone(), *color));
            result_stack = banner_stack.copy_with_count(1);
            result_stack.set(BANNER_PATTERNS, BannerPatternLayers::new(layers));
        }

        if !ItemStack::matches(&result_stack, result.get_item(0)) {
            result.set_item(0, result_stack);
        }
    }
}

impl MenuKind for LoomKind {
    /// Returns true while the original loom remains in range.
    fn still_valid(&self, _behavior: &MenuBehavior, player: &Player) -> bool {
        let state = self.world.get_block_state(self.block_pos);
        state.get_block() == &vanilla_blocks::LOOM
            && player.is_within_block_interaction_range_with_buffer(self.block_pos, 4.0)
    }

    fn slots_changed(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        _player: &Player,
    ) {
        let input_id = ContainerId::from_arc(&self.input_container);
        let result_id = ContainerId::from_arc(&self.result_container);

        let (banner, dye, pattern) = {
            let Some(input) = guard.get(input_id) else {
                return;
            };
            (
                input.get_item(0).clone(),
                input.get_item(1).clone(),
                input.get_item(2).clone(),
            )
        };

        if banner.is_empty() || dye.is_empty() {
            if let Some(result) = guard.get_mut(result_id) {
                result.set_item(0, ItemStack::empty());
            }
            self.selectable_patterns = Vec::new();
            self.selected_pattern.set(behavior, -1);
            return;
        }

        let selected = self.selected_pattern.get(behavior);
        let valid_index = self.is_valid_pattern_index(i32::from(selected));
        let previous = std::mem::take(&mut self.selectable_patterns);
        self.selectable_patterns = self.get_selectable_patterns(&pattern);

        let mut pattern_to_display: Option<RegistryHolder<BannerPattern>> =
            if self.selectable_patterns.len() == 1 {
                self.selected_pattern.set(behavior, 0);
                Some(self.selectable_patterns[0].clone())
            } else if !valid_index {
                self.selected_pattern.set(behavior, -1);
                None
            } else {
                let selected_value = previous.get(selected as usize).cloned();
                match selected_value
                    .and_then(|value| self.selectable_patterns.iter().position(|p| *p == value))
                {
                    Some(new_index) => {
                        let value = self.selectable_patterns[new_index].clone();
                        self.selected_pattern.set(behavior, new_index as i16);
                        Some(value)
                    }
                    None => {
                        self.selected_pattern.set(behavior, -1);
                        None
                    }
                }
            };

        let Some(display) = pattern_to_display.take() else {
            if let Some(result) = guard.get_mut(result_id) {
                result.set_item(0, ItemStack::empty());
            }
            return;
        };

        let layers = banner
            .get(BANNER_PATTERNS)
            .map_or(0, |layers| layers.layers().len());
        if layers >= 6 {
            self.selected_pattern.set(behavior, -1);
            if let Some(result) = guard.get_mut(result_id) {
                result.set_item(0, ItemStack::empty());
            }
            return;
        }

        self.setup_result_slot(guard, &display);
    }

    /// Clears the virtual result on close. Inputs are drained by [`Menu::removed`].
    fn removed(&mut self, _behavior: &mut MenuBehavior, _player: &Player) {
        self.result_container.lock().set_item(0, ItemStack::empty());
    }

    fn on_button_clicked(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        button_id: i32,
        _player: &Player,
    ) -> bool {
        if !self.is_valid_pattern_index(button_id) {
            return false;
        }

        self.selected_pattern.set(behavior, button_id as i16);
        let pattern = self.selectable_patterns[button_id as usize].clone();
        self.setup_result_slot(guard, &pattern);
        true
    }

    fn can_take_item_for_pick_all(&self, _carried: &ItemStack, slot_index: usize) -> bool {
        !self.result.contains(slot_index)
    }

    fn quick_move(
        &mut self,
        behavior: &mut MenuBehavior,
        guard: &mut ContainerLockGuard,
        slot_index: usize,
        player: &Player,
    ) -> Option<ItemStack> {
        if !self.player.all().contains(slot_index) {
            return None;
        }

        let clicked = behavior.slots()[slot_index].get_item(guard).clone();
        if clicked.is_empty() {
            return Some(ItemStack::empty());
        }

        let mut remaining = clicked.clone();
        let destination = if is_banner(&clicked) {
            self.banner
        } else if is_dye_item(&clicked) {
            self.dye
        } else if is_pattern_item(&clicked) {
            self.pattern
        } else if self.player.main().contains(slot_index) {
            self.player.hotbar()
        } else {
            self.player.main()
        };

        let moved = behavior.move_item_stack_to(
            guard,
            slot_index,
            &mut remaining,
            destination.start(),
            destination.end(),
            FillDirection::Forward,
        );
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
