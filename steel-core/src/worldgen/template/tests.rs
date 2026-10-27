use std::collections::BTreeMap;
use std::slice;
use std::sync::Arc;

use rand::SeedableRng as _;
use rand::rngs::StdRng;

use super::*;
use steel_registry::REGISTRY;
use steel_registry::blocks::properties::{DoorHingeSide, SlabType};
use steel_registry::init_vanilla_registry;
use steel_registry::item_stack::ItemStack;
use steel_registry::loot_table::LootContext;
use steel_registry::vanilla_custom_stats;
use steel_registry::vanilla_entities;
use steel_utils::ChunkPos;

use crate::behavior::blocks::open_menu;
use crate::behavior::{BLOCK_BEHAVIORS, init_behaviors};
use crate::block_entity::init_block_entities;
use crate::inventory::lock::ContainerLockGuard;
use crate::test_support::{
    TestPlayerBuilder, TestWorld, fresh_test_world, insert_ready_full_chunk,
};
use crate::world::{LevelReader as _, World};

fn test_registry() -> Registry {
    init_vanilla_registry();
    Registry::new_vanilla()
}

#[test]
fn palette_blocks_skips_all_out_of_bounds_for_current_chunk_processors() {
    let blocks = [StructureBlockInfo {
        pos: BlockPos::new(32, 0, 0),
        state: BlockStateId(0),
        nbt: None,
    }];
    let settings = StructurePlaceSettings {
        mirror: StructureMirror::None,
        rotation: Rotation::None,
        rotation_pivot: BlockPos::ZERO,
        bounding_box: BoundingBox::new(IVec3::ZERO, IVec3::new(15, 255, 15)),
        processors: &[],
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: StructureProcessorRandom::Positional,
        liquid_settings: LiquidSettingsData::IgnoreWaterlogging,
    };
    let mut processed = 0;

    StructureTemplate::palette_blocks_for_placement(&blocks, BlockPos::ZERO, &settings, |_, _| {
        processed += 1;
    });

    assert_eq!(processed, 0);
}

#[test]
fn palette_blocks_keeps_out_of_bounds_for_capped_processors() {
    let blocks = [StructureBlockInfo {
        pos: BlockPos::new(32, 0, 0),
        state: BlockStateId(0),
        nbt: None,
    }];
    let capped = StructureProcessorKind::Capped {
        delegate: Box::new(StructureProcessorKind::LavaSubmergedBlock),
        limit: IntProvider::Constant(1),
    };
    let settings = StructurePlaceSettings {
        mirror: StructureMirror::None,
        rotation: Rotation::None,
        rotation_pivot: BlockPos::ZERO,
        bounding_box: BoundingBox::new(IVec3::ZERO, IVec3::new(15, 255, 15)),
        processors: slice::from_ref(&capped),
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: StructureProcessorRandom::Positional,
        liquid_settings: LiquidSettingsData::IgnoreWaterlogging,
    };
    let mut processed = 0;
    let mut processed_pos = None;

    StructureTemplate::palette_blocks_for_placement(
        &blocks,
        BlockPos::ZERO,
        &settings,
        |_, pos| {
            processed += 1;
            processed_pos = Some(pos);
        },
    );

    assert_eq!(processed, 1);
    assert_eq!(processed_pos, Some(BlockPos::new(32, 0, 0)));
}

#[test]
fn zero_position_with_transform_matches_vanilla_rotation_offsets() {
    let template = StructureTemplate {
        size: IVec3::new(6, 10, 8),
        palettes: Vec::new(),
        entities: Vec::new(),
    };
    let zero = BlockPos::new(100, 64, 200);

    assert_eq!(
        template.zero_position_with_transform(zero, Rotation::None),
        zero
    );
    assert_eq!(
        template.zero_position_with_transform(zero, Rotation::Clockwise90),
        BlockPos::new(107, 64, 200)
    );
    assert_eq!(
        template.zero_position_with_transform(zero, Rotation::Clockwise180),
        BlockPos::new(105, 64, 207)
    );
    assert_eq!(
        template.zero_position_with_transform(zero, Rotation::CounterClockwise90),
        BlockPos::new(100, 64, 205)
    );
}

#[test]
fn bounding_box_with_transform_matches_vanilla_mirror_rotation_pivot() {
    let template = StructureTemplate {
        size: IVec3::new(6, 10, 8),
        palettes: Vec::new(),
        entities: Vec::new(),
    };

    assert_eq!(
        template.bounding_box_with_transform(
            BlockPos::new(100, 64, 200),
            Rotation::Clockwise90,
            StructureMirror::FrontBack,
            BlockPos::new(2, 0, 3),
        ),
        BoundingBox::new(IVec3::new(98, 64, 196), IVec3::new(105, 73, 201))
    );
    assert_eq!(
        template.bounding_box_with_transform(
            BlockPos::new(100, 64, 200),
            Rotation::CounterClockwise90,
            StructureMirror::LeftRight,
            BlockPos::new(2, 0, 3),
        ),
        BoundingBox::new(IVec3::new(92, 64, 200), IVec3::new(99, 73, 205))
    );
}

#[test]
fn block_pos_seed_matches_vanilla_mth_get_seed() {
    assert_eq!(
        StructureTemplate::block_pos_seed(BlockPos::new(12, -3, 45)),
        103_080_484_998_711
    );
}

#[test]
fn village_template_loads_entity_payloads() {
    let registry = test_registry();
    let template = StructureTemplate::load_vanilla(
        &registry,
        &Identifier::vanilla_static("village/plains/villagers/unemployed"),
    )
    .expect("villager template should be bundled");

    assert_eq!(template.entities.len(), 1);
    assert_eq!(
        &template.entities[0].entity_type.key,
        &vanilla_entities::VILLAGER.key
    );
    assert!(template.entities[0].nbt.contains("VillagerData"));
    assert!(!template.entities[0].nbt.contains("id"));
}

#[test]
fn template_block_entity_resolution_uses_final_block_state() {
    let registry = test_registry();
    let suspicious_sand = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::SUSPICIOUS_SAND);
    let mut brushable_nbt = NbtCompound::new();
    brushable_nbt.insert("LootTable", "minecraft:archaeology/ocean_ruin_warm");
    brushable_nbt.insert("LootTableSeed", 42_i64);

    let brushable_type = StructureTemplate::block_entity_type_for_nbt_or_state(
        &registry,
        suspicious_sand,
        &brushable_nbt,
    )
    .expect("suspicious sand should infer brushable block entity");

    assert_eq!(
        &brushable_type.key,
        &vanilla_block_entity_types::BRUSHABLE_BLOCK.key
    );
    assert!(!StructureTemplate::should_reseed_template_loot(
        Some(brushable_type),
        &brushable_nbt
    ));

    let mut chest_nbt = NbtCompound::new();
    chest_nbt.insert("id", "minecraft:chest");
    chest_nbt.insert("LootTable", "minecraft:chests/village/village_weaponsmith");
    let chest_type = StructureTemplate::block_entity_type_for_nbt_or_state(
        &registry,
        registry.blocks.get_default_state_id(&vanilla_blocks::CHEST),
        &chest_nbt,
    )
    .expect("chest nbt should resolve block entity type");

    assert!(StructureTemplate::should_reseed_template_loot(
        Some(chest_type),
        &chest_nbt
    ));

    assert!(
        StructureTemplate::block_entity_type_for_nbt_or_state(
            &registry,
            registry.blocks.get_default_state_id(&vanilla_blocks::LAVA),
            &chest_nbt,
        )
        .is_none()
    );
}

#[test]
fn entity_position_and_rotation_transform_match_vanilla_offsets() {
    let pos = DVec3::new(1.25, 2.0, 3.75);
    let pivot = BlockPos::new(2, 0, 3);

    assert_eq!(
        StructureTemplate::transform_entity_position(
            pos,
            StructureMirror::FrontBack,
            Rotation::Clockwise90,
            pivot,
        ),
        DVec3::new(2.25, 2.0, 0.75)
    );
    assert_eq!(
        StructureTemplate::transform_entity_rotation(
            (30.0, 10.0),
            StructureMirror::LeftRight,
            Rotation::Clockwise90,
        ),
        (240.0, 10.0)
    );
    assert_eq!(
        StructureTemplate::transform_entity_rotation(
            (30.0, 10.0),
            StructureMirror::FrontBack,
            Rotation::Clockwise90,
        ),
        (60.0, 10.0)
    );
}

#[test]
fn hanging_entity_facing_applies_rotation_before_mirror() {
    let mut nbt = NbtCompound::new();
    nbt.insert(
        "Facing",
        StructureTemplate::entity_facing_value(Direction::North),
    );

    StructureTemplate::transform_entity_additional_nbt(
        &mut nbt,
        StructureMirror::LeftRight,
        Rotation::Clockwise90,
    );

    assert_eq!(
        nbt.byte("Facing"),
        Some(StructureTemplate::entity_facing_value(Direction::East))
    );
}

#[test]
fn mirrored_door_transform_toggles_hinge() {
    let registry = test_registry();
    let door = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::SPRUCE_DOOR);
    let door = registry.blocks.set_property(
        door,
        &BlockStateProperties::HORIZONTAL_FACING,
        Direction::East,
    );
    let door =
        registry
            .blocks
            .set_property(door, &BlockStateProperties::DOOR_HINGE, DoorHingeSide::Left);

    let mirrored = StructureTemplate::transform_state(
        &registry,
        door,
        StructureMirror::FrontBack,
        Rotation::None,
    );

    assert_eq!(
        registry
            .blocks
            .try_get_property(mirrored, &BlockStateProperties::HORIZONTAL_FACING),
        Some(Direction::West),
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(mirrored, &BlockStateProperties::DOOR_HINGE),
        Some(DoorHingeSide::Right),
    );
}

#[test]
fn chiseled_bookshelf_rotation_and_mirror_preserve_occupied_slots() {
    let registry = test_registry();
    let occupied_properties = [
        &BlockStateProperties::SLOT_0_OCCUPIED,
        &BlockStateProperties::SLOT_4_OCCUPIED,
    ];
    let mut bookshelf = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::CHISELED_BOOKSHELF);
    bookshelf = registry.blocks.set_property(
        bookshelf,
        &BlockStateProperties::HORIZONTAL_FACING,
        Direction::North,
    );
    for property in occupied_properties {
        bookshelf = registry.blocks.set_property(bookshelf, property, true);
    }

    let rotated = StructureTemplate::transform_state(
        &registry,
        bookshelf,
        StructureMirror::None,
        Rotation::Clockwise90,
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(rotated, &BlockStateProperties::HORIZONTAL_FACING),
        Some(Direction::East),
    );

    let mirrored = StructureTemplate::transform_state(
        &registry,
        bookshelf,
        StructureMirror::LeftRight,
        Rotation::None,
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(mirrored, &BlockStateProperties::HORIZONTAL_FACING),
        Some(Direction::South),
    );

    for transformed in [rotated, mirrored] {
        for property in occupied_properties {
            assert_eq!(
                registry.blocks.try_get_property(transformed, property),
                Some(true),
            );
        }
    }
}

#[test]
fn jigsaw_replacement_uses_final_state_and_removes_nbt() {
    let registry = test_registry();
    let mut nbt = NbtCompound::new();
    nbt.insert(
        "final_state",
        NbtTag::String("minecraft:oak_stairs[facing=east,half=top]".into()),
    );
    let current = ProcessedBlockInfo {
        template_pos: BlockPos::ZERO,
        world_pos: BlockPos::new(1, 2, 3),
        state: registry
            .blocks
            .get_default_state_id(&vanilla_blocks::JIGSAW),
        nbt: Some(nbt),
    };

    let replaced = StructureTemplate::replace_jigsaw_block(&registry, current)
        .expect("non-structure-void final state should remain");

    assert_eq!(replaced.nbt, None);
    assert_eq!(
        replaced.state,
        StructureTemplate::parse_block_state_string(
            &registry,
            "minecraft:oak_stairs[facing=east,half=top]"
        )
        .expect("test final state should parse")
    );
}

#[test]
fn jigsaw_replacement_accepts_trailing_text_like_vanilla_parser() {
    let registry = test_registry();
    let final_state =
        "minecraft:acacia_fence[east=false,north=false,south=false,waterlogged=false,west=false]]";
    let expected =
        "minecraft:acacia_fence[east=false,north=false,south=false,waterlogged=false,west=false]";

    assert_eq!(
        StructureTemplate::parse_block_state_string(&registry, final_state),
        StructureTemplate::parse_block_state_string(&registry, expected)
    );
}

#[test]
fn jigsaw_replacement_drops_structure_void_final_state() {
    let registry = test_registry();
    let mut nbt = NbtCompound::new();
    nbt.insert(
        "final_state",
        NbtTag::String("minecraft:structure_void".into()),
    );
    let current = ProcessedBlockInfo {
        template_pos: BlockPos::ZERO,
        world_pos: BlockPos::new(1, 2, 3),
        state: registry
            .blocks
            .get_default_state_id(&vanilla_blocks::JIGSAW),
        nbt: Some(nbt),
    };

    assert!(StructureTemplate::replace_jigsaw_block(&registry, current).is_none());
}

#[test]
fn structure_block_ignore_modes_match_vanilla_single_variants() {
    let registry = test_registry();
    let structure_block = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::STRUCTURE_BLOCK);
    let air = registry.blocks.get_default_state_id(&vanilla_blocks::AIR);
    let stone = registry.blocks.get_default_state_id(&vanilla_blocks::STONE);

    assert!(StructureBlockIgnore::StructureBlock.ignores(&registry, structure_block));
    assert!(!StructureBlockIgnore::StructureBlock.ignores(&registry, air));
    assert!(StructureBlockIgnore::StructureAndAir.ignores(&registry, structure_block));
    assert!(StructureBlockIgnore::StructureAndAir.ignores(&registry, air));
    assert!(!StructureBlockIgnore::StructureAndAir.ignores(&registry, stone));
}

#[test]
fn block_age_processor_preserves_slab_properties() {
    let registry = test_registry();
    let slab = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::STONE_BRICK_SLAB);
    let slab = registry
        .blocks
        .set_property(slab, &BlockStateProperties::SLAB_TYPE, SlabType::Top);
    let current = ProcessedBlockInfo {
        template_pos: BlockPos::ZERO,
        world_pos: BlockPos::new(12, 70, -4),
        state: slab,
        nbt: None,
    };
    let mut random = LegacyRandom::from_seed(1);

    let processed =
        StructureTemplate::process_block_age_with_random(&registry, current, 1.0, &mut random);

    assert_eq!(
        StructureTemplate::block_for_state(&registry, processed.state),
        &vanilla_blocks::MOSSY_STONE_BRICK_SLAB
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(processed.state, &BlockStateProperties::SLAB_TYPE),
        Some(SlabType::Top),
    );
}

#[test]
fn lava_submerged_processor_keeps_non_full_blocks_as_lava() {
    let registry = test_registry();
    let slab = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::STONE_BRICK_SLAB);
    let current = ProcessedBlockInfo {
        template_pos: BlockPos::ZERO,
        world_pos: BlockPos::new(0, 64, 0),
        state: slab,
        nbt: None,
    };
    let lava = registry.blocks.get_default_state_id(&vanilla_blocks::LAVA);

    let processed = StructureTemplate::process_lava_submerged_block(&registry, lava, current);

    assert_eq!(
        StructureTemplate::block_for_state(&registry, processed.state),
        &vanilla_blocks::LAVA
    );
}

#[test]
fn blackstone_replace_processor_preserves_stair_orientation() {
    let registry = test_registry();
    let stairs = registry
        .blocks
        .get_default_state_id(&vanilla_blocks::STONE_BRICK_STAIRS);
    let stairs =
        registry
            .blocks
            .set_property(stairs, &BlockStateProperties::FACING, Direction::East);
    let stairs = registry
        .blocks
        .set_property(stairs, &BlockStateProperties::HALF, Half::Top);
    let current = ProcessedBlockInfo {
        template_pos: BlockPos::ZERO,
        world_pos: BlockPos::new(0, 64, 0),
        state: stairs,
        nbt: None,
    };

    let processed = StructureTemplate::process_blackstone_replace(&registry, current);

    assert_eq!(
        StructureTemplate::block_for_state(&registry, processed.state),
        &vanilla_blocks::POLISHED_BLACKSTONE_BRICK_STAIRS,
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(processed.state, &BlockStateProperties::FACING),
        Some(Direction::East),
    );
    assert_eq!(
        registry
            .blocks
            .try_get_property(processed.state, &BlockStateProperties::HALF),
        Some(Half::Top),
    );
}

#[test]
fn data_markers_read_shipwreck_structure_blocks() {
    let registry = test_registry();
    let template = StructureTemplate::load_vanilla(
        &registry,
        &Identifier::vanilla_static("shipwreck/with_mast"),
    )
    .expect("shipwreck template should be bundled");
    let settings = StructurePlaceSettings {
        mirror: StructureMirror::None,
        rotation: Rotation::Clockwise90,
        rotation_pivot: BlockPos::new(4, 0, 15),
        bounding_box: BoundingBox::new(IVec3::new(-64, 0, -64), IVec3::new(64, 128, 64)),
        processors: &[],
        block_ignore: StructureBlockIgnore::StructureAndAir,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: StructureProcessorRandom::Positional,
        liquid_settings: LiquidSettingsData::ApplyWaterlogging,
    };
    let mut random = WorldgenRandom::from_seed(0);

    let mut markers = template
        .data_markers(&registry, BlockPos::ZERO, &settings, &mut random)
        .into_iter()
        .map(|marker| marker.metadata)
        .collect::<Vec<_>>();
    markers.sort();

    assert_eq!(markers, ["map_chest", "supply_chest", "treasure_chest"]);
}

#[test]
fn data_markers_read_igloo_chest_structure_block() {
    let registry = test_registry();
    let template =
        StructureTemplate::load_vanilla(&registry, &Identifier::vanilla_static("igloo/bottom"))
            .expect("igloo bottom template should be bundled");
    let settings = StructurePlaceSettings {
        mirror: StructureMirror::None,
        rotation: Rotation::Clockwise180,
        rotation_pivot: BlockPos::new(3, 6, 7),
        bounding_box: BoundingBox::new(IVec3::new(-64, 0, -64), IVec3::new(64, 128, 64)),
        processors: &[],
        block_ignore: StructureBlockIgnore::StructureBlock,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: StructureProcessorRandom::Positional,
        liquid_settings: LiquidSettingsData::IgnoreWaterlogging,
    };
    let mut random = WorldgenRandom::from_seed(0);

    let markers = template
        .data_markers(&registry, BlockPos::ZERO, &settings, &mut random)
        .into_iter()
        .map(|marker| marker.metadata)
        .collect::<Vec<_>>();

    assert_eq!(markers, ["chest"]);
}

// ---------------------------------------------------------------------------
// Structure chest loot
// ---------------------------------------------------------------------------

/// The placement random every chest test places its template with, so the
/// reseeded `LootTableSeed` is reproducible.
const CHEST_PLACEMENT_SEED: u64 = 0x5EED_1007;

/// Test world with the block entity registries installed and ready full chunks
/// for a template at `pos`.
fn chest_test_world(key: &'static str, pos: BlockPos, size: IVec3) -> TestWorld {
    init_vanilla_registry();
    init_behaviors();
    init_block_entities();
    let fixture = fresh_test_world(key);
    for chunk_x in (pos.x() >> 4)..=((pos.x() + size.x - 1) >> 4) {
        for chunk_z in (pos.z() >> 4)..=((pos.z() + size.z - 1) >> 4) {
            insert_ready_full_chunk(&fixture.world, ChunkPos::new(chunk_x, chunk_z));
        }
    }
    fixture
}

/// Vanilla `StructurePlaceSettings` for placing a template into a live world
/// with the same flags and processor random the structure piece placer uses.
fn placement_settings(pos: BlockPos, size: IVec3) -> StructurePlaceSettings<'static> {
    StructurePlaceSettings {
        mirror: StructureMirror::None,
        rotation: Rotation::None,
        rotation_pivot: BlockPos::ZERO,
        // Vanilla's `placeTemplate` clip is infinite; a box around the template
        // bounds every write for the tests without changing a single position.
        bounding_box: BoundingBox::new(
            IVec3::new(pos.x() - 16, pos.y() - 64, pos.z() - 16),
            IVec3::new(
                pos.x() + size.x + 15,
                pos.y() + size.y + 64,
                pos.z() + size.z + 15,
            ),
        ),
        processors: &[],
        block_ignore: StructureBlockIgnore::None,
        late_block_ignore: StructureBlockIgnore::None,
        replace_jigsaws: false,
        projection: None,
        processor_random: StructureProcessorRandom::Positional,
        liquid_settings: LiquidSettingsData::IgnoreWaterlogging,
    }
}

/// Places `template` into `world` the way a structure piece does.
fn place_test_template(
    world: &Arc<World>,
    registry: &Registry,
    template: &StructureTemplate,
    pos: BlockPos,
) {
    let settings = placement_settings(pos, template.unrotated_size());
    let mut level = LiveLevel::new(world);
    let mut random = WorldgenRandom::from_seed(CHEST_PLACEMENT_SEED);
    assert!(
        template.place_in_world(
            &mut level,
            registry,
            pos,
            pos,
            &settings,
            &mut random,
            UpdateFlags::UPDATE_CLIENTS,
        ),
        "test template should place at least one block"
    );
}

/// The block entity NBT a chest keeps: the pending loot table as vanilla's
/// `trySaveLootTable` writes it, or its items once the table was unpacked.
fn saved_chest_nbt(world: &Arc<World>, pos: BlockPos) -> NbtCompound {
    world
        .get_block_entity(pos)
        .expect("placed chest should have a block entity")
        .save_custom_only()
}

/// Vanilla `RandomizableContainer.getLootTable`/`getLootTableSeed` as they
/// survive a save: the pending table of an unopened structure chest.
fn pending_loot_table(world: &Arc<World>, pos: BlockPos) -> Option<(Identifier, i64)> {
    let nbt = saved_chest_nbt(world, pos);
    let table = nbt.string("LootTable")?;
    let table = Identifier::from_str(&table.to_string()).expect("saved loot table key");
    Some((table, nbt.long("LootTableSeed").unwrap_or(0)))
}

/// The stacks a chest currently holds, snapshotted without keeping the lock.
fn chest_items(world: &Arc<World>, pos: BlockPos) -> Vec<ItemStack> {
    let container = world
        .get_block_entity(pos)
        .and_then(|entity| entity.container_ref())
        .expect("chest should expose its container");
    let guard = ContainerLockGuard::lock_all(&[&container]);
    guard
        .get(container.container_id())
        .expect("locked chest container")
        .items()
        .to_vec()
}

/// Total count per item, so a stack split across slots compares equal.
fn item_totals(items: &[ItemStack]) -> BTreeMap<Identifier, i32> {
    let mut totals: BTreeMap<Identifier, i32> = BTreeMap::new();
    for stack in items {
        if stack.is_empty() {
            continue;
        }
        *totals.entry(stack.item().key.clone()).or_default() += stack.count();
    }
    totals
}

/// Empties a chest, the way a player taking the loot out does.
fn take_all_items(world: &Arc<World>, pos: BlockPos) {
    let container = world
        .get_block_entity(pos)
        .and_then(|entity| entity.container_ref())
        .expect("chest should expose its container");
    let mut guard = ContainerLockGuard::lock_all(&[&container]);
    let container = guard
        .get_mut(container.container_id())
        .expect("locked chest container");
    for slot in 0..container.get_container_size() {
        container.set_item(slot, ItemStack::empty());
    }
}

/// The items vanilla `LootTable.fill` must end up placing for `table` with
/// `seed`: the table evaluated on its own, without the container path.
fn expected_loot(table: &Identifier, seed: i64, pos: BlockPos) -> BTreeMap<Identifier, i32> {
    let table = REGISTRY
        .loot_tables
        .by_key(table)
        .expect("village loot table should be registered");
    let origin = (
        f64::from(pos.x()) + 0.5,
        f64::from(pos.y()) + 0.5,
        f64::from(pos.z()) + 0.5,
    );
    // Vanilla `LootTable.fill` rolls the items before it shuffles them, so the
    // generated stacks are the same as a bare `getRandomItems` with that seed.
    let mut rng = StdRng::seed_from_u64(seed as u64);
    let mut context = LootContext::new(&mut rng).with_origin(origin.0, origin.1, origin.2);
    item_totals(&table.get_random_items(&mut context))
}

/// The chest block of a one-block template carrying `loot_table`.
fn loot_chest_template(registry: &Registry, loot_table: &str) -> StructureTemplate {
    let mut nbt = NbtCompound::new();
    nbt.insert("id", "minecraft:chest");
    nbt.insert("LootTable", loot_table);
    // Overwritten by placement's reseed, exactly like a vanilla template seed.
    nbt.insert("LootTableSeed", 1234_i64);
    StructureTemplate {
        size: IVec3::new(1, 1, 1),
        palettes: vec![StructureTemplatePalette {
            blocks: vec![StructureBlockInfo {
                pos: BlockPos::ZERO,
                state: registry.blocks.get_default_state_id(&vanilla_blocks::CHEST),
                nbt: Some(nbt),
            }],
        }],
        entities: Vec::new(),
    }
}

/// A chest placed from a template with a `LootTable` must generate exactly that
/// table's loot the first time it is opened, and never again.
#[test]
fn structure_chest_rolls_its_loot_table_on_first_open_only() {
    let registry = test_registry();
    let pos = BlockPos::new(4, 64, 4);
    let fixture = chest_test_world("chest_loot_first_open", pos, IVec3::new(1, 1, 1));
    let world = &fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "ChestLooter", 1).build();
    let template = loot_chest_template(&registry, "minecraft:chests/village/village_weaponsmith");

    place_test_template(world, &registry, &template, pos);

    let (table, seed) =
        pending_loot_table(world, pos).expect("the template's loot table must survive placement");
    assert_eq!(
        table,
        Identifier::vanilla_static("chests/village/village_weaponsmith")
    );
    assert_ne!(
        seed, 0,
        "vanilla `StructureTemplate.placeInWorld` reseeds a randomizable container"
    );
    assert!(
        chest_items(world, pos).iter().all(ItemStack::is_empty),
        "an unopened structure chest holds no items"
    );

    // First open: vanilla's chest menu provider unpacks the pending table with
    // the opening player before the menu is built.
    let state = world.get_block_state(pos);
    open_menu(
        state,
        world,
        pos,
        &player,
        &vanilla_custom_stats::OPEN_CHEST,
    );

    let opened = item_totals(&chest_items(world, pos));
    assert!(
        !opened.is_empty(),
        "a village chest must hold loot after its first open"
    );
    assert_eq!(
        opened,
        expected_loot(&table, seed, pos),
        "the chest must hold exactly the loot table's items for its seed"
    );

    // Taking the loot and opening again must not reroll the table.
    take_all_items(world, pos);
    open_menu(
        state,
        world,
        pos,
        &player,
        &vanilla_custom_stats::OPEN_CHEST,
    );
    assert!(
        chest_items(world, pos).iter().all(ItemStack::is_empty),
        "an unpacked loot table must not roll a second time"
    );
    assert!(
        saved_chest_nbt(world, pos).string("LootTable").is_none(),
        "an unpacked chest saves its items instead of the table"
    );
}

/// The same template placed with the same placement random must land the same
/// `LootTableSeed`, and that seed alone must drive the loot: the loot a village
/// chest yields is fixed by its structure placement.
#[test]
fn structure_chest_loot_is_deterministic_per_placement_seed() {
    let registry = test_registry();
    let first_pos = BlockPos::new(4, 64, 4);
    let second_pos = BlockPos::new(36, 64, 4);
    let fixture = chest_test_world("chest_loot_deterministic", first_pos, IVec3::new(32, 1, 1));
    let world = &fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "SameSeedLooter", 1).build();
    let template = loot_chest_template(&registry, "minecraft:chests/village/village_plains_house");

    place_test_template(world, &registry, &template, first_pos);
    place_test_template(world, &registry, &template, second_pos);

    let (table, first_seed) = pending_loot_table(world, first_pos).expect("first chest loot table");
    let (_, second_seed) =
        pending_loot_table(world, second_pos).expect("second chest keeps a pending loot table");
    assert_eq!(
        first_seed, second_seed,
        "the placement random must be the only seed source"
    );

    for chest in [first_pos, second_pos] {
        let state = world.get_block_state(chest);
        open_menu(
            state,
            world,
            chest,
            &player,
            &vanilla_custom_stats::OPEN_CHEST,
        );
    }

    let expected = expected_loot(&table, first_seed, first_pos);
    assert!(!expected.is_empty(), "the village table should yield loot");
    assert_eq!(
        item_totals(&chest_items(world, first_pos)),
        expected,
        "the seeded table evaluated on its own must match the chest's loot"
    );
    assert_eq!(
        item_totals(&chest_items(world, second_pos)),
        expected,
        "two chests of the same seed must yield the same loot"
    );
}

/// A comparator read is a container read: vanilla's
/// `AbstractContainerMenu.getRedstoneSignalFromContainer` walks the slots
/// through `RandomizableContainerBlockEntity.getItem`, which unpacks a pending
/// table (with no player) before the signal is known.
#[test]
fn structure_chest_comparator_read_rolls_its_loot_table() {
    let registry = test_registry();
    let pos = BlockPos::new(4, 64, 4);
    let fixture = chest_test_world("chest_loot_comparator", pos, IVec3::new(1, 1, 1));
    let world = &fixture.world;
    let template = loot_chest_template(&registry, "minecraft:chests/village/village_plains_house");

    place_test_template(world, &registry, &template, pos);
    assert!(
        pending_loot_table(world, pos).is_some(),
        "the placed chest starts with a pending loot table"
    );

    let state = world.get_block_state(pos);
    let signal = BLOCK_BEHAVIORS
        .get_behavior(&vanilla_blocks::CHEST)
        .get_analog_output_signal(state, world.as_ref(), pos, Direction::North);

    assert!(
        signal > 0,
        "a chest rolled by the comparator read must report a signal"
    );
    assert!(
        !item_totals(&chest_items(world, pos)).is_empty(),
        "the comparator read must have generated the chest's loot"
    );
}

/// A real village house template placed into a world must leave a chest that
/// yields loot when a player opens it: the reported empty-village symptom.
#[test]
fn structure_chest_village_house_template_leaves_loot() {
    let registry = test_registry();
    let template = StructureTemplate::load_vanilla(
        &registry,
        &Identifier::vanilla_static("village/plains/houses/plains_small_house_7"),
    )
    .expect("the plains small house should be bundled");
    let size = template.unrotated_size();
    let pos = BlockPos::new(4, 64, 4);
    let fixture = chest_test_world("village_house_chest", pos, size);
    let world = &fixture.world;
    let player = TestPlayerBuilder::new(Arc::clone(world), "VillageLooter", 1).build();

    place_test_template(world, &registry, &template, pos);

    let chests: Vec<BlockPos> = (0..size.x)
        .flat_map(|x| (0..size.y).flat_map(move |y| (0..size.z).map(move |z| (x, y, z))))
        .map(|(x, y, z)| pos.offset(x, y, z))
        .filter(|&pos| {
            world.get_block_state(pos).get_block() == &vanilla_blocks::CHEST
                && world.get_block_entity(pos).is_some()
        })
        .collect();
    assert!(
        !chests.is_empty(),
        "the village house template must place at least one chest"
    );

    let loot_chests: Vec<BlockPos> = chests
        .iter()
        .copied()
        .filter(|&pos| pending_loot_table(world, pos).is_some())
        .collect();
    assert!(
        !loot_chests.is_empty(),
        "the village house chest must carry its structure template loot table"
    );

    for chest in loot_chests {
        let (_, seed) = pending_loot_table(world, chest).expect("checked pending table");
        assert_ne!(seed, 0, "placed village chests are reseeded");
        let state = world.get_block_state(chest);
        open_menu(
            state,
            world,
            chest,
            &player,
            &vanilla_custom_stats::OPEN_CHEST,
        );
        let loot = item_totals(&chest_items(world, chest));
        assert!(
            !loot.is_empty(),
            "the village chest at {chest:?} must yield loot on open"
        );
    }
}
