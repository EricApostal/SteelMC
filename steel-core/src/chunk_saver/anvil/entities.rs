//! Vanilla entity NBT ⇄ `PersistentEntity`.
//!
//! Vanilla stores entities in `entities/r.<x>.<z>.mca` as normal `Entity.save` compounds
//! (`id`, `Pos`, `Motion`, `Rotation`, `UUID`, `Air`, `Fire`, passengers, …), while Steel
//! keeps the same information in typed `PersistentEntity` fields plus a verbatim
//! `save_additional` compound. The two base key groups are therefore mapped explicitly and
//! every remaining key is carried through untouched, which is exactly the set
//! `Entity::load_additional` expects.
//!
//! Deliberately dropped on write (Steel has no vanilla key for them, and vanilla recomputes
//! them from the world): `PersistentEntity::{is_in_powder_snow, was_in_powder_snow}`.

use std::io;
use std::str::FromStr;

use simdnbt::borrow::NbtCompound as BorrowedNbtCompound;
use simdnbt::owned::{NbtCompound, NbtList, NbtTag};
use steel_utils::Identifier;
use steel_utils::uuid_ext::UuidExt as _;
use uuid::Uuid;

use crate::chunk_saver::format::PersistentEntity;

use super::nbt::{compound_list, read_raw_nbt, write_raw_nbt};

/// Vanilla base keys consumed into `PersistentEntity` fields instead of `nbt_data`.
const BASE_KEYS: &[&str] = &[
    "id",
    "Pos",
    "Motion",
    "Rotation",
    "UUID",
    "Fire",
    "Air",
    "OnGround",
    "Invulnerable",
    "PortalCooldown",
    "TicksFrozen",
    "HasVisualFire",
    "CustomName",
    "CustomNameVisible",
    "Silent",
    "NoGravity",
    "Glowing",
    "Tags",
    "data",
    "FallDistance",
    "fall_distance",
    "Passengers",
];

/// Serializes one persistent entity (and its passengers) to vanilla entity NBT.
#[must_use]
pub(crate) fn entity_to_nbt(entity: &PersistentEntity) -> NbtCompound {
    // Type-specific `save_additional` data first, then the base keys so a stray
    // duplicate in the type data can never shadow a synchronized base field.
    let mut nbt = if entity.nbt_data.is_empty() {
        NbtCompound::new()
    } else {
        read_raw_nbt(&entity.nbt_data).unwrap_or_default()
    };

    nbt.insert("id", entity.entity_type.to_string());
    nbt.insert("UUID", NbtTag::IntArray(uuid_to_int_array(entity.uuid)));
    nbt.insert(
        "Pos",
        NbtList::Double(vec![entity.pos[0], entity.pos[1], entity.pos[2]]),
    );
    nbt.insert(
        "Motion",
        NbtList::Double(vec![entity.motion[0], entity.motion[1], entity.motion[2]]),
    );
    nbt.insert(
        "Rotation",
        NbtList::Float(vec![entity.rotation[0], entity.rotation[1]]),
    );
    nbt.insert("FallDistance", entity.fall_distance);
    nbt.insert("Fire", NbtTag::Short(entity.remaining_fire_ticks as i16));
    nbt.insert("Air", NbtTag::Short(entity.air_supply as i16));
    nbt.insert("OnGround", nbt_bool(entity.on_ground));
    nbt.insert("Invulnerable", nbt_bool(entity.invulnerable));
    nbt.insert("NoGravity", nbt_bool(entity.no_gravity));
    nbt.insert("PortalCooldown", entity.portal_cooldown);
    if entity.ticks_frozen > 0 {
        nbt.insert("TicksFrozen", entity.ticks_frozen);
    }
    if entity.has_visual_fire {
        nbt.insert("HasVisualFire", nbt_bool(true));
    }
    if !entity.custom_name_nbt.is_empty() {
        if let Ok(root) = read_raw_nbt(&entity.custom_name_nbt)
            && let Some(name) = root.get("CustomName")
        {
            nbt.insert("CustomName", name.clone());
        }
    }
    if entity.custom_name_visible {
        nbt.insert("CustomNameVisible", nbt_bool(true));
    }
    if entity.silent {
        nbt.insert("Silent", nbt_bool(true));
    }
    if entity.glowing {
        nbt.insert("Glowing", nbt_bool(true));
    }
    if !entity.tags.is_empty() {
        nbt.insert("Tags", NbtList::from(entity.tags.clone()));
    }
    if !entity.custom_data_nbt.is_empty()
        && let Ok(custom_data) = read_raw_nbt(&entity.custom_data_nbt)
        && !custom_data.is_empty()
    {
        nbt.insert("data", NbtTag::Compound(custom_data));
    }
    if !entity.passengers.is_empty() {
        nbt.insert(
            "Passengers",
            NbtList::Compound(entity.passengers.iter().map(entity_to_nbt).collect()),
        );
    }

    nbt
}

/// Parses vanilla entity NBT into a persistent entity (passengers included).
///
/// # Errors
/// Returns `InvalidData` when the entity type id is missing or unparsable.
pub(crate) fn entity_from_nbt(tag: &BorrowedNbtCompound<'_, '_>) -> io::Result<PersistentEntity> {
    let entity_type = tag
        .string("id")
        .and_then(|id| Identifier::from_str(id.to_str().as_ref()).ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "vanilla entity NBT has no parsable `id`",
            )
        })?;

    let uuid = tag
        .get("UUID")
        .and_then(|tag| tag.int_array())
        .and_then(|array| Uuid::from_int_array(&array))
        .map_or([0u8; 16], |uuid| *uuid.as_bytes());

    let pos = read_dvec3(tag, "Pos").unwrap_or([0.0; 3]);
    let motion = read_dvec3(tag, "Motion").unwrap_or([0.0; 3]);
    let rotation = tag
        .list("Rotation")
        .and_then(|list| list.floats())
        .filter(|values| values.len() >= 2)
        .map_or([0.0f32; 2], |values| [values[0], values[1]]);

    let custom_name_nbt = tag.get("CustomName").map_or_else(Vec::new, |name| {
        let mut root = NbtCompound::new();
        root.insert("CustomName", name.to_owned());
        write_raw_nbt(&root)
    });
    let custom_data_nbt = tag
        .compound("data")
        .map_or_else(Vec::new, |data| write_raw_nbt(&data.to_owned()));

    let tags = tag
        .list("Tags")
        .and_then(|list| list.strings())
        .map(|tags| {
            tags.iter()
                .map(|tag| tag.to_str().into_owned())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let mut nbt_data = NbtCompound::new();
    for (key, value) in tag.iter() {
        let key = key.to_str();
        if BASE_KEYS.contains(&key.as_ref()) {
            continue;
        }
        nbt_data.insert(key.as_ref(), value.to_owned());
    }

    let passengers = tag
        .list("Passengers")
        .map_or_else(Vec::new, compound_list)
        .iter()
        .map(entity_from_nbt)
        .collect::<io::Result<Vec<_>>>()?;

    Ok(PersistentEntity {
        entity_type,
        uuid,
        pos,
        motion,
        rotation,
        fall_distance: tag
            .double("FallDistance")
            .or_else(|| tag.double("fall_distance"))
            .unwrap_or(0.0),
        remaining_fire_ticks: read_int(tag, "Fire").unwrap_or(0),
        ticks_frozen: read_int(tag, "TicksFrozen").unwrap_or(0),
        is_in_powder_snow: false,
        was_in_powder_snow: false,
        has_visual_fire: tag.byte("HasVisualFire").is_some_and(|value| value != 0),
        on_ground: tag.byte("OnGround").is_some_and(|value| value != 0),
        no_gravity: tag.byte("NoGravity").is_some_and(|value| value != 0),
        invulnerable: tag.byte("Invulnerable").is_some_and(|value| value != 0),
        air_supply: read_int(tag, "Air").unwrap_or(300),
        portal_cooldown: tag.int("PortalCooldown").unwrap_or(0),
        custom_name_nbt,
        custom_name_visible: tag
            .byte("CustomNameVisible")
            .is_some_and(|value| value != 0),
        silent: tag.byte("Silent").is_some_and(|value| value != 0),
        glowing: tag.byte("Glowing").is_some_and(|value| value != 0),
        tags,
        custom_data_nbt,
        nbt_data: if nbt_data.is_empty() {
            Vec::new()
        } else {
            write_raw_nbt(&nbt_data)
        },
        passengers,
    })
}

fn read_dvec3(tag: &BorrowedNbtCompound<'_, '_>, key: &str) -> Option<[f64; 3]> {
    let values = tag.list(key)?.doubles()?;
    if values.len() < 3 {
        return None;
    }
    Some([values[0], values[1], values[2]])
}

/// Reads a vanilla integer that vanilla itself may store as int, short, or byte.
fn read_int(tag: &BorrowedNbtCompound<'_, '_>, key: &str) -> Option<i32> {
    tag.int(key)
        .or_else(|| tag.short(key).map(i32::from))
        .or_else(|| tag.byte(key).map(i32::from))
}

fn nbt_bool(value: bool) -> NbtTag {
    NbtTag::Byte(i8::from(value))
}

fn uuid_to_int_array(uuid: [u8; 16]) -> Vec<i32> {
    Uuid::from_bytes(uuid).to_int_array().to_vec()
}
