//! Vanilla `level.dat` and dimension `data/*.dat` ⇄ Steel level data.
//!
//! Vanilla keeps the world-level metadata in gzip-compressed NBT under a root `Data` compound;
//! Steel keeps the same information in `level.toml`. This module converts between the two so a
//! world can be imported from vanilla and mirrored back on every save.
//!
//! Fields present in vanilla but intentionally **not** modelled by Steel are dropped with an
//! explicit warning: `InhabitedTime`-style legacy keys, `snapshot`/`version_history` bookkeeping
//! beyond the version stamp, `Player` (singleplayer) blocks, and the `dimensions` registry inside
//! `WorldGenSettings` (Steel derives dimensions from its own generated datapack).

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::Identifier;
use steel_utils::types::Difficulty;
use steel_utils::version::{DATA_VERSION, DATA_VERSION_SERIES, VERSION_ID, VERSION_NAME};

use crate::chunk_saver::anvil::saved_data::{
    game_rules_from_nbt, game_rules_to_nbt, read_saved_data_file, read_world_gen_seed,
    weather_from_nbt, weather_to_nbt, write_saved_data_file,
};
use crate::level_data::{LevelData, SpawnPoint, WorldBorderData};

/// Vanilla `level.dat` file name.
pub const LEVEL_DAT_FILE: &str = "level.dat";
/// Vanilla backup of `level.dat`, written by `Util.safeReplaceFile`.
pub const LEVEL_DAT_OLD_FILE: &str = "level.dat_old";
/// Vanilla level-format version stamped into `Data.version`.
const LEVEL_FORMAT_VERSION: i32 = 19133;

static WARNED_UNMAPPED: AtomicBool = AtomicBool::new(false);

/// Reads the `Data` compound of a world's `level.dat`, if the file exists.
///
/// # Errors
/// Returns `InvalidData` when the file is not gzip NBT or has no `Data` compound.
pub fn read_level_dat(world_dir: &Path) -> io::Result<Option<NbtCompound>> {
    let bytes = match std::fs::read(world_dir.join(LEVEL_DAT_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            // Vanilla falls back to `level.dat_old` when the primary file is unreadable.
            match std::fs::read(world_dir.join(LEVEL_DAT_OLD_FILE)) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error),
            }
        }
        Err(error) => return Err(error),
    };
    let root = crate::chunk_saver::anvil::read_compressed_nbt(&bytes)?;
    match root.get("Data") {
        Some(NbtTag::Compound(data)) => Ok(Some(data.clone())),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{} has no `Data` compound", world_dir.display()),
        )),
    }
}

/// Reads only the generation seed of a vanilla world.
///
/// 26.3 stores it in `<world>/data/world_gen_settings.dat`; pre-1.21.2 worlds keep a legacy
/// `WorldGenSettings` compound inside `level.dat`, which is read as a fallback.
///
/// # Errors
/// Propagates filesystem and parse errors.
pub fn read_seed(world_dir: &Path) -> io::Result<Option<i64>> {
    if let Some(seed) = read_world_gen_seed(&world_dir.join("data").join("world_gen_settings.dat"))?
    {
        return Ok(Some(seed));
    }
    let Some(data) = read_level_dat(world_dir)? else {
        return Ok(None);
    };
    Ok(data
        .compound("WorldGenSettings")
        .or_else(|| data.compound("world_gen_settings"))
        .and_then(|settings| settings.long("seed")))
}

/// Applies a vanilla `level.dat` `Data` compound onto a Steel [`LevelData`].
///
/// Unmapped vanilla fields are ignored (the caller logs them once per world).
///
/// # Errors
/// Propagates saved-data read errors for the dimension `data/` directory.
pub(crate) fn apply_level_dat(
    data: &NbtCompound,
    dimension_dir: &Path,
    level: &mut LevelData,
) -> io::Result<()> {
    crate::datafixer::ensure_current(
        crate::datafixer::version_of(data),
        crate::datafixer::DataFixType::Level,
        "level.dat",
    )?;
    if let Some(time) = data.long("Time") {
        level.game_time = Some(time);
    }
    if let Some(spawn) = data.compound("spawn") {
        apply_spawn(spawn, level);
    } else if let Some(spawn) = legacy_spawn(data) {
        level.spawn = spawn;
    }
    if let Some(settings) = data.compound("difficulty_settings") {
        level.difficulty = read_difficulty(settings);
        level.difficulty_locked = settings.byte("locked").is_some_and(|value| value != 0);
    }
    if let Some(initialized) = data.byte("initialized") {
        level.initialized = initialized != 0;
    }
    level.world_border = read_world_border(data, &level.world_border);

    let data_dir = dimension_dir.join("data");
    if let Some(weather) = read_saved_data_file(&data_dir.join("weather.dat"))? {
        level.weather = weather_from_nbt(&weather);
    }
    if let Some(rules) = read_saved_data_file(&data_dir.join("game_rules.dat"))? {
        level.game_rules = game_rules_from_nbt(&rules);
        level.load_game_rules();
    }
    Ok(())
}

/// Builds a vanilla `level.dat` `Data` compound from Steel level data.
///
/// `bound_dimension` is the dimension key whose `spawn` position is being written; Steel stores
/// spawn as a bare position, so the caller supplies the key.
#[must_use]
pub(crate) fn level_data_to_level_dat(
    level: &LevelData,
    level_name: &str,
    bound_dimension: &Identifier,
) -> NbtCompound {
    let mut data = NbtCompound::new();
    data.insert("DataVersion", DATA_VERSION);
    data.insert("version", LEVEL_FORMAT_VERSION);
    data.insert("version_history", NbtList::Int(vec![LEVEL_FORMAT_VERSION]));

    let mut version = NbtCompound::new();
    version.insert("Name", VERSION_NAME);
    version.insert("Id", VERSION_ID);
    version.insert("Snapshot", NbtTag::Byte(0));
    version.insert("Series", DATA_VERSION_SERIES);
    data.insert("Version", NbtTag::Compound(version));

    data.insert("LevelName", level_name);
    data.insert("GameType", 0);
    data.insert("allowCommands", NbtTag::Byte(0));
    data.insert("initialized", NbtTag::Byte(i8::from(level.initialized)));
    data.insert("WasModded", NbtTag::Byte(0));
    data.insert("ServerBrands", NbtList::from(vec!["SteelMC".to_owned()]));
    data.insert("removed_features", NbtList::from(Vec::<String>::new()));
    data.insert("Time", level.game_time.unwrap_or(0));
    data.insert("LastPlayed", unix_millis());

    let mut spawn = NbtCompound::new();
    spawn.insert("dimension", bound_dimension.to_string());
    spawn.insert(
        "pos",
        NbtTag::IntArray(vec![level.spawn.x, level.spawn.y, level.spawn.z]),
    );
    spawn.insert("yaw", level.spawn.angle);
    spawn.insert("pitch", 0.0f32);
    data.insert("spawn", NbtTag::Compound(spawn));

    let mut difficulty = NbtCompound::new();
    difficulty.insert("difficulty", difficulty_name(level.difficulty));
    difficulty.insert("hardcore", NbtTag::Byte(0));
    difficulty.insert("locked", NbtTag::Byte(i8::from(level.difficulty_locked)));
    data.insert("difficulty_settings", NbtTag::Compound(difficulty));

    let mut packs = NbtCompound::new();
    packs.insert("Enabled", NbtList::from(vec!["vanilla".to_owned()]));
    packs.insert("Disabled", NbtList::from(Vec::<String>::new()));
    data.insert("DataPacks", NbtTag::Compound(packs));

    write_world_border(&level.world_border, &mut data);
    data
}

/// Writes `level.dat` (and its `level.dat_old` backup) from Steel level data.
///
/// # Errors
/// Propagates filesystem errors.
pub(crate) fn write_level_dat(
    world_dir: &Path,
    level: &LevelData,
    level_name: &str,
    bound_dimension: &Identifier,
) -> io::Result<()> {
    let mut root = NbtCompound::new();
    root.insert(
        "Data",
        NbtTag::Compound(level_data_to_level_dat(level, level_name, bound_dimension)),
    );
    let bytes = crate::chunk_saver::anvil::write_gzip_nbt(&root)?;
    std::fs::create_dir_all(world_dir)?;
    let path = world_dir.join(LEVEL_DAT_FILE);
    let temporary = world_dir.join("level.dat.tmp");
    std::fs::write(&temporary, bytes)?;
    if std::fs::exists(&path)? {
        // Mirrors `Util.safeReplaceFile(current, temp, old)`.
        std::fs::rename(&path, world_dir.join(LEVEL_DAT_OLD_FILE))?;
    }
    std::fs::rename(&temporary, &path)
}

/// Mirrors Steel's weather and game rules into the dimension's `data/` directory.
///
/// # Errors
/// Propagates filesystem errors.
pub(crate) fn write_dimension_saved_data(
    dimension_dir: &Path,
    level: &LevelData,
) -> io::Result<()> {
    let data_dir = dimension_dir.join("data");
    write_saved_data_file(
        &data_dir.join("weather.dat"),
        &weather_to_nbt(&level.weather),
    )?;
    write_saved_data_file(
        &data_dir.join("game_rules.dat"),
        &game_rules_to_nbt(&level.game_rules),
    )
}

/// Logs, once per process, that vanilla metadata Steel cannot model is being ignored.
pub(crate) fn warn_unmapped_level_dat() {
    if !WARNED_UNMAPPED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            "A vanilla level.dat was imported; fields Steel does not model (`Version` bookkeeping, `DataPacks`, `Player`, `WanderingTraderSpawner` state, `ServerBrands`, custom dimensions) are dropped and will not be re-emitted"
        );
    }
}

fn apply_spawn(spawn: &NbtCompound, level: &mut LevelData) {
    if let Some(pos) = spawn.int_array("pos")
        && pos.len() >= 3
    {
        level.spawn = SpawnPoint {
            x: pos[0],
            y: pos[1],
            z: pos[2],
            angle: spawn.float("yaw").unwrap_or(level.spawn.angle),
        };
    }
}

fn legacy_spawn(data: &NbtCompound) -> Option<SpawnPoint> {
    let x = data.int("SpawnX")?;
    let y = data.int("SpawnY")?;
    let z = data.int("SpawnZ")?;
    Some(SpawnPoint {
        x,
        y,
        z,
        angle: data.float("SpawnAngle").unwrap_or(0.0),
    })
}

fn read_difficulty(settings: &NbtCompound) -> Difficulty {
    if let Some(name) = settings.string("difficulty") {
        return match name.to_str().as_ref() {
            "peaceful" => Difficulty::Peaceful,
            "easy" => Difficulty::Easy,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Normal,
        };
    }
    if let Some(value) = settings
        .int("difficulty")
        .or_else(|| settings.byte("difficulty").map(i32::from))
    {
        return Difficulty::from(value.clamp(0, 3) as u8);
    }
    Difficulty::Normal
}

fn difficulty_name(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Peaceful => "peaceful",
        Difficulty::Easy => "easy",
        Difficulty::Normal => "normal",
        Difficulty::Hard => "hard",
    }
}

/// Reads `WorldBorder` fields, keeping Steel's current values for absent keys.
fn read_world_border(data: &NbtCompound, current: &WorldBorderData) -> WorldBorderData {
    WorldBorderData {
        center_x: data.double("BorderCenterX").unwrap_or(current.center_x),
        center_z: data.double("BorderCenterZ").unwrap_or(current.center_z),
        damage_per_block: data
            .double("BorderDamagePerBlock")
            .unwrap_or(current.damage_per_block),
        safe_zone: data.double("BorderSafeZone").unwrap_or(current.safe_zone),
        warning_blocks: data
            .int("BorderWarningBlocks")
            .unwrap_or(current.warning_blocks),
        warning_time: data
            .int("BorderWarningTime")
            .unwrap_or(current.warning_time),
        size: data.double("BorderSize").unwrap_or(current.size),
        lerp_time: data.long("BorderSizeLerpTime").unwrap_or(current.lerp_time),
        lerp_target: data
            .double("BorderSizeLerpTarget")
            .unwrap_or(current.lerp_target),
    }
}

fn write_world_border(border: &WorldBorderData, data: &mut NbtCompound) {
    data.insert("BorderCenterX", border.center_x);
    data.insert("BorderCenterZ", border.center_z);
    data.insert("BorderDamagePerBlock", border.damage_per_block);
    data.insert("BorderSafeZone", border.safe_zone);
    data.insert("BorderWarningBlocks", border.warning_blocks);
    data.insert("BorderWarningTime", border.warning_time);
    data.insert("BorderSize", border.size);
    data.insert("BorderSizeLerpTime", border.lerp_time);
    data.insert("BorderSizeLerpTarget", border.lerp_target);
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process;
    use std::sync::atomic::{AtomicU64, Ordering};

    use steel_registry::init_vanilla_registry;
    use steel_utils::Identifier;

    use super::{
        apply_level_dat, read_level_dat, read_seed, write_dimension_saved_data, write_level_dat,
    };
    use crate::level_data::{LevelData, SpawnPoint};

    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "steel-level-dat-{name}-{}-{sequence}",
            process::id()
        ))
    }

    #[test]
    fn level_dat_round_trips_steel_level_data() {
        init_vanilla_registry();
        let directory = test_directory("round-trip");
        std::fs::create_dir_all(&directory).expect("directory should be creatable");

        let mut level = LevelData::new_with_seed(0x5EED);
        level.spawn = SpawnPoint {
            x: 12,
            y: 70,
            z: -9,
            angle: 90.0,
        };
        level.weather.raining = true;
        level.weather.rain_time = 1200;
        level.initialized = true;
        level.game_time = Some(555);
        level.difficulty_locked = true;

        let dimension = Identifier::vanilla_static("overworld");
        write_level_dat(&directory, &level, "testworld", &dimension).expect("level.dat write");
        write_dimension_saved_data(&directory, &level).expect("dimension saved data write");

        assert!(directory.join("level.dat").exists());
        assert!(directory.join("data").join("weather.dat").exists());
        assert!(directory.join("data").join("game_rules.dat").exists());

        let data = read_level_dat(&directory)
            .expect("level.dat should read")
            .expect("level.dat should parse");
        let mut imported = LevelData::new_with_seed(0);
        apply_level_dat(&data, &directory, &mut imported).expect("level.dat should apply");

        assert_eq!(imported.spawn.x, level.spawn.x);
        assert_eq!(imported.spawn.y, level.spawn.y);
        assert_eq!(imported.spawn.z, level.spawn.z);
        assert_eq!(imported.spawn.angle, level.spawn.angle);
        assert!(imported.weather.raining);
        assert_eq!(imported.weather.rain_time, 1200);
        assert!(imported.initialized);
        assert_eq!(imported.game_time, Some(555));
        assert!(imported.difficulty_locked);

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn level_dat_write_keeps_a_backup_of_the_previous_file() {
        init_vanilla_registry();
        let directory = test_directory("backup");
        std::fs::create_dir_all(&directory).expect("directory should be creatable");
        let level = LevelData::new_with_seed(1);
        let dimension = Identifier::vanilla_static("overworld");

        write_level_dat(&directory, &level, "first", &dimension).expect("first write");
        write_level_dat(&directory, &level, "second", &dimension).expect("second write");
        assert!(
            directory.join("level.dat_old").exists(),
            "the previous level.dat is retained as level.dat_old"
        );

        std::fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn seed_is_read_from_the_world_gen_settings_payload() {
        init_vanilla_registry();
        let directory = test_directory("seed");
        std::fs::create_dir_all(directory.join("data")).expect("directory should be creatable");

        let mut settings = simdnbt::owned::NbtCompound::new();
        settings.insert("seed", 987_654_321i64);
        let mut root = simdnbt::owned::NbtCompound::new();
        root.insert("data", simdnbt::owned::NbtTag::Compound(settings));
        let bytes = crate::chunk_saver::anvil::write_gzip_nbt(&root).expect("gzip");
        std::fs::write(directory.join("data").join("world_gen_settings.dat"), bytes)
            .expect("settings should write");

        assert_eq!(
            read_seed(&directory).expect("seed should read"),
            Some(987_654_321)
        );
        std::fs::remove_dir_all(&directory).ok();
    }
}
