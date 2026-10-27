#![expect(
    clippy::assert_is_empty,
    reason = "`assert!(xs.is_empty())` reads better than equality against an empty array"
)]

use crate::data_components::vanilla_components::{
    BLOCK_STATE, CUSTOM_MODEL_DATA, DAMAGE, ENCHANTMENTS, INSTRUMENT, ItemEnchantments, REPAIR_COST,
};
use crate::data_components::{CustomModelData, DataComponentMap};
use crate::equipment::EquipmentSlot;
use crate::items::ItemRef;
use crate::vanilla_instrument_tags::InstrumentTag;
use crate::vanilla_items;
use crate::{init_vanilla_registry, vanilla_loot_tables};

use super::*;
use rand::SeedableRng;
use steel_utils::BlockPos;

fn test_rng() -> rand::rngs::StdRng {
    rand::rngs::StdRng::seed_from_u64(12345)
}

fn init_test_registries() {
    init_vanilla_registry();
}

#[test]
fn test_oak_log_loot() {
    init_test_registries();
    let mut rng = test_rng();

    let mut ctx = LootContext::new(&mut rng);
    let items = vanilla_loot_tables::BLOCKS_OAK_LOG.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].count, 1);
    assert_eq!(items[0].item.key, Identifier::vanilla_static("oak_log"));
}

#[test]
fn set_instrument_selects_from_the_configured_holder_set() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    let mut goat_horn = ItemStack::new(&vanilla_items::GOAT_HORN);
    let function = LootFunction::SetInstrument {
        options: InstrumentOptions::Tag(InstrumentTag::REGULAR_GOAT_HORNS),
    };

    function.apply(&mut goat_horn, &mut ctx);

    let selected = goat_horn
        .get(INSTRUMENT)
        .and_then(|component| component.instrument().as_reference())
        .expect("set_instrument should select a registered instrument");
    assert!(
        REGISTRY
            .instruments
            .is_in_tag(selected, &InstrumentTag::REGULAR_GOAT_HORNS)
    );
}

#[test]
fn test_diamond_ore_loot_no_silk_touch() {
    // Without silk touch, diamond ore should drop diamond (not the ore block)
    init_test_registries();
    let mut rng = test_rng();

    let mut ctx = LootContext::new(&mut rng);
    let items = vanilla_loot_tables::BLOCKS_DIAMOND_ORE.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].count, 1);
    // Without silk touch enchantment, diamond ore drops diamond
    assert_eq!(items[0].item.key, Identifier::vanilla_static("diamond"));
}

#[test]
fn test_grass_block_loot_no_silk_touch() {
    // Without silk touch, grass block should drop dirt
    init_test_registries();
    let mut rng = test_rng();

    let mut ctx = LootContext::new(&mut rng);
    let items = vanilla_loot_tables::BLOCKS_GRASS_BLOCK.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].count, 1);
    // Without silk touch, grass block drops dirt
    assert_eq!(items[0].item.key, Identifier::vanilla_static("dirt"));
}

#[test]
fn test_stone_loot_no_silk_touch() {
    // Without silk touch, stone should drop cobblestone
    init_test_registries();
    let mut rng = test_rng();

    let mut ctx = LootContext::new(&mut rng);
    let items = vanilla_loot_tables::BLOCKS_STONE.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].count, 1);
    // Without silk touch, stone drops cobblestone
    assert_eq!(items[0].item.key, Identifier::vanilla_static("cobblestone"));
}

#[test]
fn test_pig_loot_drops_raw_porkchop_when_not_on_fire() {
    init_test_registries();
    let mut rng = test_rng();
    let pig_key = Identifier::vanilla_static("pig");
    let pig = EntityRef {
        entity_type: Some(&pig_key),
        flags: EntityRefFlags::default(),
        equipment: None,
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    };

    let mut ctx = LootContext::new(&mut rng).with_this_entity(pig);
    let items = vanilla_loot_tables::ENTITIES_PIG.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item.key, Identifier::vanilla_static("porkchop"));
    assert!((1..=3).contains(&items[0].count));
}

#[test]
fn shearing_sheep_table_parses_the_flat_type_specific_sheep_key() {
    init_test_registries();

    let table = &vanilla_loot_tables::SHEARING_SHEEP;
    let pool = table
        .pools
        .first()
        .expect("shearing table should have a pool");
    let LootEntry::Alternatives { children, .. } = pool
        .entries
        .first()
        .expect("shearing pool should start with alternatives")
    else {
        panic!("shearing pool should use an alternatives entry");
    };

    let mut checked = 0;
    for child in *children {
        let LootEntry::LootTableRef { conditions, .. } = child else {
            continue;
        };
        let Some(LootCondition::EntityProperties { predicate, .. }) = conditions.first() else {
            continue;
        };
        assert!(
            predicate.sheep_color.is_some(),
            "branch should match its wool color"
        );
        assert_eq!(
            predicate.sheep_sheared,
            Some(false),
            "sheared must come from the flat minecraft:type_specific/sheep key"
        );
        checked += 1;
    }
    assert_eq!(
        checked, 16,
        "all sixteen color branches should carry the sheared predicate"
    );
}

#[test]
fn chicken_lay_branches_carry_the_chicken_variant_component() {
    init_test_registries();

    let table = &vanilla_loot_tables::GAMEPLAY_CHICKEN_LAY;
    let pool = table.pools.first().expect("chicken_lay should have a pool");
    let LootEntry::Alternatives { children, .. } = pool
        .entries
        .first()
        .expect("chicken_lay should start with alternatives")
    else {
        panic!("chicken_lay should use an alternatives entry");
    };

    let variants = children
        .iter()
        .filter_map(|child| {
            let LootEntry::Item { conditions, .. } = child else {
                return None;
            };
            let Some(LootCondition::EntityProperties { predicate, .. }) = conditions.first() else {
                return None;
            };
            predicate
                .chicken_variant
                .as_ref()
                .map(|id| id.path.as_ref())
        })
        .collect::<Vec<_>>();
    assert_eq!(
        variants,
        ["temperate", "warm", "cold"],
        "each egg branch should match its chicken variant"
    );
}

#[test]
fn chicken_lay_drops_the_egg_matching_the_chicken_variant() {
    init_test_registries();

    for (variant, expected_egg) in [
        (&crate::vanilla_chicken_variants::TEMPERATE.key, "egg"),
        (&crate::vanilla_chicken_variants::WARM.key, "brown_egg"),
        (&crate::vanilla_chicken_variants::COLD.key, "blue_egg"),
    ] {
        let mut rng = test_rng();
        let chicken = EntityRef {
            entity_type: Some(&Identifier::vanilla_static("chicken")),
            flags: EntityRefFlags::default(),
            equipment: None,
            custom_name: None,
            sheep_color: None,
            sheep_sheared: None,
            chicken_variant: Some(variant),
        };

        let mut ctx = LootContext::new(&mut rng).with_this_entity(chicken);
        let items = vanilla_loot_tables::GAMEPLAY_CHICKEN_LAY.get_random_items(&mut ctx);

        assert_eq!(
            items.len(),
            1,
            "a {variant} chicken should lay exactly one egg"
        );
        assert_eq!(
            items[0].item.key,
            Identifier::vanilla_static(expected_egg),
            "a {variant} chicken should lay {expected_egg}"
        );
    }
}

#[test]
fn sheared_predicate_rejects_non_sheep_entities() {
    init_test_registries();
    let mut rng = test_rng();
    let pig_key = Identifier::vanilla_static("pig");
    let pig = EntityRef {
        entity_type: Some(&pig_key),
        flags: EntityRefFlags::default(),
        equipment: None,
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    };
    let mut ctx = LootContext::new(&mut rng).with_this_entity(pig);

    let condition = LootCondition::EntityProperties {
        entity: LootContextEntity::This,
        predicate: EntityPredicate {
            entity_type: None,
            flags: None,
            equipment: None,
            sheep_color: None,
            sheep_sheared: Some(false),
            chicken_variant: None,
        },
    };
    assert!(
        !condition.test(&mut ctx),
        "a non-sheep entity must fail a sheared predicate, mirroring SheepPredicate.matches"
    );
}

#[test]
fn test_pig_loot_smelt_condition_uses_entity_fire_flag() {
    init_test_registries();
    let mut rng = test_rng();
    let pig_key = Identifier::vanilla_static("pig");
    let pig = EntityRef {
        entity_type: Some(&pig_key),
        flags: EntityRefFlags {
            is_on_fire: true,
            ..EntityRefFlags::default()
        },
        equipment: None,
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    };

    let mut ctx = LootContext::new(&mut rng).with_this_entity(pig);
    let items = vanilla_loot_tables::ENTITIES_PIG.get_random_items(&mut ctx);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].item.key,
        Identifier::vanilla_static("cooked_porkchop")
    );
    assert!((1..=3).contains(&items[0].count));
}

fn enchanted_item(item: ItemRef, enchantment: &'static str, level: u32) -> ItemStack {
    let mut enchantments = ItemEnchantments::empty();
    enchantments.set(Identifier::vanilla_static(enchantment), level);
    let mut stack = ItemStack::new(item);
    stack.set(ENCHANTMENTS, enchantments);
    stack
}

fn equipment_with(slot: EquipmentSlot, stack: ItemStack) -> EntityEquipmentSlots {
    let mut slots = EntityEquipmentSlots::default();
    slots[slot.index()] = stack;
    slots
}

fn living_entity_ref<'a>(
    entity_type: &'a Identifier,
    equipment: &'a EntityEquipmentSlots,
) -> EntityRef<'a> {
    EntityRef {
        entity_type: Some(entity_type),
        flags: EntityRefFlags::default(),
        equipment: Some(EntityEquipmentRef::new(equipment)),
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    }
}

fn roll_killed_by_player(
    table: &LootTable,
    victim: &'static str,
    player_equipment: &EntityEquipmentSlots,
    rolls: u64,
) -> Vec<ItemStack> {
    init_test_registries();
    let victim_key = Identifier::vanilla_static(victim);
    let player_key = Identifier::vanilla_static("player");
    let victim_equipment = EntityEquipmentSlots::default();
    let victim = living_entity_ref(&victim_key, &victim_equipment);
    let player = living_entity_ref(&player_key, player_equipment);

    (0..rolls)
        .flat_map(|seed| {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let mut ctx = LootContext::new(&mut rng)
                .with_killed_by_player(true)
                .with_this_entity(victim)
                .with_killer_entity(player)
                .with_direct_killer_entity(player);
            table.get_random_items(&mut ctx)
        })
        .collect()
}

fn roll_pig_killed_by_player(
    player_equipment: &EntityEquipmentSlots,
    rolls: u64,
) -> Vec<ItemStack> {
    roll_killed_by_player(
        &vanilla_loot_tables::ENTITIES_PIG,
        "pig",
        player_equipment,
        rolls,
    )
}

#[test]
fn pig_loot_smelts_when_direct_attacker_holds_fire_aspect() {
    let sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "fire_aspect", 1);
    let items = roll_pig_killed_by_player(&equipment_with(EquipmentSlot::MainHand, sword), 1);

    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].item.key,
        Identifier::vanilla_static("cooked_porkchop")
    );
}

#[test]
fn pig_loot_stays_raw_when_direct_attacker_holds_plain_sword() {
    let sword = ItemStack::new(&vanilla_items::DIAMOND_SWORD);
    let items = roll_pig_killed_by_player(&equipment_with(EquipmentSlot::MainHand, sword), 1);

    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item.key, Identifier::vanilla_static("porkchop"));
}

#[test]
fn pig_loot_count_uses_attacker_looting_level() {
    let sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "looting", 3);
    let items = roll_pig_killed_by_player(&equipment_with(EquipmentSlot::MainHand, sword), 200);

    assert!(items.iter().all(|item| (1..=6).contains(&item.count)));
    assert!(items.iter().any(|item| item.count > 3));
}

#[test]
fn pig_loot_ignores_looting_outside_its_enchantment_slots() {
    let helmet = enchanted_item(&vanilla_items::DIAMOND_HELMET, "looting", 3);
    let items = roll_pig_killed_by_player(&equipment_with(EquipmentSlot::Head, helmet), 200);

    assert!(items.iter().all(|item| (1..=3).contains(&item.count)));
}

#[test]
fn enchanted_count_increase_limit_caps_the_stack_not_the_bonus() {
    let sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "looting", 3);
    let tipped_arrow = Identifier::vanilla_static("tipped_arrow");
    let arrows: Vec<ItemStack> = roll_killed_by_player(
        &vanilla_loot_tables::ENTITIES_STRAY,
        "stray",
        &equipment_with(EquipmentSlot::MainHand, sword),
        200,
    )
    .into_iter()
    .filter(|item| item.item.key == tipped_arrow)
    .collect();

    assert_ne!(arrows, Vec::<ItemStack>::new());
    assert!(arrows.iter().all(|item| item.count == 1));
}

#[test]
fn random_chance_with_enchanted_bonus_reads_attacker_not_tool() {
    init_test_registries();
    let condition = LootCondition::RandomChanceWithEnchantedBonus {
        enchantment: Identifier::vanilla_static("looting"),
        unenchanted_chance: 0.0,
        enchanted_chance: EnchantedChance::Constant(1.0),
    };
    let player_key = Identifier::vanilla_static("player");
    let looting_sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "looting", 3);
    let looting_equipment = equipment_with(EquipmentSlot::MainHand, looting_sword.clone());
    let empty_equipment = EntityEquipmentSlots::default();

    let mut rng = test_rng();
    let mut tool_only = LootContext::new(&mut rng)
        .with_tool(&looting_sword)
        .with_killer_entity(living_entity_ref(&player_key, &empty_equipment));
    assert!(!condition.test(&mut tool_only));

    let mut rng = test_rng();
    let mut attacker_only = LootContext::new(&mut rng)
        .with_killer_entity(living_entity_ref(&player_key, &looting_equipment));
    assert!(condition.test(&mut attacker_only));
}

#[test]
fn equipment_enchantment_level_ignores_empty_stacks() {
    init_test_registries();
    let mut empty_sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "looting", 3);
    empty_sword.count = 0;
    let equipment = equipment_with(EquipmentSlot::MainHand, empty_sword);

    assert_eq!(
        EntityEquipmentRef::new(&equipment)
            .get_enchantment_level(&Identifier::vanilla_static("looting")),
        0
    );
}

#[test]
fn enchanted_count_increase_rounds_like_java_math_round() {
    init_test_registries();
    let function = LootFunction::EnchantedCountIncrease {
        enchantment: Identifier::vanilla_static("looting"),
        count: NumberProvider::Constant(-0.5),
        limit: 0,
    };
    let player_key = Identifier::vanilla_static("player");
    let sword = enchanted_item(&vanilla_items::DIAMOND_SWORD, "looting", 1);
    let equipment = equipment_with(EquipmentSlot::MainHand, sword);
    let mut item = ItemStack::with_count(&vanilla_items::PORKCHOP, 2);

    let mut rng = test_rng();
    let mut ctx =
        LootContext::new(&mut rng).with_killer_entity(living_entity_ref(&player_key, &equipment));
    function.apply(&mut item, &mut ctx);

    // Java `Math.round(-0.5f)` is 0, where Rust's `f32::round` gives -1.
    assert_eq!(item.count, 2);
}

#[test]
fn test_uniform_get_int_reaches_inclusive_max() {
    // Vanilla UniformGenerator.getInt uses Mth.nextInt(rand, min, max), which
    // samples the integer range inclusively; a uniform 1..3 count must yield 3.
    let provider = NumberProvider::Uniform { min: 1.0, max: 3.0 };
    let mut seen = [false; 4];
    for seed in 0u64..1000 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let value = provider.get_int(&mut rng);
        seen[value as usize] = true;
    }
    assert!(
        seen[1] && seen[2] && seen[3],
        "uniform 1..=3 must produce 1, 2 and 3, saw {seen:?}"
    );
}

#[test]
fn test_explosion_decay_function() {
    // Test the explosion_decay function directly
    init_test_registries();

    // explosion_decay reduces count based on 1/radius probability per item
    let cond_func = ConditionalLootFunction {
        function: LootFunction::ExplosionDecay,
        conditions: &[],
    };

    let mut total_survived = 0;
    let initial_count = 10;

    for seed in 0u64..100 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut ctx = LootContext::new(&mut rng).with_explosion(4.0);
        let mut item = ItemStack::with_count(&crate::vanilla_items::STONE, initial_count);
        cond_func.function.apply(&mut item, &mut ctx);
        total_survived += item.count;
    }

    // With 10 items each trial, 100 trials = 1000 items total
    // Each has 25% (1/4.0) chance to survive = ~250 expected
    // Allow for variance: 150-350 range
    assert!(
        total_survived > 150 && total_survived < 350,
        "Expected ~250 items with explosion decay (25% of 1000), got {total_survived}"
    );
}

#[test]
fn ominous_bottle_amplifier_function_clamps_to_persistent_range() {
    use crate::data_components::vanilla_components::OMINOUS_BOTTLE_AMPLIFIER;

    init_test_registries();
    for (provided, expected) in [(-3.0, 0), (2.0, 2), (9.0, 4)] {
        let mut rng = test_rng();
        let mut context = LootContext::new(&mut rng);
        let mut item = ItemStack::new(&crate::vanilla_items::OMINOUS_BOTTLE);
        LootFunction::SetOminousBottleAmplifier {
            amplifier: NumberProvider::Constant(provided),
        }
        .apply(&mut item, &mut context);

        assert_eq!(
            item.get(OMINOUS_BOTTLE_AMPLIFIER)
                .map(|amplifier| amplifier.value()),
            Some(expected)
        );
    }
}

#[test]
fn test_survives_explosion_condition() {
    init_test_registries();

    // Test that survives_explosion condition works
    // Gravel has survives_explosion on its alternatives
    let mut survived = 0;
    for seed in 0..100 {
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        let mut ctx = LootContext::new(&mut rng).with_explosion(4.0);
        let items = vanilla_loot_tables::BLOCKS_GRAVEL.get_random_items(&mut ctx);
        if !items.is_empty() {
            survived += 1;
        }
    }

    // With radius 4.0, ~25% should survive
    assert!(
        survived > 10 && survived < 50,
        "Expected ~25% survival rate, got {survived}%"
    );
}

/// A [`LevelView`] over a fixed set of block positions, for
/// `minecraft:location_check` tests.
struct FakeLevelView {
    blocks: Vec<(BlockPos, BlockStateId)>,
    biome: Option<Identifier>,
    dimension: Identifier,
    light: i32,
    smokey: bool,
    can_see_sky: bool,
}

impl FakeLevelView {
    fn new(dimension: &'static str) -> Self {
        Self {
            blocks: Vec::new(),
            biome: None,
            dimension: Identifier::vanilla_static(dimension),
            light: 15,
            smokey: false,
            can_see_sky: true,
        }
    }

    fn with_block(mut self, pos: BlockPos, state: BlockStateId) -> Self {
        self.blocks.push((pos, state));
        self
    }
}

impl crate::advancement::predicates::LevelView for FakeLevelView {
    fn position(&self) -> (f64, f64, f64) {
        (0.0, 0.0, 0.0)
    }

    fn dimension(&self) -> &Identifier {
        &self.dimension
    }

    fn biome_at(&self, _position: (f64, f64, f64)) -> Option<&Identifier> {
        self.biome.as_ref()
    }

    fn structure_at(&self, _position: (f64, f64, f64)) -> Option<&Identifier> {
        None
    }

    fn block_state_at(&self, position: (f64, f64, f64)) -> Option<BlockStateId> {
        let pos = BlockPos::new(position.0 as i32, position.1 as i32, position.2 as i32);
        self.blocks
            .iter()
            .find_map(|(candidate, state)| (*candidate == pos).then_some(*state))
    }

    fn fluid_at(&self, _position: (f64, f64, f64)) -> Option<&Identifier> {
        None
    }

    fn light_at(&self, _position: (f64, f64, f64)) -> i32 {
        self.light
    }

    fn is_smokey_at(&self, _position: (f64, f64, f64)) -> bool {
        self.smokey
    }

    fn can_see_sky_at(&self, _position: (f64, f64, f64)) -> bool {
        self.can_see_sky
    }
}

fn tall_grass_half(half: &'static str) -> BlockStateId {
    REGISTRY.blocks.set_property(
        REGISTRY
            .blocks
            .get_base_state_id(&crate::vanilla_blocks::TALL_GRASS),
        &crate::blocks::properties::BlockStateProperties::DOUBLE_BLOCK_HALF,
        if half == "upper" {
            crate::blocks::properties::DoubleBlockHalf::Upper
        } else {
            crate::blocks::properties::DoubleBlockHalf::Lower
        },
    )
}

fn tall_grass_location_check() -> LootCondition {
    LootCondition::LocationCheck {
        offset_x: 0,
        offset_y: 1,
        offset_z: 0,
        predicate: LocationPredicate {
            block: Some(BlockPredicate {
                blocks: Some(HolderSet::Keys(&["tall_grass"])),
                state: Some(StatePropertiesPredicate {
                    properties: &[PropertyCheck {
                        name: "half",
                        value: "upper",
                    }],
                }),
            }),
            ..LocationPredicate::default()
        },
    }
}

#[test]
fn location_check_requires_an_origin() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);

    // Vanilla `LocationCheck.test`: `context.getOptional(ORIGIN)` must be present.
    assert!(!tall_grass_location_check().test(&mut ctx));
}

#[test]
fn location_check_resolves_the_predicate_at_the_offset_position() {
    init_test_registries();
    let view = FakeLevelView::new("overworld")
        .with_block(BlockPos::new(0, 64, 0), tall_grass_half("lower"))
        .with_block(BlockPos::new(0, 65, 0), tall_grass_half("upper"));
    let condition = tall_grass_location_check();

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_location(&view);
    assert!(condition.test(&mut ctx));

    // The upper half does not satisfy `offsetY: 1`; the lower half does not
    // satisfy the `half: upper` property.
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 65.0, 0.5)
        .with_location(&view);
    assert!(!condition.test(&mut ctx));
}

#[test]
fn location_check_fails_closed_without_a_level_view() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng).with_origin(0.5, 64.0, 0.5);

    // A constrained predicate cannot be resolved without the level, so it must
    // not silently pass (the pre-`LevelView` behaviour always passed).
    assert!(!tall_grass_location_check().test(&mut ctx));
}

#[test]
fn location_check_without_a_predicate_only_needs_the_origin() {
    init_test_registries();
    let condition = LootCondition::LocationCheck {
        offset_x: 0,
        offset_y: 0,
        offset_z: 0,
        predicate: LocationPredicate::default(),
    };

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng).with_origin(0.5, 64.0, 0.5);
    assert!(condition.test(&mut ctx));

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    assert!(!condition.test(&mut ctx));
}

#[test]
fn location_check_matches_biomes_and_flags() {
    init_test_registries();
    let mut view = FakeLevelView::new("overworld");
    view.biome = Some(Identifier::vanilla_static("jungle"));
    view.smokey = true;
    view.can_see_sky = false;

    let condition = LootCondition::LocationCheck {
        offset_x: 0,
        offset_y: 0,
        offset_z: 0,
        predicate: LocationPredicate {
            biomes: Some(HolderSet::Keys(&["jungle"])),
            dimension: Some(Identifier::vanilla_static("overworld")),
            smokey: Some(true),
            can_see_sky: Some(false),
            light: Some(IntBounds::at_least(7)),
            ..LocationPredicate::default()
        },
    };

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_location(&view);
    assert!(condition.test(&mut ctx));

    // A mismatched dimension fails even when everything else matches.
    let condition = LootCondition::LocationCheck {
        offset_x: 0,
        offset_y: 0,
        offset_z: 0,
        predicate: LocationPredicate {
            dimension: Some(Identifier::vanilla_static("the_nether")),
            ..LocationPredicate::default()
        },
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_location(&view);
    assert!(!condition.test(&mut ctx));
}

#[test]
fn tall_grass_loot_table_encodes_the_double_block_location_check() {
    init_test_registries();

    let pools = vanilla_loot_tables::BLOCKS_TALL_GRASS.pools;
    assert_eq!(pools.len(), 2);

    for (index, pool) in pools.iter().enumerate() {
        let condition = pool
            .conditions
            .iter()
            .find_map(|condition| match condition {
                LootCondition::AllOf(terms) => terms
                    .iter()
                    .find(|term| matches!(term, LootCondition::LocationCheck { .. })),
                _ => None,
            })
            .expect("each tall_grass pool is gated on a location check");

        let LootCondition::LocationCheck {
            offset_y,
            predicate,
            ..
        } = condition
        else {
            unreachable!("the condition was just matched as a location check");
        };
        let expected_offset = if index == 0 { 1 } else { -1 };
        assert_eq!(*offset_y, expected_offset);
        let block = predicate
            .block
            .as_ref()
            .expect("the location check names the neighboring half");
        assert_eq!(
            block.blocks,
            Some(HolderSet::Keys(&["tall_grass"])),
            "the location check names the tall grass block"
        );
    }
}

/// [`LootItemSourceHooks`] with a distinct component map per source, for the
/// `minecraft:copy_components` tests.
struct FakeItemSourceHooks {
    entity: DataComponentMap,
    block_entity: DataComponentMap,
}

impl LootItemSourceHooks for FakeItemSourceHooks {
    fn entity_components(&self, _target: LootContextEntity) -> Option<&DataComponentMap> {
        Some(&self.entity)
    }

    fn block_entity_components(&self) -> Option<&DataComponentMap> {
        Some(&self.block_entity)
    }
}

/// [`ScoreboardValueLookup`] holding one score for one objective and one target.
struct FakeScoreboard {
    objective: &'static str,
    score: i32,
    expected_target: LootContextEntity,
}

impl ScoreboardValueLookup for FakeScoreboard {
    fn get_score(
        &self,
        _ctx: &LootContextRef<'_>,
        target: &ScoreboardTarget,
        objective: &str,
    ) -> Option<i32> {
        // Vanilla `Scoreboard.getPlayerScoreInfo` resolves the entity of the
        // condition's `entity` field, so the target must be that context entity.
        let ScoreboardTarget::Context(entity) = target else {
            return None;
        };
        (objective == self.objective && *entity == self.expected_target).then_some(self.score)
    }
}

/// [`EnvironmentAttributeLookup`] exposing a single attribute value.
struct FakeEnvironmentAttributes {
    attribute: Identifier,
    value: f32,
}

impl EnvironmentAttributeLookup for FakeEnvironmentAttributes {
    fn get_attribute(
        &self,
        attribute: &Identifier,
        _origin: Option<(f64, f64, f64)>,
    ) -> Option<f32> {
        (attribute == &self.attribute).then_some(self.value)
    }
}

/// The `repair_cost`-only filter of the `minecraft:copy_components` test.
static REPAIR_COST_ONLY: [Identifier; 1] = [Identifier::vanilla_static("repair_cost")];

/// The `damage`-only filter of the `minecraft:copy_components` test.
static DAMAGE_ONLY: [Identifier; 1] = [Identifier::vanilla_static("damage")];

/// The `include` list of the `exclude`-wins-over-`include` `copy_components`
/// test.
static REPAIR_COST_AND_DAMAGE: [Identifier; 2] = [
    Identifier::vanilla_static("repair_cost"),
    Identifier::vanilla_static("damage"),
];

/// The `kills` objective range (`5..=10`) of the `minecraft:entity_scores`
/// tests; a `static` is required because [`NumberProvider`] is not `Copy`.
static KILLS_RANGE: [(&str, NumberProviderRange); 1] = [(
    "kills",
    NumberProviderRange {
        min: Some(NumberProvider::Constant(5.0)),
        max: Some(NumberProvider::Constant(10.0)),
    },
)];

/// A one-element [`NumberProvider`] list; a `static` is required because
/// `NumberProvider` is not `Copy`, so an array literal cannot be promoted.
static FLOAT_APPEND: [NumberProvider; 1] = [NumberProvider::Constant(3.5)];

/// The ARGB colour `0x112233`, as the int provider vanilla evaluates.
static COLOR_APPEND: [NumberProvider; 1] = [NumberProvider::Constant(0x11_22_33 as f32)];

fn test_entity(entity_type: &Identifier) -> EntityRef<'_> {
    EntityRef {
        entity_type: Some(entity_type),
        flags: EntityRefFlags::default(),
        equipment: None,
        custom_name: None,
        sheep_color: None,
        sheep_sheared: None,
        chicken_variant: None,
    }
}

fn copy_source_hooks() -> FakeItemSourceHooks {
    let mut entity = DataComponentMap::new();
    entity.set(DAMAGE, Some(5));
    let mut block_entity = DataComponentMap::new();
    block_entity.set(REPAIR_COST, Some(7));
    // A second component so the `exclude` filter has something to keep.
    block_entity.set(DAMAGE, Some(4));
    FakeItemSourceHooks {
        entity,
        block_entity,
    }
}

#[test]
fn copy_components_function_copies_only_the_listed_components() {
    init_test_registries();
    let mut rng = test_rng();
    let hooks = copy_source_hooks();
    let mut ctx = LootContext::new(&mut rng).with_item_source(&hooks);
    let mut stack = ItemStack::new(&vanilla_items::STONE);
    // `minecraft:stone` carries vanilla's `COMMON_ITEM_COMPONENTS` default
    // (`repair_cost: 0`) but no `damage`, so the untouched stack reports the
    // default repair cost rather than `None`.
    assert_eq!(stack.get(REPAIR_COST), Some(&0));
    assert!(stack.get(DAMAGE).is_none());

    // `include` restricts the copy to the named components (vanilla
    // `CopyComponentsFunction.bakedPredicate`).
    LootFunction::CopyComponents {
        source: CopySource::BlockEntity,
        include: &REPAIR_COST_ONLY,
        exclude: &[],
    }
    .apply(&mut stack, &mut ctx);
    assert_eq!(stack.get(REPAIR_COST), Some(&7));
    assert!(stack.get(DAMAGE).is_none());

    // The entity source reads the attacker's `DataComponentGetter` instead.
    LootFunction::CopyComponents {
        source: CopySource::Attacker,
        include: &[],
        exclude: &[],
    }
    .apply(&mut stack, &mut ctx);
    assert_eq!(stack.get(DAMAGE), Some(&5));
    assert_eq!(stack.get(REPAIR_COST), Some(&7));
}

#[test]
fn copy_components_function_without_a_source_hook_leaves_the_stack_untouched() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    let mut stack = ItemStack::new(&vanilla_items::STONE);

    // Vanilla `CopyComponentsFunction.run` returns the stack unchanged when the
    // referenced `DataComponentGetter` is absent.
    LootFunction::CopyComponents {
        source: CopySource::BlockEntity,
        include: &[],
        exclude: &[],
    }
    .apply(&mut stack, &mut ctx);

    // The stack keeps its prototype defaults (`repair_cost: 0`) and gains no
    // `damage` when the referenced `DataComponentGetter` is absent.
    assert_eq!(stack.get(REPAIR_COST), Some(&0));
    assert!(stack.get(DAMAGE).is_none());
}

#[test]
fn copy_components_function_honours_the_exclude_list() {
    init_test_registries();
    let mut rng = test_rng();
    let hooks = copy_source_hooks();
    let mut ctx = LootContext::new(&mut rng).with_item_source(&hooks);
    let mut stack = ItemStack::new(&vanilla_items::STONE);

    // Vanilla `CopyComponentsFunction.exclude` drops the listed components
    // even when an empty `include` copies everything else.
    LootFunction::CopyComponents {
        source: CopySource::BlockEntity,
        include: &[],
        exclude: &REPAIR_COST_ONLY,
    }
    .apply(&mut stack, &mut ctx);
    assert_eq!(stack.get(DAMAGE), Some(&4));
    // `repair_cost` is excluded from the copy, so the stack still reports its
    // prototype default (0) rather than the source's 7.
    assert_eq!(stack.get(REPAIR_COST), Some(&0));

    // ... and an excluded name wins over an explicit `include` of the same
    // name (`CopyComponentsFunction.bakedPredicate` requires both).
    let mut stack = ItemStack::new(&vanilla_items::STONE);
    LootFunction::CopyComponents {
        source: CopySource::BlockEntity,
        include: &REPAIR_COST_AND_DAMAGE,
        exclude: &DAMAGE_ONLY,
    }
    .apply(&mut stack, &mut ctx);
    assert_eq!(stack.get(REPAIR_COST), Some(&7));
    assert!(stack.get(DAMAGE).is_none());
}

#[test]
fn copy_state_function_copies_only_properties_the_block_declares() {
    init_test_registries();
    let mut rng = test_rng();
    let block = Identifier::vanilla_static("bee_nest");
    let state = REGISTRY
        .blocks
        .state_id_from_properties(&block, &[("honey_level", "3")])
        .expect("bee_nest has honey_level");
    let mut ctx = LootContext::new(&mut rng).with_block_state(state);
    let mut stack = ItemStack::new(&vanilla_items::BEE_NEST);

    // Vanilla `CopyBlockState.run`: only properties in the function's resolved
    // set (i.e. declared by `block`) that the looted state carries are written;
    // `bee_count` is not a `bee_nest` property, so it is dropped.
    LootFunction::CopyState {
        block,
        properties: &["honey_level", "bee_count"],
    }
    .apply(&mut stack, &mut ctx);

    let properties = stack
        .get(BLOCK_STATE)
        .expect("copy_state writes the block_state component");
    assert_eq!(properties.get("honey_level"), Some("3"));
    assert_eq!(properties.get("bee_count"), None);
}

#[test]
fn set_custom_model_data_edits_each_list_independently() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    let mut stack = ItemStack::new(&vanilla_items::STONE);
    stack.set(
        CUSTOM_MODEL_DATA,
        CustomModelData::new(
            vec![1.0, 2.0],
            vec![true],
            vec!["base".to_owned()],
            vec![0x11],
        ),
    );

    // Vanilla `SetCustomModelDataFunction.run`: each list is edited on its own
    // (floats appended, strings replaced wholesale) and the lists without an edit
    // keep their entries.
    LootFunction::SetCustomModelData {
        floats: Some(StandaloneListOperation {
            value: &FLOAT_APPEND,
            operation: ListOperation::Append,
        }),
        flags: None,
        strings: Some(StandaloneListOperation {
            value: &["replaced"],
            operation: ListOperation::ReplaceAll,
        }),
        colors: None,
    }
    .apply(&mut stack, &mut ctx);

    let data = stack
        .get(CUSTOM_MODEL_DATA)
        .expect("set_custom_model_data writes the component");
    assert_eq!(data.floats(), [1.0, 2.0, 3.5]);
    assert_eq!(data.flags(), [true]);
    assert_eq!(data.strings(), ["replaced"]);
    assert_eq!(data.colors(), [0x11]);
}

#[test]
fn set_custom_model_data_starts_from_an_empty_component() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    let mut stack = ItemStack::new(&vanilla_items::STONE);
    assert!(stack.get(CUSTOM_MODEL_DATA).is_none());

    // With no existing component vanilla reads `CustomModelData.EMPTY`, so the
    // untouched lists stay empty.
    LootFunction::SetCustomModelData {
        floats: None,
        flags: Some(StandaloneListOperation {
            value: &[true, false],
            operation: ListOperation::Append,
        }),
        strings: None,
        colors: Some(StandaloneListOperation {
            value: &COLOR_APPEND,
            operation: ListOperation::Append,
        }),
    }
    .apply(&mut stack, &mut ctx);

    let data = stack
        .get(CUSTOM_MODEL_DATA)
        .expect("set_custom_model_data writes the component");
    assert_eq!(data.floats(), [] as [f32; 0]);
    assert_eq!(data.flags(), [true, false]);
    assert_eq!(data.strings(), [] as [String; 0]);
    assert_eq!(data.colors(), [0x11_22_33]);
}

#[test]
fn entity_scores_requires_a_live_score_inside_the_range() {
    init_test_registries();
    let pig_key = Identifier::vanilla_static("pig");
    let condition = LootCondition::EntityScores {
        entity: LootContextEntity::This,
        scores: &[
            (
                "kills",
                NumberProviderRange {
                    min: Some(NumberProvider::Constant(5.0)),
                    max: Some(NumberProvider::Constant(10.0)),
                },
            ),
            (
                "deaths",
                NumberProviderRange {
                    min: None,
                    max: None,
                },
            ),
        ],
    };
    let scoreboard = FakeScoreboard {
        objective: "kills",
        score: 7,
        expected_target: LootContextEntity::This,
    };

    // Both objectives must resolve: an unknown objective fails the condition.
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_this_entity(test_entity(&pig_key))
        .with_scoreboard(&scoreboard);
    assert!(!condition.test(&mut ctx));

    // The range is inclusive on both ends (vanilla `IntRangePredicate`).
    let condition = LootCondition::EntityScores {
        entity: LootContextEntity::This,
        scores: &[(
            "kills",
            NumberProviderRange {
                min: Some(NumberProvider::Constant(5.0)),
                max: Some(NumberProvider::Constant(10.0)),
            },
        )],
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_this_entity(test_entity(&pig_key))
        .with_scoreboard(&scoreboard);
    assert!(condition.test(&mut ctx));
}

#[test]
fn entity_scores_fails_without_the_entity_score_or_hook() {
    init_test_registries();
    let pig_key = Identifier::vanilla_static("pig");
    let condition = LootCondition::EntityScores {
        entity: LootContextEntity::This,
        scores: &KILLS_RANGE,
    };
    let scoreboard = FakeScoreboard {
        objective: "kills",
        score: 7,
        expected_target: LootContextEntity::This,
    };

    // No entity in the context: vanilla `EntityHasScoreCondition.test` returns
    // false before touching the scoreboard.
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng).with_scoreboard(&scoreboard);
    assert!(!condition.test(&mut ctx));

    // Score outside the range.
    let low = FakeScoreboard {
        objective: "kills",
        score: 4,
        expected_target: LootContextEntity::This,
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_this_entity(test_entity(&pig_key))
        .with_scoreboard(&low);
    assert!(!condition.test(&mut ctx));

    // The entity is present but the condition reads a different target.
    let other = FakeScoreboard {
        objective: "kills",
        score: 7,
        expected_target: LootContextEntity::Killer,
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_this_entity(test_entity(&pig_key))
        .with_scoreboard(&other);
    assert!(!condition.test(&mut ctx));

    // No scoreboard hook installed.
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng).with_this_entity(test_entity(&pig_key));
    assert!(!condition.test(&mut ctx));
}

#[test]
fn environment_attribute_check_compares_the_attribute_at_the_origin() {
    init_test_registries();
    let attribute = Identifier::vanilla_static("gameplay/sky_light_level");
    let hook = FakeEnvironmentAttributes {
        attribute: attribute.clone(),
        value: 1.0,
    };
    let condition = LootCondition::EnvironmentAttributeCheck {
        attribute: attribute.clone(),
        value: 1.0,
    };

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_environment_attributes(&hook);
    assert!(condition.test(&mut ctx));

    // Vanilla `EnvironmentAttributeCheck.test` compares for equality, so a
    // different configured value fails.
    let mismatched = LootCondition::EnvironmentAttributeCheck {
        attribute,
        value: 2.0,
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_environment_attributes(&hook);
    assert!(!mismatched.test(&mut ctx));

    // An attribute the environment does not carry fails, as does a context with
    // no attribute source installed.
    let unknown = LootCondition::EnvironmentAttributeCheck {
        attribute: Identifier::vanilla_static("gameplay/unknown_attribute"),
        value: 1.0,
    };
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng)
        .with_origin(0.5, 64.0, 0.5)
        .with_environment_attributes(&hook);
    assert!(!unknown.test(&mut ctx));

    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng).with_origin(0.5, 64.0, 0.5);
    assert!(!condition.test(&mut ctx));
}

#[test]
fn unresolved_condition_references_fail_closed() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);

    // Steel inlines `minecraft:<path>` condition references at build time, so an
    // unresolved reference must not silently pass.
    assert!(
        !LootCondition::Reference(Identifier::vanilla_static("tool/can_silk_touch")).test(&mut ctx)
    );
}

/// Vanilla `ShulkerBoxBlock.CONTENTS`: the dynamic drop a container block
/// entity registers for its own slots.
fn dynamic_contents_entry() -> LootEntry {
    LootEntry::Dynamic {
        name: Identifier::vanilla_static("contents"),
        conditions: &[],
    }
}

fn block_entity_with(items: &[ItemStack]) -> BlockEntityRef<'_> {
    BlockEntityRef {
        block_entity_type: None,
        custom_name: None,
        inventory: Some(items),
    }
}

#[test]
fn dynamic_contents_entry_yields_the_containers_stacks() {
    init_test_registries();
    let mut rng = test_rng();
    let inventory = [
        ItemStack::with_count(&vanilla_items::DIAMOND, 3),
        ItemStack::empty(),
        ItemStack::with_count(&vanilla_items::STICK, 2),
    ];
    let mut ctx = LootContext::new(&mut rng).with_block_entity(block_entity_with(&inventory));

    let result = dynamic_contents_entry().expand_items(&mut ctx);

    // Mirrors the container drop: every non-empty slot, in order, empties gone.
    assert_eq!(
        result,
        [
            ItemStack::with_count(&vanilla_items::DIAMOND, 3),
            ItemStack::with_count(&vanilla_items::STICK, 2),
        ]
    );
}

#[test]
fn dynamic_contents_entry_without_a_container_yields_nothing() {
    init_test_registries();
    let entry = dynamic_contents_entry();

    // No block entity in context: vanilla's absent dynamic-drop map lookup.
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);
    assert!(entry.expand_items(&mut ctx).is_empty());

    // A block entity with no inventory: same absent-drop fallback.
    let mut ctx = LootContext::new(&mut rng).with_block_entity(BlockEntityRef {
        block_entity_type: None,
        custom_name: None,
        inventory: None,
    });
    assert!(entry.expand_items(&mut ctx).is_empty());
}

#[test]
fn dynamic_entry_with_an_unknown_name_yields_nothing() {
    init_test_registries();
    let mut rng = test_rng();
    let inventory = [ItemStack::with_count(&vanilla_items::DIAMOND, 3)];
    let mut ctx = LootContext::new(&mut rng).with_block_entity(block_entity_with(&inventory));

    // Only the ids a block registers reach its dynamic drops (`minecraft:sherds`
    // comes from a decorated pot's own data, which Steel's pot behaviour supplies
    // through `get_drops` instead of the context).
    let entry = LootEntry::Dynamic {
        name: Identifier::vanilla_static("sherds"),
        conditions: &[],
    };
    assert!(entry.expand_items(&mut ctx).is_empty());
}

/// A `LootSlotSourceResolver` over a fixed stack list, mirroring how the
/// runtime plumbs a container snapshot into the context.
struct TestSlotSources<'a> {
    items: &'a [ItemStack],
}

impl crate::loot_table::LootSlotSourceResolver for TestSlotSources<'_> {
    fn slot_source_items(
        &self,
        _source: &'static crate::slot_source::SlotSourceHolder,
    ) -> Vec<ItemStack> {
        self.items.to_vec()
    }
}

fn slots_entry() -> LootEntry {
    static SOURCE: crate::slot_source::SlotSourceHolder =
        crate::slot_source::SlotSourceHolder::Inline(&crate::slot_source::SlotSource::Empty(
            crate::slot_source::EmptySlotSource,
        ));
    LootEntry::Slots {
        slot_source: &SOURCE,
        conditions: &[],
        functions: &[],
    }
}

#[test]
fn slots_entry_yields_the_slots_the_source_resolves() {
    init_test_registries();
    let mut rng = test_rng();
    let items = [
        ItemStack::with_count(&vanilla_items::DIAMOND, 3),
        ItemStack::empty(),
    ];
    let resolver = TestSlotSources { items: &items };
    let mut ctx = LootContext::new(&mut rng).with_slot_sources(&resolver);

    // Vanilla `SlotLoot` drops the empty slots.
    assert_eq!(
        slots_entry().expand_items(&mut ctx),
        [ItemStack::with_count(&vanilla_items::DIAMOND, 3)]
    );
}

#[test]
fn slots_entry_without_a_resolver_yields_nothing() {
    init_test_registries();
    let mut rng = test_rng();
    let mut ctx = LootContext::new(&mut rng);

    assert!(slots_entry().expand_items(&mut ctx).is_empty());
}

#[test]
fn slots_entry_applies_entry_functions() {
    init_test_registries();
    let mut rng = test_rng();
    static LIMITED_FUNCTIONS: &[ConditionalLootFunction] = &[ConditionalLootFunction {
        function: LootFunction::LimitCount {
            min: None,
            max: Some(1),
        },
        conditions: &[],
    }];
    static SOURCE: crate::slot_source::SlotSourceHolder =
        crate::slot_source::SlotSourceHolder::Inline(&crate::slot_source::SlotSource::Empty(
            crate::slot_source::EmptySlotSource,
        ));
    let items = [ItemStack::with_count(&vanilla_items::DIAMOND, 3)];
    let resolver = TestSlotSources { items: &items };
    let mut ctx = LootContext::new(&mut rng).with_slot_sources(&resolver);
    let entry = LootEntry::Slots {
        slot_source: &SOURCE,
        conditions: &[],
        functions: LIMITED_FUNCTIONS,
    };

    assert_eq!(
        entry.expand_items(&mut ctx),
        [ItemStack::with_count(&vanilla_items::DIAMOND, 1)]
    );
}
