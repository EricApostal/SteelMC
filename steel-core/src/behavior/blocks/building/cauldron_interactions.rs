//! Vanilla `CauldronInteractions`: the built-in `(cauldron, item)` interaction table.
//!
//! Vanilla hangs this table off a `Dispatcher` per cauldron kind. Steel keeps the
//! same shape as a match over the cauldron kind and the held item/tag, because the
//! table is closed and never loaded from data.

use std::sync::Arc;

use steel_registry::blocks::block_state_ext::BlockStateExt as _;
use steel_registry::blocks::properties::{BlockStateProperties, IntProperty};
use steel_registry::data_components::vanilla_components::{
    BANNER_PATTERNS, DYED_COLOR, POTION_CONTENTS,
};
use steel_registry::item_stack::ItemStack;
use steel_registry::items::ItemRef;
use steel_registry::sound_event::SoundEventRef;
use steel_registry::stat::vanilla_stat_types;
use steel_registry::vanilla_item_tags::ItemTag;
use steel_registry::{
    REGISTRY, sound_events, vanilla_blocks, vanilla_custom_stats, vanilla_game_events,
    vanilla_items, vanilla_potions,
};
use steel_utils::types::{InteractionHand, UpdateFlags};
use steel_utils::{BlockPos, BlockStateId};

use crate::behavior::InventoryAccess;
use crate::behavior::context::InteractionResult;
use crate::behavior::item_utils::apply_filled_result;
use crate::behavior::items::water_potion_stack;
use crate::fluid::FluidStateExt as _;
use crate::player::Player;
use crate::world::World;
use crate::world::game_event::GameEventContext;

const LEVEL_CAULDRON: &IntProperty = &BlockStateProperties::LEVEL_CAULDRON;

/// The cauldron flavor an interaction targets, mirroring vanilla's four
/// `CauldronInteraction.Dispatcher` instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CauldronKind {
    /// The empty `minecraft:cauldron`.
    Empty,
    /// `minecraft:water_cauldron`.
    Water,
    /// `minecraft:lava_cauldron`.
    Lava,
    /// `minecraft:powder_snow_cauldron`.
    PowderSnow,
}

/// One cauldron interaction. Mirrors vanilla `CauldronInteraction.interact`.
pub(crate) fn use_item_on(
    kind: CauldronKind,
    state: BlockStateId,
    world: &Arc<World>,
    pos: BlockPos,
    player: &Player,
    hand: InteractionHand,
    inv: &mut InventoryAccess,
) -> InteractionResult {
    let interaction = Interaction {
        state,
        world,
        pos,
        player,
        hand,
        inv,
    };

    match kind {
        CauldronKind::Empty => interaction.empty_cauldron(),
        CauldronKind::Water => interaction.water_cauldron(),
        CauldronKind::Lava => interaction.lava_cauldron(),
        CauldronKind::PowderSnow => interaction.powder_snow_cauldron(),
    }
}

#[derive(Clone, Copy)]
struct Interaction<'a> {
    state: BlockStateId,
    world: &'a Arc<World>,
    pos: BlockPos,
    player: &'a Player,
    hand: InteractionHand,
    inv: &'a InventoryAccess,
}

impl Interaction<'_> {
    fn empty_cauldron(self) -> InteractionResult {
        let item = self.item();
        if item == &*vanilla_items::POTION {
            self.potion_into_empty_cauldron()
        } else {
            self.default_interactions(item)
        }
    }

    fn water_cauldron(self) -> InteractionResult {
        let item = self.item();
        if item == &*vanilla_items::BUCKET {
            return self.fill_bucket(
                &vanilla_items::WATER_BUCKET,
                &sound_events::ITEM_BUCKET_FILL,
                is_full(self.state),
            );
        }
        if item == &*vanilla_items::GLASS_BOTTLE {
            return self.bottle_fill_from_water();
        }
        if item == &*vanilla_items::POTION {
            return self.potion_into_water_cauldron();
        }
        if item.has_tag(&ItemTag::CAULDRON_CAN_REMOVE_DYE) {
            return self.wash_dyed_item();
        }
        if item.has_tag(&ItemTag::BANNERS) {
            return self.wash_banner();
        }
        if item.has_tag(&ItemTag::SHULKER_BOXES) {
            return self.wash_shulker_box();
        }

        self.default_interactions(item)
    }

    fn lava_cauldron(self) -> InteractionResult {
        let item = self.item();
        if item == &*vanilla_items::BUCKET {
            return self.fill_bucket(
                &vanilla_items::LAVA_BUCKET,
                &sound_events::ITEM_BUCKET_FILL_LAVA,
                true,
            );
        }

        self.default_interactions(item)
    }

    fn powder_snow_cauldron(self) -> InteractionResult {
        let item = self.item();
        if item == &*vanilla_items::BUCKET {
            return self.fill_bucket(
                &vanilla_items::POWDER_SNOW_BUCKET,
                &sound_events::ITEM_BUCKET_FILL_POWDER_SNOW,
                is_full(self.state),
            );
        }

        self.default_interactions(item)
    }

    /// Vanilla `CauldronInteractions.addDefaultInteractions`: the filled-bucket
    /// handlers every dispatcher starts with.
    fn default_interactions(self, item: ItemRef) -> InteractionResult {
        match item {
            item if item == &*vanilla_items::LAVA_BUCKET => {
                if self.is_under_water() {
                    return InteractionResult::Consume;
                }
                self.empty_bucket(
                    vanilla_blocks::LAVA_CAULDRON.default_state(),
                    &sound_events::ITEM_BUCKET_EMPTY_LAVA,
                )
            }
            item if item == &*vanilla_items::WATER_BUCKET => {
                let new_state = vanilla_blocks::WATER_CAULDRON
                    .default_state()
                    .set_value(LEVEL_CAULDRON, LEVEL_CAULDRON.max);
                self.empty_bucket(new_state, &sound_events::ITEM_BUCKET_EMPTY)
            }
            item if item == &*vanilla_items::POWDER_SNOW_BUCKET => {
                if self.is_under_water() {
                    return InteractionResult::Consume;
                }
                let new_state = vanilla_blocks::POWDER_SNOW_CAULDRON
                    .default_state()
                    .set_value(LEVEL_CAULDRON, LEVEL_CAULDRON.max);
                self.empty_bucket(new_state, &sound_events::ITEM_BUCKET_EMPTY_POWDER_SNOW)
            }
            _ => InteractionResult::TryEmptyHandInteraction,
        }
    }

    /// Vanilla `CauldronInteractions.fillBucket`: take a full cauldron's contents.
    fn fill_bucket(
        self,
        filled_item: ItemRef,
        sound: SoundEventRef,
        can_fill: bool,
    ) -> InteractionResult {
        if !can_fill {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let used_item = self.item();
        self.apply_filled_result(ItemStack::new(filled_item), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, used_item);
        self.world.set_block(
            self.pos,
            vanilla_blocks::CAULDRON.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        self.world.play_block_sound(sound, self.pos, 1.0, 1.0, None);
        self.world.game_event(
            &vanilla_game_events::FLUID_PICKUP,
            self.pos,
            &GameEventContext::new(None, None),
        );

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.emptyBucket`: pour a filled bucket into the cauldron.
    fn empty_bucket(self, new_state: BlockStateId, sound: SoundEventRef) -> InteractionResult {
        let used_item = self.item();
        self.apply_filled_result(ItemStack::new(&vanilla_items::BUCKET), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::FILL_CAULDRON);
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, used_item);
        self.world
            .set_block(self.pos, new_state, UpdateFlags::UPDATE_ALL);
        self.world.play_block_sound(sound, self.pos, 1.0, 1.0, None);
        self.world.game_event(
            &vanilla_game_events::FLUID_PLACE,
            self.pos,
            &GameEventContext::new(None, None),
        );

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions` water dispatcher's glass-bottle handler.
    fn bottle_fill_from_water(self) -> InteractionResult {
        let used_item = self.item();
        self.apply_filled_result(water_potion_stack(), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, used_item);
        lower_fill_level(self.state, self.world, self.pos);
        self.world
            .play_block_sound(&sound_events::ITEM_BOTTLE_FILL, self.pos, 1.0, 1.0, None);
        self.world.game_event(
            &vanilla_game_events::FLUID_PICKUP,
            self.pos,
            &GameEventContext::new(None, None),
        );

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions` empty dispatcher's water-potion handler:
    /// an empty cauldron becomes a level-1 water cauldron.
    fn potion_into_empty_cauldron(self) -> InteractionResult {
        if !self.holds_water_potion() {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let used_item = self.item();
        self.apply_filled_result(ItemStack::new(&vanilla_items::GLASS_BOTTLE), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, used_item);
        self.world.set_block(
            self.pos,
            vanilla_blocks::WATER_CAULDRON.default_state(),
            UpdateFlags::UPDATE_ALL,
        );
        self.world
            .play_block_sound(&sound_events::ITEM_BOTTLE_EMPTY, self.pos, 1.0, 1.0, None);
        self.world.game_event(
            &vanilla_game_events::FLUID_PLACE,
            self.pos,
            &GameEventContext::new(None, None),
        );

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions` water dispatcher's water-potion handler:
    /// one level up, leaving a glass bottle behind.
    fn potion_into_water_cauldron(self) -> InteractionResult {
        let level = self.state.get_value(LEVEL_CAULDRON);
        if level == LEVEL_CAULDRON.max || !self.holds_water_potion() {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let used_item = self.item();
        self.apply_filled_result(ItemStack::new(&vanilla_items::GLASS_BOTTLE), true);
        self.player
            .award_custom_stat(&vanilla_custom_stats::USE_CAULDRON);
        self.player
            .award_stat(&vanilla_stat_types::ITEM_USED, used_item);
        let new_state = self.state.set_value(LEVEL_CAULDRON, level + 1);
        self.world
            .set_block(self.pos, new_state, UpdateFlags::UPDATE_ALL);
        self.world
            .play_block_sound(&sound_events::ITEM_BOTTLE_EMPTY, self.pos, 1.0, 1.0, None);
        self.world.game_event(
            &vanilla_game_events::FLUID_PLACE,
            self.pos,
            &GameEventContext::new(None, None),
        );

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.dyedItemIteration`: the dye tag handler
    /// shared by leather armour, wolf armour, and leather horse armour.
    fn wash_dyed_item(self) -> InteractionResult {
        if !self.inv.with_item(|stack| stack.has(DYED_COLOR)) {
            return InteractionResult::TryEmptyHandInteraction;
        }

        self.inv.with_item(|stack| stack.remove(DYED_COLOR));
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_ARMOR);
        lower_fill_level(self.state, self.world, self.pos);

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.bannerInteraction`.
    fn wash_banner(self) -> InteractionResult {
        let Some(layers) = self
            .inv
            .with_item(|stack| stack.get(BANNER_PATTERNS).cloned())
        else {
            return InteractionResult::TryEmptyHandInteraction;
        };
        if layers.layers().is_empty() {
            return InteractionResult::TryEmptyHandInteraction;
        }

        let mut result = self.inv.with_item(|stack| stack.copy_with_count(1));
        result.set(BANNER_PATTERNS, layers.remove_last());
        self.apply_filled_result(result, false);
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_BANNER);
        lower_fill_level(self.state, self.world, self.pos);

        InteractionResult::Success
    }

    /// Vanilla `CauldronInteractions.shulkerBoxInteraction`: a dyed shulker box
    /// loses its colour while keeping its contents.
    fn wash_shulker_box(self) -> InteractionResult {
        let plain_shulker_box = REGISTRY.items.by_block(&vanilla_blocks::SHULKER_BOX);
        let result = self.inv.with_item(|stack| {
            // Vanilla registers only the sixteen dyed shulker boxes; the plain
            // `shulker_boxes` tag also contains the undyed item.
            if stack.item() == plain_shulker_box {
                return None;
            }
            Some(ItemStack::with_count_and_patch(
                plain_shulker_box,
                1,
                stack.components_patch().clone(),
            ))
        });
        let Some(result) = result else {
            return InteractionResult::TryEmptyHandInteraction;
        };

        self.apply_filled_result(result, false);
        self.player
            .award_custom_stat(&vanilla_custom_stats::CLEAN_SHULKER_BOX);
        lower_fill_level(self.state, self.world, self.pos);

        InteractionResult::Success
    }

    fn holds_water_potion(&self) -> bool {
        self.inv.with_item(|stack| {
            stack
                .get(POTION_CONTENTS)
                .is_some_and(|contents| contents.is(&vanilla_potions::WATER))
        })
    }

    fn item(&self) -> ItemRef {
        self.inv.with_item(|stack| stack.item())
    }

    fn is_under_water(&self) -> bool {
        self.world
            .get_block_state(self.pos.above())
            .get_fluid_state()
            .is_water()
    }

    fn apply_filled_result(&self, result_stack: ItemStack, limit_creative_stack_size: bool) {
        apply_filled_result(
            self.inv,
            self.player,
            self.hand,
            result_stack,
            limit_creative_stack_size,
        );
    }
}

/// Vanilla `LayeredCauldronBlock.lowerFillLevel`.
fn lower_fill_level(state: BlockStateId, world: &Arc<World>, pos: BlockPos) {
    let new_level = state.get_value(LEVEL_CAULDRON) - 1;
    let new_state = if new_level == 0 {
        vanilla_blocks::CAULDRON.default_state()
    } else {
        state.set_value(LEVEL_CAULDRON, new_level)
    };

    world.set_block(pos, new_state, UpdateFlags::UPDATE_ALL);
    world.game_event(
        &vanilla_game_events::BLOCK_CHANGE,
        pos,
        &GameEventContext::new(None, Some(new_state)),
    );
}

fn is_full(state: BlockStateId) -> bool {
    state.get_value(LEVEL_CAULDRON) == LEVEL_CAULDRON.max
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use glam::DVec3;
    use steel_registry::blocks::BlockRef;
    use steel_registry::data_components::vanilla_components::{DYED_COLOR, POTION_CONTENTS};
    use steel_registry::data_components::{DyedItemColor, PotionContents};
    use steel_registry::item_stack::ItemStack;
    use steel_registry::{
        RegistryReference, init_vanilla_registry, vanilla_blocks, vanilla_items, vanilla_potions,
    };
    use steel_utils::types::{GameType, InteractionHand, UpdateFlags};
    use steel_utils::{BlockPos, ChunkPos, Direction};

    use super::*;
    use crate::behavior::init_behaviors;
    use crate::behavior::{BlockHitResult, InteractionResult};
    use crate::entity::Entity as _;
    use crate::player::Player;
    use crate::player::game_mode::use_item_on;
    use crate::test_support::{
        TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
    };

    const TEST_POS: BlockPos = BlockPos::new(8, 64, 8);

    fn cauldron_world(
        key: &'static str,
        block: BlockRef,
        level: Option<u8>,
    ) -> (TestWorld, Arc<Player>) {
        init_vanilla_registry();
        init_behaviors();

        let world_fixture = fresh_test_world(key);
        let world = &world_fixture.world;
        insert_ready_full_chunk(world, ChunkPos::from_block_pos(TEST_POS));
        let mut state = block.default_state();
        if let Some(level) = level {
            state = state.set_value(&BlockStateProperties::LEVEL_CAULDRON, level);
        }
        assert!(world.set_block(TEST_POS, state, UpdateFlags::UPDATE_ALL));

        let player = TestPlayerBuilder::new(Arc::clone(world), "CauldronTester", 1).build();
        player.base().set_position_local(DVec3::new(
            f64::from(TEST_POS.x()) + 0.5,
            f64::from(TEST_POS.y()),
            f64::from(TEST_POS.z()) + 0.5,
        ));
        player.set_rotation((0.0, 0.0));
        (world_fixture, player)
    }

    fn hit(pos: BlockPos) -> BlockHitResult {
        BlockHitResult {
            location: DVec3::new(
                f64::from(pos.x()) + 0.5,
                f64::from(pos.y()) + 0.5,
                f64::from(pos.z()) + 0.5,
            ),
            direction: Direction::Up,
            block_pos: pos,
            miss: false,
            inside: false,
            world_border_hit: false,
        }
    }

    fn hold(player: &Player, stack: ItemStack) {
        player
            .inventory
            .lock()
            .set_item_in_hand(InteractionHand::MainHand, stack);
    }

    fn held(player: &Player) -> ItemStack {
        player
            .inventory
            .lock()
            .get_item_in_hand(InteractionHand::MainHand)
            .clone()
    }

    fn use_on(player: &Arc<Player>, world: &Arc<World>, pos: BlockPos) -> InteractionResult {
        use_item_on(player, world, InteractionHand::MainHand, &hit(pos))
    }

    fn water_stack() -> ItemStack {
        let mut stack = ItemStack::new(&vanilla_items::POTION);
        stack.set(
            POTION_CONTENTS,
            PotionContents::new(
                Some(RegistryReference::new(&vanilla_potions::WATER)),
                None,
                Vec::new(),
                None,
            ),
        );
        stack
    }

    fn level(world: &World, pos: BlockPos) -> u8 {
        world
            .get_block_state(pos)
            .get_value(&BlockStateProperties::LEVEL_CAULDRON)
    }

    #[test]
    fn water_bucket_fills_an_empty_cauldron() {
        let (world_fixture, player) = cauldron_world(
            "cauldron_water_bucket_fill",
            &vanilla_blocks::CAULDRON,
            None,
        );
        let world = &world_fixture.world;
        hold(&player, ItemStack::new(&vanilla_items::WATER_BUCKET));

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::WATER_CAULDRON
        );
        assert_eq!(level(world, TEST_POS), 3);
        assert_eq!(held(&player).item, &*vanilla_items::BUCKET);
    }

    #[test]
    fn glass_bottle_takes_water_and_leaves_the_level_down() {
        let (world_fixture, player) = cauldron_world(
            "cauldron_bottle_fill",
            &vanilla_blocks::WATER_CAULDRON,
            Some(3),
        );
        let world = &world_fixture.world;
        hold(&player, ItemStack::new(&vanilla_items::GLASS_BOTTLE));

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::WATER_CAULDRON
        );
        assert_eq!(level(world, TEST_POS), 2);
        let bottle = held(&player);
        assert_eq!(bottle.item, &*vanilla_items::POTION);
        assert!(
            bottle
                .get(POTION_CONTENTS)
                .is_some_and(|contents| contents.is(&vanilla_potions::WATER))
        );
    }

    #[test]
    fn water_bottle_empties_into_an_empty_cauldron() {
        let (world_fixture, player) =
            cauldron_world("cauldron_potion_empty", &vanilla_blocks::CAULDRON, None);
        let world = &world_fixture.world;
        hold(&player, water_stack());

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::WATER_CAULDRON
        );
        assert_eq!(level(world, TEST_POS), 1);
        assert_eq!(held(&player).item, &*vanilla_items::GLASS_BOTTLE);
    }

    #[test]
    fn water_bottle_raises_a_partial_water_cauldron() {
        let (world_fixture, player) = cauldron_world(
            "cauldron_potion_level",
            &vanilla_blocks::WATER_CAULDRON,
            Some(1),
        );
        let world = &world_fixture.world;
        hold(&player, water_stack());

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(level(world, TEST_POS), 2);
        assert_eq!(held(&player).item, &*vanilla_items::GLASS_BOTTLE);
    }

    #[test]
    fn lava_bucket_fills_a_lava_cauldron_and_a_bucket_takes_it_back() {
        let (world_fixture, player) =
            cauldron_world("cauldron_lava_bucket", &vanilla_blocks::CAULDRON, None);
        let world = &world_fixture.world;
        hold(&player, ItemStack::new(&vanilla_items::LAVA_BUCKET));

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::LAVA_CAULDRON
        );
        assert_eq!(held(&player).item, &*vanilla_items::BUCKET);

        hold(&player, ItemStack::new(&vanilla_items::BUCKET));
        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::CAULDRON
        );
        assert_eq!(held(&player).item, &*vanilla_items::LAVA_BUCKET);
    }

    #[test]
    fn powder_snow_bucket_fills_and_a_bucket_takes_it_back() {
        let (world_fixture, player) = cauldron_world(
            "cauldron_powder_snow_bucket",
            &vanilla_blocks::CAULDRON,
            None,
        );
        let world = &world_fixture.world;
        hold(&player, ItemStack::new(&vanilla_items::POWDER_SNOW_BUCKET));

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::POWDER_SNOW_CAULDRON
        );
        assert_eq!(level(world, TEST_POS), 3);
        assert_eq!(held(&player).item, &*vanilla_items::BUCKET);

        hold(&player, ItemStack::new(&vanilla_items::BUCKET));
        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::CAULDRON
        );
        assert_eq!(held(&player).item, &*vanilla_items::POWDER_SNOW_BUCKET);
    }

    #[test]
    fn dyed_leather_armour_is_washed_and_lowers_the_cauldron() {
        let (world_fixture, player) = cauldron_world(
            "cauldron_wash_armour",
            &vanilla_blocks::WATER_CAULDRON,
            Some(3),
        );
        let world = &world_fixture.world;
        let mut armour = ItemStack::new(&vanilla_items::LEATHER_CHESTPLATE);
        armour.set(DYED_COLOR, DyedItemColor::new(0xff_0000));
        hold(&player, armour);

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert!(!held(&player).has(DYED_COLOR));
        assert_eq!(level(world, TEST_POS), 2);
    }

    #[test]
    fn creative_players_keep_the_used_bucket() {
        let (world_fixture, player) =
            cauldron_world("cauldron_creative_fill", &vanilla_blocks::CAULDRON, None);
        let world = &world_fixture.world;
        player.restore_game_modes(GameType::Creative, None);
        player
            .abilities
            .lock()
            .update_for_game_mode(GameType::Creative);
        hold(&player, ItemStack::new(&vanilla_items::WATER_BUCKET));

        assert_eq!(use_on(&player, world, TEST_POS), InteractionResult::Success);
        assert_eq!(
            world.get_block_state(TEST_POS).get_block(),
            &vanilla_blocks::WATER_CAULDRON
        );
        assert_eq!(held(&player).item, &*vanilla_items::WATER_BUCKET);
        assert_eq!(held(&player).count(), 1);
    }
}
