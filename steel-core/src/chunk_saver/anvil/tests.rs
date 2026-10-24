//! Round-trip tests for the Anvil container and codecs.
//!
//! `level.dat` round-tripping lives in [`crate::level_data::anvil`] because it needs access to
//! private level-data fields.

use std::io;
use std::path::PathBuf;
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use simdnbt::borrow::read_compound as read_borrowed_compound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_registry::init_vanilla_registry;
use steel_registry::vanilla_blocks;
use steel_utils::{ChunkPos, Identifier};
use tokio::fs;

use crate::chunk::heightmap::HeightmapType;
use crate::chunk::light::DATA_LAYER_SIZE;
use crate::chunk::status::ChunkStatus;
use crate::chunk_saver::anvil::chunk::{chunk_to_nbt, nbt_to_chunk};
use crate::chunk_saver::anvil::entities::{entity_from_nbt, entity_to_nbt};
use crate::chunk_saver::anvil::nbt::{read_compressed_nbt, write_gzip_nbt, write_raw_nbt};
use crate::chunk_saver::anvil::poi::{VanillaPoi, poi_from_nbt, poi_to_nbt};
use crate::chunk_saver::anvil::region::AnvilRegionFile;
use crate::chunk_saver::bit_pack::pack_indices_from_iter;
use crate::chunk_saver::format::{
    PersistentBiomeData, PersistentBlockState, PersistentLightData, PersistentLightSection,
    PersistentSection, PersistentTick,
};
use crate::chunk_saver::{PersistentChunk, PersistentEntity, PersistentHeightmap, PersistentPoi};

const MIN_Y: i32 = -64;
const HEIGHT: i32 = 384;

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

fn test_directory(name: &str) -> PathBuf {
    let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("steel-anvil-{name}-{}-{sequence}", process::id()))
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

fn reborrow(bytes: &[u8]) -> simdnbt::borrow::BaseNbtCompound<'_> {
    read_borrowed_compound(&mut io::Cursor::new(bytes)).expect("test NBT should parse")
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
    let (decoded, status) = nbt_to_chunk(&view, pos, MIN_Y, HEIGHT).expect("chunk should decode");

    assert_eq!(status, ChunkStatus::Full);
    assert_eq!(decoded.last_modified, chunk.last_modified);
    assert_eq!(decoded.inhabited_time, chunk.inhabited_time);
    assert_eq!(decoded.sections.len(), chunk.sections.len());
    assert_eq!(decoded.block_states, chunk.block_states);
    assert_eq!(decoded.biomes, chunk.biomes);
    assert_eq!(decoded.block_ticks.len(), 1);
    assert_eq!(decoded.block_ticks[0].delay, 42);
    assert_eq!(decoded.block_ticks[0].priority, 2);
    assert_eq!(decoded.heightmaps.len(), 1);
    assert_eq!(decoded.heightmaps[0].data, chunk.heightmaps[0].data);
    assert_eq!(decoded.entities.len(), 1);
    assert_eq!(decoded.entities[0].uuid, chunk.entities[0].uuid);
    assert_eq!(decoded.entities[0].tags, vec!["steel".to_owned()]);
    assert_eq!(decoded.light.block.len(), 1);
    assert_eq!(decoded.light.sky.len(), 1);
    assert_eq!(
        decoded.light.block[0].section_index(),
        chunk.light.block[0].section_index()
    );
    assert_eq!(decoded.postprocessing, chunk.postprocessing);
    assert!(decoded.structure_starts.is_empty());
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
    let mut passengers = Vec::new();
    passengers.push(entity_to_nbt(&entity));

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
    let raids = crate::entity::raid::RaidsData {
        raids: vec![crate::entity::raid::RaidRecord {
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
    let nbt = crate::chunk_saver::anvil::saved_data::raids_to_nbt(&raids);
    assert_eq!(
        crate::chunk_saver::anvil::saved_data::raids_from_nbt(&nbt),
        raids
    );

    let weather = crate::level_data::WeatherState {
        raining: true,
        rain_time: 5,
        thundering: false,
        thunder_time: 6,
        clear_weather_time: 7,
    };
    let nbt = crate::chunk_saver::anvil::saved_data::weather_to_nbt(&weather);
    let decoded = crate::chunk_saver::anvil::saved_data::weather_from_nbt(&nbt);
    assert_eq!(decoded.raining, weather.raining);
    assert_eq!(decoded.rain_time, weather.rain_time);
    assert_eq!(decoded.clear_weather_time, weather.clear_weather_time);

    let mut rules = rustc_hash::FxHashMap::default();
    rules.insert("keep_inventory".to_owned(), serde_json::Value::Bool(true));
    rules.insert("random_tick_speed".to_owned(), serde_json::Value::from(3));
    let nbt = crate::chunk_saver::anvil::saved_data::game_rules_to_nbt(&rules);
    let decoded = crate::chunk_saver::anvil::saved_data::game_rules_from_nbt(&nbt);
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
