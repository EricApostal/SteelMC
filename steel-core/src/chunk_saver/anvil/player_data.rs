//! Vanilla `playerdata/<uuid>.dat` ⇄ Steel's persistent player snapshot.
//!
//! Vanilla's `PlayerDataStorage.save` writes `Player.saveWithoutId` into a gzip-compressed
//! `CompoundTag`; the classes defining that layout are `Player`, `ServerPlayer`,
//! `LivingEntity`, `Entity`, `FoodData`, `Abilities$Packed`, `Inventory`,
//! `ItemStackWithSlot`, `EntityEquipment`, `PlayerEnderChestContainer`,
//! `ServerRecipeBook$Packed`, `RecipeBookSettings`, `LevelData$RespawnData`,
//! `GlobalPos`, and `NbtUtils.addCurrentDataVersion`.
//!
//! Steel stores the same state in [`PersistentPlayerData`] inside its own `STLP` file. This
//! module maps the two formats field by field, including the inventory/equipment split:
//! vanilla writes the 36 main slots to `Inventory` and the armour/offhand slots to
//! `equipment`, while Steel keeps all of them in one 43-slot container (36..42 are the
//! equipment indices).
//!
//! Fields Steel models are read and written. Vanilla keys Steel has no destination for are
//! reported through [`ImportedPlayer::unmapped`] and logged once per player file, never
//! silently dropped. Vanilla `stats/<uuid>.json` and `advancements/<uuid>.json` are not part
//! of `playerdata/<uuid>.dat`; they are not converted here.

use std::fs;
use std::io;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use simdnbt::borrow::{NbtCompound as BorrowedNbtCompound, read_compound};
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_protocol::packets::game::{RecipeBookSettings, RecipeBookTypeSettings};
use steel_registry::item_stack::ItemStack;
use steel_utils::uuid_ext::UuidExt as _;
use steel_utils::{BlockPos, Identifier};
use uuid::Uuid;

use crate::datafixer::{DATA_VERSION, DataFixType, ensure_current, version_of};
use crate::level_data::RespawnData;
use crate::player::PlayerRespawnConfig;
use crate::player::player_data::{
    PLAYER_DATA_VERSION, PersistentAbilities, PersistentEnderPearl, PersistentPlayerData,
    PersistentRootVehicle, PersistentSlot,
};
use crate::player::player_data_storage::{decode_persistent_player, encode_persistent_player};
use crate::player::recipe_book::PackedRecipeBook;

use super::entities::{entity_from_nbt, entity_to_nbt};
use super::nbt::{compound_list, read_compressed_nbt, read_raw_nbt, write_gzip_nbt, write_raw_nbt};

/// Vanilla player-container indices 36..42 and the `equipment` key each one uses.
///
/// These are `PlayerInventory`'s equipment slots (`36 = feet` … `40 = offhand`,
/// `41 = body`, `42 = saddle`) and match vanilla `EquipmentSlot.getSerializedName`.
const EQUIPMENT_SLOTS: [(i8, &str); 7] = [
    (36, "feet"),
    (37, "legs"),
    (38, "chest"),
    (39, "head"),
    (40, "offhand"),
    (41, "body"),
    (42, "saddle"),
];

/// Vanilla player-data keys that Steel's [`PersistentPlayerData`] cannot represent.
///
/// Every key present in an imported file is listed in [`ImportedPlayer::unmapped`] and
/// logged, so a conversion never loses state silently.
const UNMODELLED_KEYS: &[&str] = &[
    "Air",
    "fall_distance",
    "PortalCooldown",
    "invulnerable_time",
    "Invulnerable",
    "Passengers",
    "CustomName",
    "CustomNameVisible",
    "Silent",
    "NoGravity",
    "Glowing",
    "Tags",
    "data",
    "HurtTime",
    "SleepTimer",
    "sleeping_pos",
    "Brain",
    "last_hurt_by_player",
    "last_hurt_by_player_memory_time",
    "last_hurt_by_mob",
    "ticks_since_last_hurt_by_mob",
    "locator_bar_icon",
    "current_impulse_context_reset_grace_time",
    "current_explosion_impact_pos",
    "LastDeathLocation",
    "warden_spawn_tracker",
    "entered_nether_pos",
    "spawn_extra_particles_on_fall",
    "raid_omen_position",
    "post_effects",
    "ShoulderEntityLeft",
    "ShoulderEntityRight",
    "last_explosion_impact_pos",
];

/// Keys [`player_from_vanilla_nbt`] consumes (modelled or explicitly reported).
const HANDLED_KEYS: &[&str] = &[
    "DataVersion",
    "UUID",
    "Pos",
    "Motion",
    "Rotation",
    "OnGround",
    "Fire",
    "TicksFrozen",
    "HasVisualFire",
    "Health",
    "DeathTime",
    "AbsorptionAmount",
    "attributes",
    "active_effects",
    "FallFlying",
    "Inventory",
    "SelectedItemSlot",
    "XpP",
    "XpLevel",
    "XpTotal",
    "XpSeed",
    "Score",
    "foodLevel",
    "foodTickTimer",
    "foodSaturationLevel",
    "foodExhaustionLevel",
    "abilities",
    "EnderItems",
    "equipment",
    "playerGameType",
    "previousPlayerGameType",
    "seenCredits",
    "RootVehicle",
    "recipeBook",
    "Dimension",
    "respawn",
    "ender_pearls",
];

/// A vanilla player record decoded into Steel's snapshot, plus the fields that were present
/// but have no Steel representation.
#[derive(Debug)]
pub(crate) struct ImportedPlayer {
    /// The converted snapshot.
    pub data: PersistentPlayerData,
    /// Vanilla keys present in the file that Steel does not model.
    pub unmapped: Vec<&'static str>,
}

/// Reads one vanilla `playerdata/<uuid>.dat` compound.
///
/// # Errors
/// Refuses a record whose `DataVersion` is not this build's, and returns `InvalidData` for
/// an inventory slot or item stack Steel cannot represent (rather than dropping it).
pub(crate) fn player_from_vanilla_nbt(
    nbt: &NbtCompound,
    source: &str,
) -> io::Result<ImportedPlayer> {
    ensure_current(version_of(nbt), DataFixType::Player, source)?;
    let bytes = write_raw_nbt(nbt);
    let base = read_compound(&mut Cursor::new(bytes.as_slice())).map_err(|error| {
        invalid(format!(
            "{source}: player record failed to decode: {error:?}"
        ))
    })?;
    let borrowed: BorrowedNbtCompound<'_, '_> = (&base).into();
    player_from_borrowed(&borrowed, source)
}

fn player_from_borrowed(
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<ImportedPlayer> {
    let pos = read_dvec3(nbt, "Pos");
    let motion = read_dvec3(nbt, "Motion");
    let rotation = nbt
        .list("Rotation")
        .and_then(|list| list.floats())
        .filter(|values| values.len() >= 2)
        .map_or([0.0; 2], |values| [values[0], values[1]]);
    let on_ground = read_bool(nbt.byte("OnGround"));
    let fall_flying = read_bool(nbt.byte("FallFlying"));
    let health = nbt.float("Health").unwrap_or(20.0);

    let mut inventory = read_slots(nbt, "Inventory", source, 42)?;
    merge_equipment(&mut inventory, nbt, source)?;

    let ender_items = read_slots(nbt, "EnderItems", source, 26)?;
    let world = nbt
        .string("Dimension")
        .map(|dimension| dimension.to_str().into_owned())
        .ok_or_else(|| invalid(format!("{source}: player record has no `Dimension`")))?;
    Identifier::from_str(&world)
        .map_err(|error| invalid(format!("{source}: invalid `Dimension` `{world}`: {error}")))?;

    let abilities = read_abilities(nbt);
    let respawn_config = read_respawn_config(nbt, source)?;
    let root_vehicle = read_root_vehicle(nbt, source)?;
    let ender_pearls = read_ender_pearls(nbt, source)?;
    let recipe_book = read_recipe_book(nbt, source)?;

    let living_nbt = living_nbt_from_vanilla(nbt, health, fall_flying);
    let unmapped = report_unmapped(nbt, source);

    let data = PersistentPlayerData {
        pos,
        motion,
        rotation,
        on_ground,
        fall_flying,
        remaining_fire_ticks: read_int(nbt, "Fire").unwrap_or(0),
        ticks_frozen: read_int(nbt, "TicksFrozen").unwrap_or(0),
        is_in_powder_snow: false,
        was_in_powder_snow: false,
        has_visual_fire: read_bool(nbt.byte("HasVisualFire")),
        health,
        living_nbt,
        game_mode: read_int(nbt, "playerGameType").unwrap_or(0),
        prev_game_mode: read_int(nbt, "previousPlayerGameType"),
        abilities,
        inventory,
        selected_slot: read_int(nbt, "SelectedItemSlot").unwrap_or(0),
        world,
        food_level: read_int(nbt, "foodLevel").unwrap_or(20),
        food_saturation_level: nbt.float("foodSaturationLevel").unwrap_or(5.0),
        food_exhaustion_level: nbt.float("foodExhaustionLevel").unwrap_or(0.0),
        food_tick_timer: read_int(nbt, "foodTickTimer").unwrap_or(0),
        // Steel's own save-format stamp; the vanilla `DataVersion` was gated above.
        data_version: PLAYER_DATA_VERSION,
        experience_level: read_int(nbt, "XpLevel").unwrap_or(0),
        experience_progress: nbt.float("XpP").unwrap_or(0.0),
        experience_total: read_int(nbt, "XpTotal").unwrap_or(0),
        enchantment_seed: read_int(nbt, "XpSeed").unwrap_or(0),
        score: read_int(nbt, "Score").unwrap_or(0),
        seen_credits: read_bool(nbt.byte("seenCredits")),
        root_vehicle,
        respawn_config,
        ender_pearls,
        ender_items,
        // Vanilla keeps stats and advancements in separate JSON files, not in this compound.
        stats: Vec::new(),
        advancements: Vec::new(),
        recipe_book,
    };

    Ok(ImportedPlayer { data, unmapped })
}

fn merge_equipment(
    inventory: &mut Vec<PersistentSlot>,
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<()> {
    let Some(equipment) = nbt.compound("equipment") else {
        return Ok(());
    };
    for (slot, key) in EQUIPMENT_SLOTS {
        let Some(stack) = equipment.get(key) else {
            continue;
        };
        let Some(compound) = stack.compound() else {
            return Err(invalid(format!(
                "{source}: equipment slot `{key}` is not an item compound"
            )));
        };
        let item = ItemStack::from_borrowed_compound(&compound).ok_or_else(|| {
            invalid(format!(
                "{source}: equipment slot `{key}` holds an invalid item stack"
            ))
        })?;
        push_slot(inventory, slot, item);
    }
    Ok(())
}

fn report_unmapped(nbt: &BorrowedNbtCompound<'_, '_>, source: &str) -> Vec<&'static str> {
    let unmapped = UNMODELLED_KEYS
        .iter()
        .copied()
        .filter(|key| nbt.contains(key))
        .collect::<Vec<_>>();
    for key in nbt.keys() {
        let key = key.to_str();
        if !HANDLED_KEYS.contains(&key.as_ref()) && !UNMODELLED_KEYS.contains(&key.as_ref()) {
            log::warn!(
                "{source}: player key `{key}` has no Steel representation and was not imported"
            );
        }
    }
    if !unmapped.is_empty() {
        log::warn!(
            "{source}: vanilla player keys without a Steel representation were not imported: {}",
            unmapped.join(", ")
        );
    }
    unmapped
}

/// Serializes a Steel snapshot as a vanilla `Player.saveWithoutId` compound.
///
/// # Errors
/// Returns `InvalidData` when the stored `living_nbt` payload is malformed or a slot index
/// has no vanilla representation; effects, attributes and items must not be dropped
/// silently.
pub(crate) fn player_to_vanilla_nbt(
    data: &PersistentPlayerData,
    uuid: Uuid,
) -> io::Result<NbtCompound> {
    validate_slots(data, uuid)?;

    let mut nbt = NbtCompound::new();
    nbt.insert("DataVersion", DATA_VERSION);
    nbt.insert("UUID", NbtTag::IntArray(uuid.to_int_array().to_vec()));
    nbt.insert("Pos", NbtList::Double(data.pos.to_vec()));
    nbt.insert("Motion", NbtList::Double(data.motion.to_vec()));
    nbt.insert("Rotation", NbtList::Float(data.rotation.to_vec()));
    nbt.insert("OnGround", nbt_bool(data.on_ground));
    nbt.insert("Fire", NbtTag::Short(data.remaining_fire_ticks as i16));
    if data.ticks_frozen > 0 {
        nbt.insert("TicksFrozen", data.ticks_frozen);
    }
    if data.has_visual_fire {
        nbt.insert("HasVisualFire", nbt_bool(true));
    }

    insert_living_state(&mut nbt, data, uuid)?;
    insert_inventory(&mut nbt, data)?;
    insert_progress(&mut nbt, data);
    insert_server_state(&mut nbt, data);
    Ok(nbt)
}

fn validate_slots(data: &PersistentPlayerData, uuid: Uuid) -> io::Result<()> {
    for slot in &data.inventory {
        if !slot.item.is_empty() && !(0..=42).contains(&slot.slot) {
            return Err(invalid(format!(
                "player {uuid}: inventory slot {} is outside vanilla's 0..=42 range",
                slot.slot
            )));
        }
    }
    for slot in &data.ender_items {
        if !slot.item.is_empty() && !(0..=26).contains(&slot.slot) {
            return Err(invalid(format!(
                "player {uuid}: ender chest slot {} is outside vanilla's 0..=26 range",
                slot.slot
            )));
        }
    }
    Ok(())
}

fn insert_living_state(
    nbt: &mut NbtCompound,
    data: &PersistentPlayerData,
    uuid: Uuid,
) -> io::Result<()> {
    let living = if data.living_nbt.is_empty() {
        NbtCompound::new()
    } else {
        read_raw_nbt(&data.living_nbt).map_err(|error| {
            invalid(format!(
                "player {uuid}: stored living NBT is malformed: {error}"
            ))
        })?
    };
    nbt.insert("Health", data.health);
    nbt.insert(
        "DeathTime",
        living.get("DeathTime").cloned().unwrap_or(NbtTag::Short(0)),
    );
    nbt.insert(
        "AbsorptionAmount",
        living
            .get("AbsorptionAmount")
            .cloned()
            .unwrap_or(NbtTag::Float(0.0)),
    );
    nbt.insert(
        "attributes",
        living
            .get("attributes")
            .cloned()
            .unwrap_or(NbtTag::List(NbtList::Compound(Vec::new()))),
    );
    if let Some(effects) = living.get("active_effects") {
        nbt.insert("active_effects", effects.clone());
    }
    nbt.insert("FallFlying", nbt_bool(data.fall_flying));
    Ok(())
}

fn insert_inventory(nbt: &mut NbtCompound, data: &PersistentPlayerData) -> io::Result<()> {
    nbt.insert(
        "Inventory",
        NbtList::Compound(
            data.inventory
                .iter()
                .filter(|slot| slot.slot < 36 && !slot.item.is_empty())
                .map(slot_to_nbt)
                .collect::<io::Result<Vec<_>>>()?,
        ),
    );
    nbt.insert("SelectedItemSlot", data.selected_slot);

    let mut equipment = NbtCompound::new();
    for (container_slot, key) in EQUIPMENT_SLOTS {
        if let Some(slot) = data
            .inventory
            .iter()
            .find(|slot| slot.slot == container_slot && !slot.item.is_empty())
        {
            equipment.insert(key, slot.item.to_nbt_tag_ref());
        }
    }
    if !equipment.is_empty() {
        nbt.insert("equipment", NbtTag::Compound(equipment));
    }

    nbt.insert(
        "EnderItems",
        NbtList::Compound(
            data.ender_items
                .iter()
                .filter(|slot| !slot.item.is_empty())
                .map(slot_to_nbt)
                .collect::<io::Result<Vec<_>>>()?,
        ),
    );
    Ok(())
}

fn insert_progress(nbt: &mut NbtCompound, data: &PersistentPlayerData) {
    nbt.insert("XpP", data.experience_progress);
    nbt.insert("XpLevel", data.experience_level);
    nbt.insert("XpTotal", data.experience_total);
    nbt.insert("XpSeed", data.enchantment_seed);
    nbt.insert("Score", data.score);
    nbt.insert("foodLevel", data.food_level);
    nbt.insert("foodTickTimer", data.food_tick_timer);
    nbt.insert("foodSaturationLevel", data.food_saturation_level);
    nbt.insert("foodExhaustionLevel", data.food_exhaustion_level);
    nbt.insert("abilities", abilities_to_nbt(&data.abilities));
    nbt.insert("playerGameType", data.game_mode);
    if let Some(previous) = data.prev_game_mode {
        nbt.insert("previousPlayerGameType", previous);
    }
    nbt.insert("seenCredits", nbt_bool(data.seen_credits));
    nbt.insert("Dimension", data.world.clone());
}

fn insert_server_state(nbt: &mut NbtCompound, data: &PersistentPlayerData) {
    if let Some(respawn) = &data.respawn_config {
        nbt.insert("respawn", respawn_to_nbt(respawn));
    }
    if let Some(root_vehicle) = &data.root_vehicle {
        let mut root = NbtCompound::new();
        root.insert(
            "Attach",
            NbtTag::IntArray(
                Uuid::from_bytes(root_vehicle.attach)
                    .to_int_array()
                    .to_vec(),
            ),
        );
        root.insert(
            "Entity",
            NbtTag::Compound(entity_to_nbt(&root_vehicle.entity)),
        );
        nbt.insert("RootVehicle", NbtTag::Compound(root));
    }
    if !data.ender_pearls.is_empty() {
        nbt.insert(
            "ender_pearls",
            NbtList::Compound(
                data.ender_pearls
                    .iter()
                    .map(|pearl| {
                        let mut entity = entity_to_nbt(&pearl.entity);
                        entity.insert("ender_pearl_dimension", pearl.world.clone());
                        entity
                    })
                    .collect(),
            ),
        );
    }
    nbt.insert("recipeBook", recipe_book_to_nbt(&data.recipe_book));
}

/// Imports every `playerdata/*.dat` of `vanilla_dir` into the domain's Steel player directory.
///
/// Returns the number of players converted and the vanilla keys that had no Steel
/// representation.
///
/// # Errors
/// Propagates filesystem errors, refuses data-version mismatches, and refuses item stacks
/// Steel cannot represent.
pub(crate) fn import_player_data(
    vanilla_dir: &Path,
    steel_dir: &Path,
) -> io::Result<(u64, Vec<&'static str>)> {
    let source_dir = vanilla_dir.join("playerdata");
    let mut entries = match fs::read_dir(&source_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok((0, Vec::new())),
        Err(error) => return Err(error),
    };
    let destination = players_dir(steel_dir).join("data");
    let mut converted = 0;
    let mut unmapped = Vec::new();
    while let Some(entry) = entries.next().transpose()? {
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "dat") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(stem) else {
            log::warn!(
                "{}: playerdata file name is not a UUID; skipping",
                path.display()
            );
            continue;
        };
        let bytes = fs::read(&path)?;
        let nbt = read_compressed_nbt(&bytes)?;
        let imported = player_from_vanilla_nbt(&nbt, &path.display().to_string())?;
        if let Some(tag_uuid) = nbt
            .get("UUID")
            .and_then(|tag| tag.int_array())
            .and_then(Uuid::from_int_array)
            && tag_uuid != uuid
        {
            log::warn!(
                "{}: record UUID {} differs from the file name; using the file name",
                path.display(),
                tag_uuid
            );
        }
        let encoded = encode_persistent_player(&imported.data)?;
        fs::create_dir_all(&destination)?;
        fs::write(destination.join(uuid.to_string()), encoded)?;
        for key in imported.unmapped {
            if !unmapped.contains(&key) {
                unmapped.push(key);
            }
        }
        converted += 1;
    }
    Ok((converted, unmapped))
}

/// Exports every player in the domain's Steel player directory to `playerdata/*.dat`.
///
/// # Errors
/// Propagates filesystem and decode errors.
pub(crate) fn export_player_data(steel_dir: &Path, vanilla_dir: &Path) -> io::Result<u64> {
    let source_dir = players_dir(steel_dir).join("data");
    let mut entries = match fs::read_dir(&source_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let destination = vanilla_dir.join("playerdata");
    let mut converted = 0;
    while let Some(entry) = entries.next().transpose()? {
        let path = entry.path();
        if !path.is_file() || path.extension().is_some() {
            continue;
        }
        let Some(stem) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Ok(uuid) = Uuid::parse_str(stem) else {
            log::warn!(
                "{}: Steel player file name is not a UUID; skipping",
                path.display()
            );
            continue;
        };
        let bytes = fs::read(&path)?;
        let data = decode_persistent_player(&bytes, &format!("player file {}", path.display()))?;
        let nbt = player_to_vanilla_nbt(&data, uuid)?;
        fs::create_dir_all(&destination)?;
        fs::write(
            destination.join(format!("{uuid}.dat")),
            write_gzip_nbt(&nbt)?,
        )?;
        converted += 1;
    }
    Ok(converted)
}

/// Resolves the domain `players` directory that owns the given Steel world directory.
///
/// The live layout is `<save root>/<domain>/worlds/<world>` with domain-wide players under
/// `<save root>/<domain>/players`; a world directory that does not follow that shape is
/// treated as the domain root itself.
fn players_dir(steel_dir: &Path) -> PathBuf {
    if steel_dir
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|name| name == "worlds")
        && let Some(domain) = steel_dir.parent().and_then(Path::parent)
    {
        return domain.join("players");
    }
    steel_dir.join("players")
}

fn read_slots(
    nbt: &BorrowedNbtCompound<'_, '_>,
    key: &str,
    source: &str,
    max_slot: i8,
) -> io::Result<Vec<PersistentSlot>> {
    let mut slots = Vec::new();
    let Some(list) = nbt.list(key) else {
        return Ok(slots);
    };
    for entry in compound_list(list) {
        let slot = read_int(&entry, "Slot")
            .ok_or_else(|| invalid(format!("{source}: `{key}` entry has no `Slot`")))?;
        let Ok(slot) = i8::try_from(slot) else {
            return Err(invalid(format!(
                "{source}: `{key}` slot {slot} does not fit Steel's slot index"
            )));
        };
        if !(0..=max_slot).contains(&slot) {
            return Err(invalid(format!(
                "{source}: `{key}` slot {slot} is outside Steel's 0..={max_slot} range"
            )));
        }
        let item = ItemStack::from_borrowed_compound(&entry).ok_or_else(|| {
            invalid(format!(
                "{source}: `{key}` slot {slot} holds an item stack Steel cannot represent"
            ))
        })?;
        if item.is_empty() {
            continue;
        }
        push_slot(&mut slots, slot, item);
    }
    Ok(slots)
}

fn push_slot(slots: &mut Vec<PersistentSlot>, slot: i8, item: ItemStack) {
    if let Some(existing) = slots.iter_mut().find(|entry| entry.slot == slot) {
        existing.item = item;
    } else {
        slots.push(PersistentSlot { slot, item });
    }
}

fn slot_to_nbt(slot: &PersistentSlot) -> io::Result<NbtCompound> {
    let NbtTag::Compound(mut compound) = slot.item.to_nbt_tag_ref() else {
        return Err(invalid(format!(
            "inventory slot {} item did not serialize to a compound",
            slot.slot
        )));
    };
    compound.insert("Slot", NbtTag::Byte(slot.slot));
    Ok(compound)
}

fn read_abilities(nbt: &BorrowedNbtCompound<'_, '_>) -> PersistentAbilities {
    let defaults = PersistentAbilities::default();
    let Some(abilities) = nbt.compound("abilities") else {
        return defaults;
    };
    PersistentAbilities {
        invulnerable: read_bool(abilities.byte("invulnerable")),
        flying: read_bool(abilities.byte("flying")),
        may_fly: read_bool(abilities.byte("mayfly")),
        instabuild: read_bool(abilities.byte("instabuild")),
        may_build: abilities
            .byte("mayBuild")
            .map_or(defaults.may_build, |value| value != 0),
        flying_speed: abilities.float("flySpeed").unwrap_or(defaults.flying_speed),
        walking_speed: abilities
            .float("walkSpeed")
            .unwrap_or(defaults.walking_speed),
    }
}

fn abilities_to_nbt(abilities: &PersistentAbilities) -> NbtTag {
    let mut nbt = NbtCompound::new();
    nbt.insert("invulnerable", nbt_bool(abilities.invulnerable));
    nbt.insert("flying", nbt_bool(abilities.flying));
    nbt.insert("mayfly", nbt_bool(abilities.may_fly));
    nbt.insert("instabuild", nbt_bool(abilities.instabuild));
    nbt.insert("mayBuild", nbt_bool(abilities.may_build));
    nbt.insert("flySpeed", abilities.flying_speed);
    nbt.insert("walkSpeed", abilities.walking_speed);
    NbtTag::Compound(nbt)
}

fn read_respawn_config(
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<Option<PlayerRespawnConfig>> {
    let Some(respawn) = nbt.compound("respawn") else {
        return Ok(None);
    };
    let dimension = respawn
        .string("dimension")
        .ok_or_else(|| invalid(format!("{source}: `respawn` has no `dimension`")))?;
    let dimension = Identifier::from_str(dimension.to_str().as_ref()).map_err(|error| {
        invalid(format!(
            "{source}: `respawn.dimension` is not an identifier: {error}"
        ))
    })?;
    let pos = respawn
        .list("pos")
        .and_then(|list| list.ints())
        .filter(|values| values.len() >= 3)
        .ok_or_else(|| {
            invalid(format!(
                "{source}: `respawn.pos` is not a 3-int block position"
            ))
        })?;
    Ok(Some(PlayerRespawnConfig::new(
        RespawnData::of(
            dimension,
            BlockPos::new(pos[0], pos[1], pos[2]),
            respawn.float("yaw").unwrap_or(0.0),
            respawn.float("pitch").unwrap_or(0.0),
        ),
        read_bool(respawn.byte("forced")),
    )))
}

fn respawn_to_nbt(config: &PlayerRespawnConfig) -> NbtTag {
    let pos = config.respawn_data.pos();
    let mut nbt = NbtCompound::new();
    nbt.insert("dimension", config.respawn_data.dimension().to_string());
    nbt.insert("pos", NbtList::Int(vec![pos.x(), pos.y(), pos.z()]));
    nbt.insert("yaw", config.respawn_data.yaw);
    nbt.insert("pitch", config.respawn_data.pitch);
    nbt.insert("forced", nbt_bool(config.forced));
    NbtTag::Compound(nbt)
}

fn read_root_vehicle(
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<Option<PersistentRootVehicle>> {
    let Some(root) = nbt.compound("RootVehicle") else {
        return Ok(None);
    };
    let attach = root
        .get("Attach")
        .and_then(|tag| tag.int_array())
        .and_then(|array| Uuid::from_int_array(&array))
        .ok_or_else(|| invalid(format!("{source}: `RootVehicle.Attach` is not a UUID")))?;
    let entity = root
        .compound("Entity")
        .ok_or_else(|| invalid(format!("{source}: `RootVehicle` has no `Entity`")))?;
    Ok(Some(PersistentRootVehicle {
        attach: *attach.as_bytes(),
        entity: entity_from_nbt(&entity)?,
    }))
}

fn read_ender_pearls(
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<Vec<PersistentEnderPearl>> {
    let Some(list) = nbt.list("ender_pearls") else {
        return Ok(Vec::new());
    };
    let mut pearls = Vec::new();
    for entry in compound_list(list) {
        let world = entry
            .string("ender_pearl_dimension")
            .ok_or_else(|| invalid(format!("{source}: ender pearl has no dimension")))?
            .to_str()
            .into_owned();
        // `entity_from_nbt` carries unknown keys into the entity's type data, so the pearl
        // dimension is stripped before decoding and re-added on export.
        let mut entity = NbtCompound::new();
        for (key, value) in entry.iter() {
            let key = key.to_str();
            if key.as_ref() != "ender_pearl_dimension" {
                entity.insert(key.as_ref(), value.to_owned());
            }
        }
        let bytes = write_raw_nbt(&entity);
        let base = read_compound(&mut Cursor::new(bytes.as_slice())).map_err(|error| {
            invalid(format!(
                "{source}: ender pearl entity failed to decode: {error:?}"
            ))
        })?;
        let borrowed: BorrowedNbtCompound<'_, '_> = (&base).into();
        pearls.push(PersistentEnderPearl {
            world,
            entity: entity_from_nbt(&borrowed)?,
        });
    }
    Ok(pearls)
}

fn read_recipe_book(
    nbt: &BorrowedNbtCompound<'_, '_>,
    source: &str,
) -> io::Result<PackedRecipeBook> {
    let Some(book) = nbt.compound("recipeBook") else {
        return Ok(PackedRecipeBook::default());
    };
    let type_settings = |open_key: &str, filtering_key: &str| RecipeBookTypeSettings {
        open: read_bool(book.byte(open_key)),
        filtering: read_bool(book.byte(filtering_key)),
    };
    Ok(PackedRecipeBook {
        settings: RecipeBookSettings {
            crafting: type_settings("isGuiOpen", "isFilteringCraftable"),
            furnace: type_settings("isFurnaceGuiOpen", "isFurnaceFilteringCraftable"),
            blast_furnace: type_settings(
                "isBlastingFurnaceGuiOpen",
                "isBlastingFurnaceFilteringCraftable",
            ),
            smoker: type_settings("isSmokerGuiOpen", "isSmokerFilteringCraftable"),
        },
        known: read_recipe_ids(&book, "recipes", source)?,
        highlight: read_recipe_ids(&book, "toBeDisplayed", source)?,
    })
}

fn read_recipe_ids(
    book: &BorrowedNbtCompound<'_, '_>,
    key: &str,
    source: &str,
) -> io::Result<Vec<Identifier>> {
    let Some(list) = book.list(key) else {
        return Ok(Vec::new());
    };
    let Some(values) = list.strings() else {
        return Err(invalid(format!(
            "{source}: `recipeBook.{key}` is not a string list"
        )));
    };
    values
        .iter()
        .map(|value| {
            Identifier::from_str(value.to_str().as_ref()).map_err(|error| {
                invalid(format!(
                    "{source}: `recipeBook.{key}` holds invalid id `{}`: {error}",
                    value.to_str()
                ))
            })
        })
        .collect()
}

fn recipe_book_to_nbt(book: &PackedRecipeBook) -> NbtTag {
    let mut nbt = NbtCompound::new();
    nbt.insert("isGuiOpen", nbt_bool(book.settings.crafting.open));
    nbt.insert(
        "isFilteringCraftable",
        nbt_bool(book.settings.crafting.filtering),
    );
    nbt.insert("isFurnaceGuiOpen", nbt_bool(book.settings.furnace.open));
    nbt.insert(
        "isFurnaceFilteringCraftable",
        nbt_bool(book.settings.furnace.filtering),
    );
    nbt.insert(
        "isBlastingFurnaceGuiOpen",
        nbt_bool(book.settings.blast_furnace.open),
    );
    nbt.insert(
        "isBlastingFurnaceFilteringCraftable",
        nbt_bool(book.settings.blast_furnace.filtering),
    );
    nbt.insert("isSmokerGuiOpen", nbt_bool(book.settings.smoker.open));
    nbt.insert(
        "isSmokerFilteringCraftable",
        nbt_bool(book.settings.smoker.filtering),
    );
    nbt.insert(
        "recipes",
        NbtList::String(
            book.known
                .iter()
                .map(ToString::to_string)
                .map(Into::into)
                .collect(),
        ),
    );
    nbt.insert(
        "toBeDisplayed",
        NbtList::String(
            book.highlight
                .iter()
                .map(ToString::to_string)
                .map(Into::into)
                .collect(),
        ),
    );
    NbtTag::Compound(nbt)
}

fn living_nbt_from_vanilla(
    nbt: &BorrowedNbtCompound<'_, '_>,
    health: f32,
    fall_flying: bool,
) -> Vec<u8> {
    let mut living = NbtCompound::new();
    living.insert("Health", nbt.float("Health").unwrap_or(health));
    living.insert(
        "DeathTime",
        NbtTag::Short(nbt.short("DeathTime").unwrap_or(0)),
    );
    living.insert(
        "AbsorptionAmount",
        nbt.float("AbsorptionAmount").unwrap_or(0.0),
    );
    living.insert(
        "attributes",
        nbt.list("attributes")
            .map_or(NbtList::Compound(Vec::new()), |list| list.to_owned()),
    );
    if let Some(effects) = nbt.list("active_effects") {
        living.insert("active_effects", effects.to_owned());
    }
    living.insert("FallFlying", nbt_bool(fall_flying));
    let mut bytes = Vec::new();
    living.write(&mut bytes);
    bytes
}

fn read_dvec3(nbt: &BorrowedNbtCompound<'_, '_>, key: &str) -> [f64; 3] {
    nbt.list(key)
        .and_then(|list| list.doubles())
        .filter(|values| values.len() >= 3)
        .map_or([0.0; 3], |values| [values[0], values[1], values[2]])
}

/// Reads a vanilla integer that vanilla itself may store as int, short, or byte.
fn read_int(nbt: &BorrowedNbtCompound<'_, '_>, key: &str) -> Option<i32> {
    nbt.int(key)
        .or_else(|| nbt.short(key).map(i32::from))
        .or_else(|| nbt.byte(key).map(i32::from))
}

fn read_bool(value: Option<i8>) -> bool {
    value.is_some_and(|value| value != 0)
}

fn nbt_bool(value: bool) -> NbtTag {
    NbtTag::Byte(i8::from(value))
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
