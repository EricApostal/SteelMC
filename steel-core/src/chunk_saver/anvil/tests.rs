//! Round-trip tests for the Anvil container and codecs.
//!
//! `level.dat` round-tripping lives in [`crate::level_data::anvil`] because it needs access to
//! private level-data fields.

use std::env;
use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use simdnbt::borrow::BaseNbtCompound;
use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::init_vanilla_registry;
use steel_registry::vanilla_blocks;
use steel_utils::{BlockStateId, ChunkPos, Identifier};
use tokio::fs;

use crate::chunk::heightmap::HeightmapType;
use crate::chunk::light::DATA_LAYER_SIZE;
use crate::chunk::status::ChunkStatus;
use crate::chunk_saver::anvil::chunk::{block_state_at, chunk_to_nbt, nbt_to_chunk};
use crate::chunk_saver::anvil::entities::{entity_from_nbt, entity_to_nbt};
use crate::chunk_saver::anvil::nbt::{read_compressed_nbt, write_gzip_nbt, write_raw_nbt};
use crate::chunk_saver::anvil::poi::{VanillaPoi, poi_from_nbt, poi_to_nbt};
use crate::chunk_saver::anvil::region::AnvilRegionFile;
use crate::chunk_saver::anvil::saved_data::{
    game_rules_from_nbt, game_rules_to_nbt, raids_from_nbt, raids_to_nbt, weather_from_nbt,
    weather_to_nbt,
};
use crate::chunk_saver::bit_pack::{pack_indices_from_iter, unpack_indices};
use crate::chunk_saver::format::{
    PersistentBiomeData, PersistentBlockState, PersistentLightData, PersistentLightSection,
    PersistentSection, PersistentTick,
};
use crate::chunk_saver::{PersistentChunk, PersistentEntity, PersistentHeightmap, PersistentPoi};
use crate::entity::raid::{RaidRecord, RaidsData};
use crate::level_data::WeatherState;

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("steel-anvil-{name}-{}-{sequence}", process::id()))
}

/// A three-section chunk with blocks, biomes, light, a tick, a heightmap, an entity, and a POI.
fn sample_chunk() -> PersistentChunk<'static> {
    let stone = &vanilla_blocks::STONE;
    let dirt = &vanilla_blocks::DIRT;

    let sections = vec![
        PersistentSection::Homogeneous {
            block_state: 0,
            biomes: PersistentBiomeData::Homogeneous { biome: 0 },
        },
        PersistentSection::Heterogeneous {
            palette: vec![0, 1],
            bits_per_entry: 1,
            block_data: pack_indices_from_iter((0..4096).map(|index| index % 2), 1),
            biomes: PersistentBiomeData::Heterogeneous {
                palette: vec![0, 1],
                bits_per_entry: 1,
                biome_data: pack_indices_from_iter((0..64).map(|index| index % 2), 1),
            },
        },
        PersistentSection::Homogeneous {
            block_state: 1,
            biomes: PersistentBiomeData::Homogeneous { biome: 1 },
        },
    ];

    PersistentChunk {
        last_modified: 1_700_000_000,
        block_states: vec![
            PersistentBlockState {
                name: stone.key.clone(),
                properties: Vec::new(),
            },
            PersistentBlockState {
                name: dirt.key.clone(),
                properties: Vec::new(),
            },
        ],
        biomes: vec![
            Identifier::vanilla_static("plains"),
            Identifier::vanilla_static("desert"),
        ],
        sections,
        block_entities: Vec::new(),
        entities: vec![sample_entity()],
        block_ticks: vec![PersistentTick {
            x: 3,
            y: (MIN_Y + 1) as i16,
            z: 7,
            delay: 42,
            priority: 2,
            tick_type: Identifier::vanilla_static("stone_button"),
        }],
        fluid_ticks: Vec::new(),
        heightmaps: vec![PersistentHeightmap {
            heightmap_type: HeightmapType::WorldSurface.persistence_id(),
            data: (0..256).map(|index| (index % 32) as u16).collect(),
        }],
        light: PersistentLightData {
            block: vec![PersistentLightSection::Initialized {
                section_index: 1,
                data: vec![0x21; DATA_LAYER_SIZE],
            }],
            sky: vec![PersistentLightSection::Initialized {
                section_index: 2,
                data: vec![0xFF; DATA_LAYER_SIZE],
            }],
        },
        postprocessing: vec![Vec::new(), vec![0x0123]],
        structure_starts: Vec::new(),
        structure_references: Vec::new(),
        inhabited_time: 1234,
        pois: vec![PersistentPoi {
            x: 1,
            y: (MIN_Y + 2) as i16,
            z: 2,
            free_tickets: 3,
        }],
    }
}

fn sample_entity() -> PersistentEntity {
    PersistentEntity {
        entity_type: Identifier::vanilla_static("item"),
        uuid: *uuid::Uuid::from_u128(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF).as_bytes(),
        pos: [0.5, 64.0, 0.5],
        motion: [0.0, -0.1, 0.0],
        rotation: [0.0, 0.0],
        fall_distance: 0.0,
        remaining_fire_ticks: 0,
        ticks_frozen: 0,
        is_in_powder_snow: false,
        was_in_powder_snow: false,
        has_visual_fire: false,
        on_ground: false,
        no_gravity: false,
        invulnerable: false,
        air_supply: 300,
        portal_cooldown: 0,
        custom_name_nbt: Vec::new(),
        custom_name_visible: false,
        silent: false,
        glowing: false,
        tags: vec!["steel".to_owned()],
        custom_data_nbt: Vec::new(),
        nbt_data: Vec::new(),
        passengers: Vec::new(),
    }
}

fn reborrow(bytes: &[u8]) -> BaseNbtCompound<'_> {
    read_borrowed_compound(&mut io::Cursor::new(bytes)).expect("test NBT should parse")
}

/// Every block state of one section, resolved through the chunk's own palette.
fn section_blocks(chunk: &PersistentChunk<'_>, index: usize) -> Vec<BlockStateId> {
    let base_y = MIN_Y + 16 * index as i32;
    let mut blocks = Vec::with_capacity(4096);
    for y in 0i32..16 {
        for z in 0u8..16 {
            for x in 0u8..16 {
                blocks.push(
                    block_state_at(chunk, x, (base_y + y) as i16, z, MIN_Y)
                        .expect("section blocks should resolve"),
                );
            }
        }
    }
    blocks
}

/// Every biome of one section (4×4×4 cells), resolved through the chunk's own palette.
fn section_biomes(chunk: &PersistentChunk<'_>, index: usize) -> Vec<Identifier> {
    let biomes = match &chunk.sections[index] {
        PersistentSection::Homogeneous { biomes, .. }
        | PersistentSection::Heterogeneous { biomes, .. } => biomes,
    };
    match biomes {
        PersistentBiomeData::Homogeneous { biome } => {
            vec![chunk.biomes[usize::from(*biome)].clone(); 64]
        }
        PersistentBiomeData::Heterogeneous {
            palette,
            bits_per_entry,
            biome_data,
        } => unpack_indices(biome_data, *bits_per_entry)
            .take(64)
            .map(|cell| chunk.biomes[usize::from(palette[cell as usize])].clone())
            .collect(),
    }
}

#[test]
fn chunk_nbt_round_trips_blocks_biomes_light_ticks_heightmaps_and_entities() {
    init_vanilla_registry();
    let chunk = sample_chunk();
    let pos = ChunkPos::new(-3, 5);
    let nbt = chunk_to_nbt(&chunk, pos, ChunkStatus::Full, MIN_Y, HEIGHT, false);
    let bytes = write_raw_nbt(&nbt);

    let borrowed = reborrow(&bytes);
    let view = simdnbt::borrow::NbtCompound::from(&borrowed);
    let (decoded, status) =
        nbt_to_chunk(&view, pos, MIN_Y, HEIGHT, "test chunk").expect("chunk should decode");

    assert_eq!(status, ChunkStatus::Full);
    assert_eq!(decoded.last_modified, chunk.last_modified);
    assert_eq!(decoded.inhabited_time, chunk.inhabited_time);
    // Vanilla `SerializableChunkData.read` materialises the dimension's whole section array, so
    // the stored sections come back inside 24; the ones the file omits decode as air + plains.
    assert_eq!(decoded.sections.len(), (HEIGHT / 16) as usize);
    for index in 0..chunk.sections.len() {
        assert_eq!(
            section_blocks(&decoded, index),
            section_blocks(&chunk, index),
            "section {index} keeps its blocks at the same world Y"
        );
        assert_eq!(
            section_biomes(&decoded, index),
            section_biomes(&chunk, index),
            "section {index} keeps its biomes at the same world Y"
        );
    }
    for index in chunk.sections.len()..decoded.sections.len() {
        assert_eq!(
            section_blocks(&decoded, index),
            vec![vanilla_blocks::AIR.default_state(); 4096],
            "unstored section {index} decodes as air"
        );
    }
    // Palettes are rebuilt in decode order (air and plains are interned first), so the chunk
    // palettes are compared by membership, not by index.
    assert!(
        chunk
            .block_states
            .iter()
            .all(|state| decoded.block_states.contains(state)),
        "every stored block state survives decoding"
    );
    assert!(
        chunk
            .biomes
            .iter()
            .all(|biome| decoded.biomes.contains(biome)),
        "every stored biome survives decoding"
    );
    assert_eq!(decoded.block_ticks.len(), 1);
    assert_eq!(decoded.block_ticks[0].delay, 42);
    assert_eq!(decoded.block_ticks[0].priority, 2);
    assert_eq!(decoded.heightmaps.len(), 1);
    assert_eq!(decoded.heightmaps[0].data, chunk.heightmaps[0].data);
    // A full chunk keeps entities out of the chunk record: vanilla writes them to
    // `entities/*.mca` (`ChunkType.LEVELCHUNK`), which is why the writer took
    // `include_entities = false`.
    assert!(decoded.entities.is_empty());

    // The protochunk form (what the storage backend writes for unfinished chunks) does carry
    // them inline, and they decode field for field.
    let inline = chunk_to_nbt(&chunk, pos, ChunkStatus::Features, MIN_Y, HEIGHT, true);
    let inline_bytes = write_raw_nbt(&inline);
    let inline_borrowed = reborrow(&inline_bytes);
    let inline_view = simdnbt::borrow::NbtCompound::from(&inline_borrowed);
    let (with_entities, _) = nbt_to_chunk(&inline_view, pos, MIN_Y, HEIGHT, "test inline chunk")
        .expect("chunk should decode");
    assert_eq!(with_entities.entities.len(), 1);
    assert_eq!(with_entities.entities[0].uuid, chunk.entities[0].uuid);
    assert_eq!(with_entities.entities[0].tags, vec!["steel".to_owned()]);
    // Light keeps both its vanilla section Y and its bytes: light index 1 is the world section
    // at `min_y`, i.e. tag `Y = min_y / 16 - 1 + 1`.
    assert_eq!(decoded.light.block.len(), 1);
    assert_eq!(decoded.light.sky.len(), 1);
    assert_eq!(
        decoded.light.block[0].section_index(),
        chunk.light.block[0].section_index()
    );
    assert_eq!(
        decoded.light.sky[0].section_index(),
        chunk.light.sky[0].section_index()
    );
    let PersistentLightSection::Initialized { data, .. } = &decoded.light.block[0] else {
        panic!("block light should decode as initialized");
    };
    assert_eq!(data.as_slice(), &[0x21u8; DATA_LAYER_SIZE][..]);
    let PersistentLightSection::Initialized { data, .. } = &decoded.light.sky[0] else {
        panic!("sky light should decode as initialized");
    };
    assert_eq!(data.as_slice(), &[0xFFu8; DATA_LAYER_SIZE][..]);
    assert_eq!(decoded.postprocessing, chunk.postprocessing);
    assert_eq!(decoded.structure_starts.as_slice(), []);
}

#[test]
fn chunk_nbt_keeps_inline_entities_only_for_protochunks() {
    init_vanilla_registry();
    let chunk = sample_chunk();
    let pos = ChunkPos::new(0, 0);
    let nbt = chunk_to_nbt(&chunk, pos, ChunkStatus::Features, MIN_Y, HEIGHT, true);
    assert!(matches!(nbt.get("entities"), Some(NbtTag::List(_))));

    let full = chunk_to_nbt(&chunk, pos, ChunkStatus::Full, MIN_Y, HEIGHT, false);
    assert!(full.get("entities").is_none());
}

#[test]
fn entity_nbt_round_trips_base_fields_and_type_data() {
    init_vanilla_registry();
    let mut entity = sample_entity();
    let mut type_data = NbtCompound::new();
    type_data.insert("Age", 12i16);
    entity.nbt_data = write_raw_nbt(&type_data);

    let nbt = entity_to_nbt(&entity);
    assert_eq!(
        nbt.string("id").map(|id| id.to_str().into_owned()),
        Some("minecraft:item".to_owned())
    );
    let passengers = [entity_to_nbt(&entity)];

    let bytes = write_raw_nbt(&nbt);
    let borrowed = reborrow(&bytes);
    let view = simdnbt::borrow::NbtCompound::from(&borrowed);
    let decoded = entity_from_nbt(&view).expect("entity should decode");

    assert_eq!(decoded.entity_type, entity.entity_type);
    assert_eq!(decoded.uuid, entity.uuid);
    assert_eq!(decoded.pos, entity.pos);
    assert_eq!(decoded.motion, entity.motion);
    assert_eq!(decoded.tags, entity.tags);
    assert_eq!(decoded.nbt_data, entity.nbt_data);
    assert_eq!(passengers.len(), 1);
}

#[test]
fn poi_nbt_round_trips_sections() {
    init_vanilla_registry();
    let entries = vec![
        VanillaPoi {
            pos: [1, MIN_Y + 2, 2],
            poi_type: Some(Identifier::vanilla_static("home")),
            free_tickets: 3,
        },
        VanillaPoi {
            pos: [4, MIN_Y + 20, 6],
            poi_type: Some(Identifier::vanilla_static("home")),
            free_tickets: 0,
        },
    ];
    let nbt = poi_to_nbt(&entries);
    let bytes = write_raw_nbt(&nbt);
    let borrowed = reborrow(&bytes);
    let view = simdnbt::borrow::NbtCompound::from(&borrowed);
    let decoded = poi_from_nbt(&view).expect("POI should decode");
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0].free_tickets, 3);
    assert_eq!(decoded[1].pos[1], MIN_Y + 20);
}

#[tokio::test]
async fn region_container_round_trips_records_and_timestamps() {
    let directory = test_directory("region");
    let mut file = AnvilRegionFile::open(&directory, 0, 0, true)
        .await
        .expect("region should open")
        .expect("region should be created");

    let payload = vec![7u8; 5000];
    file.write(0, &payload).await.expect("record should write");
    file.write(1023, b"small")
        .await
        .expect("record should write");
    file.flush().await.expect("header should flush");

    assert!(file.contains(0));
    assert!(file.contains(1023));
    assert!(!file.contains(5));
    let read = file.read(0).await.expect("record should read");
    assert_eq!(read.as_deref(), Some(payload.as_slice()));
    let read = file.read(1023).await.expect("record should read");
    assert_eq!(read.as_deref(), Some(b"small".as_slice()));
    file.close().await.expect("region should close");

    // The on-disk header must be a plain vanilla offsets+timestamps table.
    let bytes = fs::read(directory.join("r.0.0.mca"))
        .await
        .expect("region file should exist");
    let slot_zero = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    assert_eq!(slot_zero >> 8, 2, "slot 0 points at the first data sector");
    assert!(slot_zero & 0xFF >= 1, "slot 0 has a non-zero sector count");
    assert_ne!(&bytes[4096..4100], &[0, 0, 0, 0], "slot 0 has a timestamp");
    let slot_last = u32::from_be_bytes([bytes[4092], bytes[4093], bytes[4094], bytes[4095]]);
    assert_eq!(
        slot_last >> 8,
        3,
        "slot 1023 is appended after slot 0's single sector"
    );

    // Rewriting an existing record reuses its previous allocation instead of leaking sectors.
    let before = fs::metadata(directory.join("r.0.0.mca"))
        .await
        .expect("metadata")
        .len();
    let mut file = AnvilRegionFile::open(&directory, 0, 0, true)
        .await
        .expect("region should reopen")
        .expect("region exists");
    file.write(0, &payload)
        .await
        .expect("rewrite should succeed");
    file.close().await.expect("region should close");
    let after = fs::metadata(directory.join("r.0.0.mca"))
        .await
        .expect("metadata")
        .len();
    assert_eq!(
        before, after,
        "a same-size rewrite must not grow the region"
    );

    fs::remove_dir_all(&directory).await.ok();
}

#[tokio::test]
async fn region_container_treats_a_truncated_header_as_empty() {
    let directory = test_directory("truncated");
    fs::create_dir_all(&directory).await.expect("directory");
    fs::write(directory.join("r.0.0.mca"), [0u8; 64])
        .await
        .expect("write truncated file");
    let file = AnvilRegionFile::open(&directory, 0, 0, false)
        .await
        .expect("truncated header must not fail the open")
        .expect("file exists");
    assert!(!file.contains(0));
    fs::remove_dir_all(&directory).await.ok();
}

#[test]
fn raids_weather_and_game_rule_payloads_round_trip() {
    init_vanilla_registry();
    let raids = RaidsData {
        raids: vec![RaidRecord {
            id: 7,
            started: true,
            active: true,
            ticks_active: 120,
            raid_omen_level: 2,
            groups_spawned: 1,
            raid_cooldown_ticks: 0,
            post_raid_ticks: 0,
            total_health: 42.5,
            group_count: 3,
            status: "ongoing".to_owned(),
            center: [10, 64, -10],
            heroes_of_the_village: vec![uuid::Uuid::from_u128(1)],
        }],
        next_id: 8,
        tick: 999,
    };
    let nbt = raids_to_nbt(&raids);
    assert_eq!(raids_from_nbt(&nbt), raids);

    let weather = WeatherState {
        raining: true,
        rain_time: 5,
        thundering: false,
        thunder_time: 6,
        clear_weather_time: 7,
    };
    let nbt = weather_to_nbt(&weather);
    let decoded = weather_from_nbt(&nbt);
    assert_eq!(decoded.raining, weather.raining);
    assert_eq!(decoded.rain_time, weather.rain_time);
    assert_eq!(decoded.clear_weather_time, weather.clear_weather_time);

    let mut rules = rustc_hash::FxHashMap::default();
    rules.insert("keep_inventory".to_owned(), serde_json::Value::Bool(true));
    rules.insert("random_tick_speed".to_owned(), serde_json::Value::from(3));
    let nbt = game_rules_to_nbt(&rules);
    let decoded = game_rules_from_nbt(&nbt);
    assert_eq!(
        decoded.get("keep_inventory"),
        Some(&serde_json::Value::Bool(true))
    );
    assert_eq!(
        decoded.get("random_tick_speed"),
        Some(&serde_json::Value::from(3))
    );
}

#[test]
fn compressed_nbt_reads_gzip_and_raw_payloads() {
    let mut compound = NbtCompound::new();
    compound.insert("seed", 12345i64);
    compound.insert("list", NbtList::Int(vec![1, 2, 3]));

    let gzip = write_gzip_nbt(&compound).expect("gzip should succeed");
    assert_eq!(
        read_compressed_nbt(&gzip).expect("gzip should parse"),
        compound
    );

    let raw = write_raw_nbt(&compound);
    assert_eq!(
        read_compressed_nbt(&raw).expect("raw should parse"),
        compound
    );
}

/// The version-gate regression guard: every vanilla-format reader must accept this build's
/// `DataVersion` and refuse any other instead of loading data it cannot fix.
mod version_gate {
    use std::path::Path;

    use steel_utils::version::DATA_VERSION;

    use crate::chunk_saver::anvil::saved_data::read_saved_data_file;
    use crate::chunk_saver::anvil::storage::decode_persistent_chunk;
    use crate::level_data::LevelData;
    use crate::level_data::anvil::apply_level_dat;

    use super::*;

    const SOURCE: &str = "region/r.0.0.mca at chunk (0, 0)";

    /// Re-stamps a compound a Steel writer produced.
    ///
    /// `NbtCompound::insert` appends and a lookup returns the *first* matching tag, so a second
    /// `insert` would leave the writer's own `DataVersion` in place and the gate would pass for
    /// the wrong reason. (`take` does not remove either — it swaps the value for a `Byte(0)`.)
    /// The existing `Int` tag has to be overwritten through `int_mut`.
    fn restamp(nbt: &mut NbtCompound, version: i32) {
        match nbt.int_mut("DataVersion") {
            Some(current) => *current = version,
            None => panic!("fixture must carry an Int DataVersion to re-stamp"),
        }
    }

    /// Chunk NBT stamped with `chunk_version`, plus the entity and POI side records stamped with
    /// `side_version`.
    ///
    /// The two are separate so a test can hold the chunk at this build's version and vary only
    /// the record whose gate it means to exercise — otherwise the chunk gate fires first and
    /// the test would pass without ever reaching the side-record gate.
    fn fixtures(chunk_version: i32, side_version: i32) -> (Vec<u8>, Vec<u8>, Option<Vec<u8>>) {
        let chunk = sample_chunk();
        let pos = ChunkPos::new(0, 0);

        let mut chunk_nbt = chunk_to_nbt(&chunk, pos, ChunkStatus::Full, MIN_Y, HEIGHT, false);
        restamp(&mut chunk_nbt, chunk_version);

        let mut entity_nbt = NbtCompound::new();
        entity_nbt.insert("DataVersion", side_version);
        entity_nbt.insert("Position", NbtTag::IntArray(vec![pos.0.x, pos.0.y]));
        entity_nbt.insert(
            "Entities",
            NbtList::Compound(vec![entity_to_nbt(&sample_entity())]),
        );

        let mut poi_root = poi_to_nbt(&[VanillaPoi {
            pos: [1, MIN_Y + 2, 2],
            poi_type: Some(Identifier::vanilla_static("home")),
            free_tickets: 3,
        }]);
        restamp(&mut poi_root, side_version);

        (
            write_raw_nbt(&chunk_nbt),
            write_raw_nbt(&entity_nbt),
            Some(write_raw_nbt(&poi_root)),
        )
    }

    fn decode(
        bytes: (&[u8], &[u8], Option<&[u8]>),
    ) -> io::Result<(PersistentChunk<'static>, ChunkStatus)> {
        let (chunk, entities, poi) = bytes;
        decode_persistent_chunk(
            chunk,
            Some(entities),
            poi,
            ChunkPos::new(0, 0),
            MIN_Y,
            HEIGHT,
            SOURCE,
        )
    }

    #[test]
    fn chunk_records_load_at_this_data_version_and_are_refused_otherwise() {
        init_vanilla_registry();

        let (chunk, entities, poi) = fixtures(DATA_VERSION, DATA_VERSION);
        decode((&chunk, &entities, poi.as_deref()))
            .expect("a chunk stamped with this build's data version must load");

        for stale in [DATA_VERSION - 1, DATA_VERSION + 1] {
            let (chunk, entities, poi) = fixtures(stale, stale);
            let Err(error) = decode((&chunk, &entities, poi.as_deref())) else {
                panic!("a chunk from data version {stale} must be refused");
            };
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            let message = error.to_string();
            assert!(message.contains("the chunk of"), "{message}");
            assert!(message.contains(&stale.to_string()), "{message}");
            assert!(message.contains(SOURCE), "{message}");
        }
    }

    #[test]
    fn entity_side_records_are_refused_at_another_data_version() {
        init_vanilla_registry();

        let (chunk, _, _) = fixtures(DATA_VERSION, DATA_VERSION);
        let (_, entities, _) = fixtures(DATA_VERSION, DATA_VERSION - 1);
        let Err(error) = decode_persistent_chunk(
            &chunk,
            Some(&entities),
            None,
            ChunkPos::new(0, 0),
            MIN_Y,
            HEIGHT,
            SOURCE,
        ) else {
            panic!("an entity record from an older data version must be refused");
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(message.contains("the entity of"), "{message}");
        assert!(message.contains(SOURCE), "{message}");
    }

    #[test]
    fn poi_side_records_are_refused_at_another_data_version() {
        init_vanilla_registry();

        let (chunk, _, poi) = fixtures(DATA_VERSION, DATA_VERSION + 1);
        let Err(error) = decode_persistent_chunk(
            &chunk,
            None,
            poi.as_deref(),
            ChunkPos::new(0, 0),
            MIN_Y,
            HEIGHT,
            SOURCE,
        ) else {
            panic!("a POI record from a newer data version must be refused");
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(message.contains("the POI of"), "{message}");
        assert!(message.contains(SOURCE), "{message}");

        let (chunk, _, poi) = fixtures(DATA_VERSION, DATA_VERSION);
        // The same chunk that was refused above now decodes, which is what isolates the gate to
        // the POI record. The resolved `pois` set is not asserted: a record only survives when
        // the chunk's block state at its position maps to a POI type (`storage.rs`), and the
        // fixture chunk holds stone there.
        let (_, _) = decode_persistent_chunk(
            &chunk,
            None,
            poi.as_deref(),
            ChunkPos::new(0, 0),
            MIN_Y,
            HEIGHT,
            SOURCE,
        )
        .expect("a POI record stamped with this build's data version must load");
    }

    #[test]
    fn level_dat_loads_at_this_data_version_and_is_refused_otherwise() {
        init_vanilla_registry();
        let directory = Path::new("/nonexistent-steel-level-dat-version-test");

        let mut data = NbtCompound::new();
        data.insert("DataVersion", DATA_VERSION);
        let mut level = LevelData::new_with_seed(1);
        apply_level_dat(&data, directory, &mut level)
            .expect("a level.dat stamped with this build's data version must load");

        // `insert` appends and the writer lookup returns the first tag, so a second `insert`
        // would not change what the gate sees: build the stale record fresh.
        let mut stale = NbtCompound::new();
        stale.insert("DataVersion", DATA_VERSION - 1);
        let Err(error) = apply_level_dat(&stale, directory, &mut level) else {
            panic!("a level.dat from another data version must be refused");
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        let message = error.to_string();
        assert!(
            message.contains(&(DATA_VERSION - 1).to_string()),
            "{message}"
        );
        assert!(message.contains("level.dat"), "{message}");
    }

    #[tokio::test]
    async fn saved_data_files_load_at_this_data_version_and_are_refused_otherwise() {
        let directory = test_directory("saved-data-version");
        fs::create_dir_all(&directory)
            .await
            .expect("test directory should be created");
        let path = directory.join("raids.dat");

        for version in [DATA_VERSION, DATA_VERSION + 1] {
            let mut root = NbtCompound::new();
            root.insert("data", NbtTag::Compound(NbtCompound::new()));
            root.insert("DataVersion", version);
            fs::write(&path, write_gzip_nbt(&root).expect("root should encode"))
                .await
                .expect("saved data should be written");

            if version == DATA_VERSION {
                read_saved_data_file(&path)
                    .expect("this build's saved data must load")
                    .expect("the wrapper must carry a payload");
                continue;
            }
            let Err(error) = read_saved_data_file(&path) else {
                panic!("saved data from data version {version} must be refused");
            };
            assert_eq!(error.kind(), io::ErrorKind::Unsupported);
            let message = error.to_string();
            assert!(message.contains(&version.to_string()), "{message}");
            assert!(message.contains("raids.dat"), "{message}");
        }

        fs::remove_dir_all(directory)
            .await
            .expect("test directory should be removable");
    }
}

mod player_data {
    use super::*;
    use crate::chunk_saver::anvil::nbt::read_raw_nbt;
    use crate::chunk_saver::anvil::player_data::{
        ImportedPlayer, export_player_data, import_player_data, player_from_vanilla_nbt,
        player_to_vanilla_nbt,
    };
    use crate::datafixer::DATA_VERSION;
    use crate::player::player_data::PLAYER_DATA_VERSION;
    use crate::player::player_data_storage::decode_persistent_player;
    use steel_registry::vanilla_items;
    use steel_utils::BlockPos;
    use steel_utils::uuid_ext::UuidExt as _;
    use uuid::Uuid;

    fn sample_uuid() -> Uuid {
        Uuid::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef)
    }

    fn vehicle_uuid() -> Uuid {
        Uuid::from_u128(0x0fed_cba9_8765_4321_0fed_cba9_8765_4321)
    }

    fn item(id: &str, count: i32) -> NbtCompound {
        let mut item = NbtCompound::new();
        item.insert("id", id.to_owned());
        item.insert("count", count);
        item
    }

    fn slot(slot: i8, id: &str, count: i32) -> NbtCompound {
        let mut stack = item(id, count);
        stack.insert("Slot", NbtTag::Byte(slot));
        stack
    }

    fn entity(id: &str, uuid: Uuid) -> NbtCompound {
        let mut entity = NbtCompound::new();
        entity.insert("id", id.to_owned());
        entity.insert("UUID", NbtTag::IntArray(uuid.to_int_array().to_vec()));
        entity.insert("Pos", NbtList::Double(vec![2.5, 65.0, -3.5]));
        entity.insert("Motion", NbtList::Double(vec![0.0, 0.1, 0.0]));
        entity.insert("Rotation", NbtList::Float(vec![45.0, 10.0]));
        entity
    }

    /// A hand-built record with the exact layout `PlayerDataStorage.save` produces for a
    /// player with items, effects, a bed respawn, a vehicle and an in-flight ender pearl.
    #[expect(
        clippy::too_many_lines,
        reason = "one hand-built fixture mirroring the exact vanilla player compound keys keeps the layout auditable in one place"
    )]
    fn sample_player_nbt(data_version: i32) -> NbtCompound {
        let mut nbt = NbtCompound::new();
        nbt.insert("DataVersion", data_version);
        nbt.insert(
            "UUID",
            NbtTag::IntArray(sample_uuid().to_int_array().to_vec()),
        );
        nbt.insert("Pos", NbtList::Double(vec![1.5, 64.0, -2.5]));
        nbt.insert("Motion", NbtList::Double(vec![0.1, 0.0, -0.1]));
        nbt.insert("Rotation", NbtList::Float(vec![90.0, -30.0]));
        nbt.insert("OnGround", NbtTag::Byte(1));
        nbt.insert("Fire", NbtTag::Short(20));
        nbt.insert("TicksFrozen", 40);
        nbt.insert("HasVisualFire", NbtTag::Byte(1));
        nbt.insert("Health", 18.5f32);
        nbt.insert("DeathTime", NbtTag::Short(0));
        nbt.insert("AbsorptionAmount", 2.0f32);
        nbt.insert("FallFlying", NbtTag::Byte(0));

        let mut attribute = NbtCompound::new();
        attribute.insert("id", "minecraft:generic.max_health".to_owned());
        attribute.insert("base", 20.0f64);
        let mut modifier = NbtCompound::new();
        modifier.insert("id", "minecraft:test_bonus".to_owned());
        modifier.insert("amount", 2.0f64);
        modifier.insert("operation", "add_value".to_owned());
        attribute.insert("modifiers", NbtList::Compound(vec![modifier]));
        nbt.insert("attributes", NbtList::Compound(vec![attribute]));

        let mut speed = NbtCompound::new();
        speed.insert("id", "minecraft:speed".to_owned());
        speed.insert("duration", 200);
        speed.insert("amplifier", NbtTag::Byte(1));
        speed.insert("ambient", NbtTag::Byte(1));
        speed.insert("show_particles", NbtTag::Byte(0));
        speed.insert("show_icon", NbtTag::Byte(1));
        nbt.insert("active_effects", NbtList::Compound(vec![speed]));

        nbt.insert(
            "Inventory",
            NbtList::Compound(vec![
                slot(0, "minecraft:stone", 64),
                slot(10, "minecraft:diamond_sword", 1),
            ]),
        );
        nbt.insert("SelectedItemSlot", 3);
        nbt.insert(
            "EnderItems",
            NbtList::Compound(vec![slot(4, "minecraft:ender_pearl", 16)]),
        );

        let mut equipment = NbtCompound::new();
        equipment.insert(
            "head",
            NbtTag::Compound(item("minecraft:diamond_helmet", 1)),
        );
        equipment.insert("offhand", NbtTag::Compound(item("minecraft:shield", 1)));
        nbt.insert("equipment", NbtTag::Compound(equipment));

        nbt.insert("XpP", 0.25f32);
        nbt.insert("XpLevel", 7);
        nbt.insert("XpTotal", 100);
        nbt.insert("XpSeed", 42);
        nbt.insert("Score", 9);
        nbt.insert("foodLevel", 17);
        nbt.insert("foodTickTimer", 5);
        nbt.insert("foodSaturationLevel", 3.5f32);
        nbt.insert("foodExhaustionLevel", 0.75f32);

        let mut abilities = NbtCompound::new();
        abilities.insert("invulnerable", NbtTag::Byte(1));
        abilities.insert("flying", NbtTag::Byte(0));
        abilities.insert("mayfly", NbtTag::Byte(1));
        abilities.insert("instabuild", NbtTag::Byte(1));
        abilities.insert("mayBuild", NbtTag::Byte(0));
        abilities.insert("flySpeed", 0.075f32);
        abilities.insert("walkSpeed", 0.15f32);
        nbt.insert("abilities", NbtTag::Compound(abilities));

        nbt.insert("playerGameType", 1);
        nbt.insert("previousPlayerGameType", 0);
        nbt.insert("seenCredits", NbtTag::Byte(1));
        nbt.insert("Dimension", "minecraft:overworld".to_owned());

        let mut respawn = NbtCompound::new();
        respawn.insert("dimension", "minecraft:overworld".to_owned());
        respawn.insert("pos", NbtList::Int(vec![10, 64, 10]));
        respawn.insert("yaw", 90.0f32);
        respawn.insert("pitch", 0.0f32);
        respawn.insert("forced", NbtTag::Byte(1));
        nbt.insert("respawn", NbtTag::Compound(respawn));

        let mut root_vehicle = NbtCompound::new();
        root_vehicle.insert(
            "Attach",
            NbtTag::IntArray(vehicle_uuid().to_int_array().to_vec()),
        );
        root_vehicle.insert(
            "Entity",
            NbtTag::Compound(entity("minecraft:oak_boat", vehicle_uuid())),
        );
        nbt.insert("RootVehicle", NbtTag::Compound(root_vehicle));

        let mut pearl = entity("minecraft:ender_pearl", vehicle_uuid());
        pearl.insert("ender_pearl_dimension", "minecraft:the_nether".to_owned());
        nbt.insert("ender_pearls", NbtList::Compound(vec![pearl]));

        let mut recipe_book = NbtCompound::new();
        recipe_book.insert("isGuiOpen", NbtTag::Byte(1));
        recipe_book.insert("isFilteringCraftable", NbtTag::Byte(0));
        recipe_book.insert("isFurnaceGuiOpen", NbtTag::Byte(1));
        recipe_book.insert("isFurnaceFilteringCraftable", NbtTag::Byte(0));
        recipe_book.insert("isBlastingFurnaceGuiOpen", NbtTag::Byte(0));
        recipe_book.insert("isBlastingFurnaceFilteringCraftable", NbtTag::Byte(0));
        recipe_book.insert("isSmokerGuiOpen", NbtTag::Byte(0));
        recipe_book.insert("isSmokerFilteringCraftable", NbtTag::Byte(0));
        recipe_book.insert(
            "recipes",
            NbtList::String(vec![
                "minecraft:crafting_table".into(),
                "minecraft:stick".into(),
            ]),
        );
        recipe_book.insert(
            "toBeDisplayed",
            NbtList::String(vec!["minecraft:stick".into()]),
        );
        nbt.insert("recipeBook", NbtTag::Compound(recipe_book));

        // Present in real files but with no Steel destination: must be reported, not dropped
        // silently.
        nbt.insert("Air", NbtTag::Short(300));
        nbt.insert("fall_distance", 1.25f64);
        let mut death = NbtCompound::new();
        death.insert("dimension", "minecraft:overworld".to_owned());
        death.insert("pos", NbtList::Int(vec![1, 2, 3]));
        nbt.insert("LastDeathLocation", NbtTag::Compound(death));
        nbt
    }

    fn imported() -> ImportedPlayer {
        init_vanilla_registry();
        player_from_vanilla_nbt(&sample_player_nbt(DATA_VERSION), "test player")
            .expect("a vanilla-shaped player record must import")
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "asserting every imported field in one test documents the complete vanilla-to-Steel mapping"
    )]
    fn vanilla_player_record_imports_every_modelled_field() {
        let imported = imported();
        let data = &imported.data;

        assert_eq!(data.pos, [1.5, 64.0, -2.5]);
        assert_eq!(data.motion, [0.1, 0.0, -0.1]);
        assert_eq!(data.rotation, [90.0, -30.0]);
        assert!(data.on_ground);
        assert!(!data.fall_flying);
        assert_eq!(data.remaining_fire_ticks, 20);
        assert_eq!(data.ticks_frozen, 40);
        assert!(data.has_visual_fire);
        assert_eq!(data.health, 18.5);
        assert_eq!(data.game_mode, 1);
        assert_eq!(data.prev_game_mode, Some(0));
        assert_eq!(data.selected_slot, 3);
        assert_eq!(data.world, "minecraft:overworld");
        assert_eq!(data.food_level, 17);
        assert_eq!(data.food_tick_timer, 5);
        assert_eq!(data.food_saturation_level, 3.5);
        assert_eq!(data.food_exhaustion_level, 0.75);
        assert_eq!(data.data_version, PLAYER_DATA_VERSION);
        assert_eq!(data.experience_level, 7);
        assert_eq!(data.experience_progress, 0.25);
        assert_eq!(data.experience_total, 100);
        assert_eq!(data.enchantment_seed, 42);
        assert_eq!(data.score, 9);
        assert!(data.seen_credits);

        assert!(data.abilities.invulnerable);
        assert!(!data.abilities.flying);
        assert!(data.abilities.may_fly);
        assert!(data.abilities.instabuild);
        assert!(!data.abilities.may_build);
        assert_eq!(data.abilities.flying_speed, 0.075);
        assert_eq!(data.abilities.walking_speed, 0.15);

        let stone = data
            .inventory
            .iter()
            .find(|slot| slot.slot == 0)
            .expect("hotbar slot 0");
        assert_eq!(stone.item.item().key, vanilla_items::STONE.key);
        assert_eq!(stone.item.count(), 64);
        assert_eq!(
            data.inventory
                .iter()
                .find(|slot| slot.slot == 10)
                .expect("present")
                .item
                .item()
                .key,
            vanilla_items::DIAMOND_SWORD.key
        );
        assert_eq!(
            data.inventory
                .iter()
                .find(|slot| slot.slot == 39)
                .expect("helmet was moved from `equipment` to slot 39")
                .item
                .item()
                .key,
            vanilla_items::DIAMOND_HELMET.key
        );
        assert_eq!(
            data.inventory
                .iter()
                .find(|slot| slot.slot == 40)
                .expect("offhand was moved from `equipment` to slot 40")
                .item
                .item()
                .key,
            vanilla_items::SHIELD.key
        );
        assert_eq!(data.ender_items.len(), 1);
        assert_eq!(data.ender_items[0].slot, 4);
        assert_eq!(data.ender_items[0].item.count(), 16);

        let respawn = data.respawn_config.as_ref().expect("bed respawn");
        assert_eq!(
            respawn.respawn_data.dimension(),
            &Identifier::vanilla_static("overworld")
        );
        assert_eq!(respawn.respawn_data.pos(), BlockPos::new(10, 64, 10));
        assert_eq!(respawn.respawn_data.yaw, 90.0);
        assert_eq!(respawn.respawn_data.pitch, 0.0);
        assert!(respawn.forced);

        let root_vehicle = data.root_vehicle.as_ref().expect("root vehicle");
        assert_eq!(root_vehicle.attach, *vehicle_uuid().as_bytes());
        assert_eq!(
            root_vehicle.entity.entity_type,
            Identifier::vanilla_static("oak_boat")
        );

        assert_eq!(data.ender_pearls.len(), 1);
        assert_eq!(data.ender_pearls[0].world, "minecraft:the_nether");
        assert_eq!(
            data.ender_pearls[0].entity.entity_type,
            Identifier::vanilla_static("ender_pearl")
        );

        assert!(data.recipe_book.settings.crafting.open);
        assert!(!data.recipe_book.settings.crafting.filtering);
        assert!(data.recipe_book.settings.furnace.open);
        assert_eq!(
            data.recipe_book.known,
            vec![
                Identifier::vanilla_static("crafting_table"),
                Identifier::vanilla_static("stick")
            ]
        );
        assert_eq!(
            data.recipe_book.highlight,
            vec![Identifier::vanilla_static("stick")]
        );

        assert!(imported.unmapped.contains(&"Air"));
        assert!(imported.unmapped.contains(&"fall_distance"));
        assert!(imported.unmapped.contains(&"LastDeathLocation"));

        let living = read_raw_nbt(&data.living_nbt).expect("living payload");
        assert_eq!(living.float("Health"), Some(18.5));
        assert_eq!(living.short("DeathTime"), Some(0));
        assert_eq!(living.float("AbsorptionAmount"), Some(2.0));
        assert_eq!(living.byte("FallFlying"), Some(0));
        let Some(NbtTag::List(NbtList::Compound(attributes))) = living.get("attributes") else {
            panic!("attributes must be carried into the living payload");
        };
        assert_eq!(attributes.len(), 1);
        assert_eq!(
            attributes[0]
                .string("id")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:generic.max_health"
        );
        assert_eq!(attributes[0].double("base"), Some(20.0));
        let Some(NbtTag::List(NbtList::Compound(effects))) = living.get("active_effects") else {
            panic!("active effects must be carried into the living payload");
        };
        assert_eq!(
            effects[0].string("id").expect("present").to_str().as_ref(),
            "minecraft:speed"
        );
        assert_eq!(effects[0].int("duration"), Some(200));
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "asserting every exported key and type against the jar is clearest as one sequential test"
    )]
    fn exported_player_record_has_vanilla_keys_and_types() {
        init_vanilla_registry();
        let data = imported().data;
        let nbt = player_to_vanilla_nbt(&data, sample_uuid()).expect("player must export");

        assert_eq!(nbt.int("DataVersion"), Some(DATA_VERSION));
        assert_eq!(
            nbt.get("UUID").and_then(|tag| tag.int_array()),
            Some(sample_uuid().to_int_array().as_slice())
        );
        assert_eq!(
            nbt.get("Pos"),
            Some(&NbtTag::List(NbtList::Double(vec![1.5, 64.0, -2.5])))
        );
        assert_eq!(
            nbt.get("Motion"),
            Some(&NbtTag::List(NbtList::Double(vec![0.1, 0.0, -0.1])))
        );
        assert_eq!(
            nbt.get("Rotation"),
            Some(&NbtTag::List(NbtList::Float(vec![90.0, -30.0])))
        );
        assert_eq!(nbt.get("OnGround"), Some(&NbtTag::Byte(1)));
        assert_eq!(nbt.get("Fire"), Some(&NbtTag::Short(20)));
        assert_eq!(nbt.int("TicksFrozen"), Some(40));
        assert_eq!(nbt.get("HasVisualFire"), Some(&NbtTag::Byte(1)));
        assert_eq!(nbt.float("Health"), Some(18.5));
        assert_eq!(nbt.short("DeathTime"), Some(0));
        assert_eq!(nbt.float("AbsorptionAmount"), Some(2.0));
        assert_eq!(nbt.byte("FallFlying"), Some(0));
        assert_eq!(nbt.int("SelectedItemSlot"), Some(3));
        assert_eq!(nbt.float("XpP"), Some(0.25));
        assert_eq!(nbt.int("XpLevel"), Some(7));
        assert_eq!(nbt.int("XpTotal"), Some(100));
        assert_eq!(nbt.int("XpSeed"), Some(42));
        assert_eq!(nbt.int("Score"), Some(9));
        assert_eq!(nbt.int("foodLevel"), Some(17));
        assert_eq!(nbt.int("foodTickTimer"), Some(5));
        assert_eq!(nbt.float("foodSaturationLevel"), Some(3.5));
        assert_eq!(nbt.float("foodExhaustionLevel"), Some(0.75));
        assert_eq!(nbt.int("playerGameType"), Some(1));
        assert_eq!(nbt.int("previousPlayerGameType"), Some(0));
        assert_eq!(nbt.byte("seenCredits"), Some(1));
        assert_eq!(
            nbt.string("Dimension").expect("present").to_str().as_ref(),
            "minecraft:overworld"
        );

        let abilities = nbt.compound("abilities").expect("abilities");
        for key in ["invulnerable", "flying", "mayfly", "instabuild", "mayBuild"] {
            assert!(matches!(abilities.get(key), Some(NbtTag::Byte(_))), "{key}");
        }
        assert_eq!(abilities.float("flySpeed"), Some(0.075));
        assert_eq!(abilities.float("walkSpeed"), Some(0.15));

        let Some(NbtTag::List(NbtList::Compound(inventory))) = nbt.get("Inventory") else {
            panic!("Inventory must be a compound list");
        };
        assert_eq!(inventory.len(), 2);
        let stone = inventory
            .iter()
            .find(|stack| stack.byte("Slot") == Some(0))
            .expect("present");
        assert_eq!(
            stone.string("id").expect("present").to_str().as_ref(),
            "minecraft:stone"
        );
        assert_eq!(stone.int("count"), Some(64));
        assert!(
            inventory
                .iter()
                .all(|stack| stack.byte("Slot").is_some_and(|slot| slot < 36))
        );

        let equipment = nbt.compound("equipment").expect("equipment carries armour");
        assert_eq!(
            equipment
                .compound("head")
                .expect("present")
                .string("id")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:diamond_helmet"
        );
        assert_eq!(
            equipment
                .compound("offhand")
                .expect("present")
                .string("id")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:shield"
        );

        let Some(NbtTag::List(NbtList::Compound(ender))) = nbt.get("EnderItems") else {
            panic!("EnderItems must be a compound list");
        };
        assert_eq!(ender[0].byte("Slot"), Some(4));
        assert_eq!(ender[0].int("count"), Some(16));

        assert!(
            matches!(nbt.get("attributes"), Some(NbtTag::List(NbtList::Compound(attributes))) if attributes.len() == 1)
        );
        assert!(
            matches!(nbt.get("active_effects"), Some(NbtTag::List(NbtList::Compound(effects))) if effects.len() == 1)
        );

        let respawn = nbt.compound("respawn").expect("respawn");
        assert_eq!(
            respawn
                .string("dimension")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:overworld"
        );
        assert_eq!(
            respawn.get("pos"),
            Some(&NbtTag::List(NbtList::Int(vec![10, 64, 10])))
        );
        assert_eq!(respawn.float("yaw"), Some(90.0));
        assert_eq!(respawn.byte("forced"), Some(1));

        let root = nbt.compound("RootVehicle").expect("RootVehicle");
        assert_eq!(
            root.get("Entity")
                .and_then(|tag| tag.compound())
                .expect("present")
                .string("id")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:oak_boat"
        );
        assert!(matches!(root.get("Attach"), Some(NbtTag::IntArray(_))));

        let Some(NbtTag::List(NbtList::Compound(pearls))) = nbt.get("ender_pearls") else {
            panic!("ender_pearls must be a compound list");
        };
        assert_eq!(
            pearls[0]
                .string("ender_pearl_dimension")
                .expect("present")
                .to_str()
                .as_ref(),
            "minecraft:the_nether"
        );

        let book = nbt.compound("recipeBook").expect("recipeBook");
        assert_eq!(book.byte("isGuiOpen"), Some(1));
        assert_eq!(book.byte("isFilteringCraftable"), Some(0));
        assert_eq!(book.byte("isFurnaceGuiOpen"), Some(1));
        assert!(book.list("recipes").expect("present").strings().is_some());
        assert_eq!(
            book.list("recipes")
                .expect("present")
                .strings()
                .expect("present")
                .len(),
            2
        );

        // Keys Steel cannot represent are not invented on export.
        assert!(nbt.get("Air").is_none());
        assert!(nbt.get("fall_distance").is_none());
        assert!(nbt.get("LastDeathLocation").is_none());
    }

    #[test]
    fn exported_record_reimports_to_the_same_state() {
        init_vanilla_registry();
        let original = imported().data;
        let exported = player_to_vanilla_nbt(&original, sample_uuid()).expect("export");
        let reimported = player_from_vanilla_nbt(&exported, "round trip")
            .expect("exported record must import")
            .data;

        assert_eq!(reimported.pos, original.pos);
        assert_eq!(reimported.motion, original.motion);
        assert_eq!(reimported.rotation, original.rotation);
        assert_eq!(reimported.health, original.health);
        assert_eq!(reimported.food_level, original.food_level);
        assert_eq!(reimported.experience_level, original.experience_level);
        assert_eq!(reimported.enchantment_seed, original.enchantment_seed);
        assert_eq!(reimported.world, original.world);
        assert_eq!(reimported.inventory.len(), original.inventory.len());
        for slot in &original.inventory {
            let other = reimported
                .inventory
                .iter()
                .find(|other| other.slot == slot.slot)
                .expect("slot survives");
            assert_eq!(other.item.item().key, slot.item.item().key);
            assert_eq!(other.item.count(), slot.item.count());
        }
        assert_eq!(reimported.ender_items.len(), original.ender_items.len());
        assert_eq!(reimported.recipe_book, original.recipe_book);
        assert_eq!(
            reimported.respawn_config.as_ref().map(|r| (
                r.respawn_data.dimension().clone(),
                r.respawn_data.pos(),
                r.forced
            )),
            original.respawn_config.as_ref().map(|r| (
                r.respawn_data.dimension().clone(),
                r.respawn_data.pos(),
                r.forced
            ))
        );
        assert_eq!(
            reimported.root_vehicle.as_ref().map(|v| v.attach),
            original.root_vehicle.as_ref().map(|v| v.attach)
        );
        assert_eq!(
            reimported.ender_pearls[0].world,
            original.ender_pearls[0].world
        );
    }

    #[test]
    fn stale_player_data_version_is_refused() {
        init_vanilla_registry();
        let Err(error) =
            player_from_vanilla_nbt(&sample_player_nbt(DATA_VERSION - 1), "stale player")
        else {
            panic!("a player record from another data version must be refused");
        };
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("player data"));
        assert!(error.to_string().contains("stale player"));
    }

    #[test]
    fn unrepresentable_inventory_slot_is_refused() {
        init_vanilla_registry();
        let mut nbt = NbtCompound::new();
        nbt.insert("DataVersion", DATA_VERSION);
        nbt.insert("Dimension", "minecraft:overworld".to_owned());
        nbt.insert(
            "Inventory",
            NbtList::Compound(vec![slot(60, "minecraft:stone", 1)]),
        );
        let Err(error) = player_from_vanilla_nbt(&nbt, "bad slot") else {
            panic!("slot 60 has no Steel destination and must be refused");
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("slot 60"), "{error}");
    }

    #[tokio::test]
    async fn player_data_files_import_and_export_between_worlds() {
        init_vanilla_registry();
        let directory = test_directory("playerdata-world");
        let vanilla = directory.join("vanilla");
        let steel_world = directory.join("steel").join("worlds").join("overworld");
        fs::create_dir_all(vanilla.join("playerdata"))
            .await
            .expect("vanilla playerdata directory");
        fs::create_dir_all(&steel_world)
            .await
            .expect("steel world directory");

        let vanilla_file = vanilla
            .join("playerdata")
            .join(format!("{}.dat", sample_uuid()));
        fs::write(
            &vanilla_file,
            write_gzip_nbt(&sample_player_nbt(DATA_VERSION)).expect("record encodes"),
        )
        .await
        .expect("vanilla player record");

        let (players, unmapped) =
            import_player_data(&vanilla, &steel_world).expect("player import");
        assert_eq!(players, 1);
        assert!(unmapped.contains(&"Air"));

        let steel_file = directory
            .join("steel")
            .join("players")
            .join("data")
            .join(sample_uuid().to_string());
        let bytes = fs::read(&steel_file)
            .await
            .expect("Steel player file was written");
        let stored = decode_persistent_player(&bytes, "test").expect("Steel player file decodes");
        assert_eq!(stored.pos, [1.5, 64.0, -2.5]);
        assert_eq!(stored.experience_total, 100);

        let exported = directory.join("exported");
        assert_eq!(
            export_player_data(&steel_world, &exported).expect("player export"),
            1
        );
        let exported_file = exported
            .join("playerdata")
            .join(format!("{}.dat", sample_uuid()));
        let exported_bytes = fs::read(&exported_file)
            .await
            .expect("exported player record");
        let exported_nbt = read_compressed_nbt(&exported_bytes).expect("gzip NBT");
        assert_eq!(exported_nbt.int("DataVersion"), Some(DATA_VERSION));
        assert_eq!(exported_nbt.float("Health"), Some(18.5));

        fs::remove_dir_all(directory)
            .await
            .expect("test directory should be removable");
    }
}
