use super::{
    BlockPredicateJson, BlockStateShorthandJson, DamageSourcePredicateJson,
    DamageSourceTagPredicateJson, EnchantmentTargetJson, EntityEffectJson,
    EntityFlagsPredicateJson, EntityLocationPredicateJson, EntityPredicateJson, EntityTargetJson,
    EntityTypePredicateJson, EntityTypeSetJson, EntityTypeSpecificPredicateJson,
    EntityVehiclePredicateJson, FloatProviderJson, GameType, Ident, Identifier, ItemHolderSetJson,
    LevelBasedValueJson, LocationCheckBlockJson, MobEffectSelectionJson,
    ParticlePositionSourceJson, ParticleVelocitySourceJson, PlayerPredicateJson, RequirementsJson,
    Span, TokenStream, quote,
};

pub(super) fn slot_to_tokens(slot: &str) -> TokenStream {
    match slot {
        "any" => quote! { EquipmentSlotGroup::Any },
        "hand" => quote! { EquipmentSlotGroup::Hand },
        "mainhand" => quote! { EquipmentSlotGroup::MainHand },
        "offhand" => quote! { EquipmentSlotGroup::OffHand },
        "armor" => quote! { EquipmentSlotGroup::Armor },
        "head" => quote! { EquipmentSlotGroup::Head },
        "chest" => quote! { EquipmentSlotGroup::Chest },
        "legs" => quote! { EquipmentSlotGroup::Legs },
        "feet" => quote! { EquipmentSlotGroup::Feet },
        "body" => quote! { EquipmentSlotGroup::Body },
        other => panic!("Unknown equipment slot group: {other}"),
    }
}

pub(super) fn identifier_token(identifier: &Identifier) -> TokenStream {
    let namespace = identifier.namespace.as_ref();
    let path = identifier.path.as_ref();
    quote! { Identifier::new_static(#namespace, #path) }
}

pub(super) fn damage_type_ref_token(identifier: &Identifier) -> TokenStream {
    assert_eq!(
        identifier.namespace.as_ref(),
        "minecraft",
        "vanilla enchantment damage_type references must use the minecraft namespace: {identifier}"
    );
    let ident = Ident::new(&identifier.path.to_ascii_uppercase(), Span::call_site());
    quote! { &crate::vanilla_damage_types::#ident }
}

pub(super) fn parse_identifier(raw: &str) -> Result<Identifier, String> {
    raw.parse::<Identifier>()
        .map_err(|error| format!("invalid identifier {raw}: {error}"))
}

/// Parses a value that must be a `#`-prefixed tag reference (as opposed to the
/// tag-or-direct-id shapes handled inline elsewhere in this module).
pub(super) fn parse_required_tag(raw: &str) -> Result<Identifier, String> {
    let tag = raw
        .strip_prefix('#')
        .ok_or_else(|| format!("expected a tag reference (`#...`), got {raw}"))?;
    parse_identifier(tag)
}

pub(super) fn object_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<&'a serde_json::Value, String> {
    object
        .get(field)
        .ok_or_else(|| format!("missing enchantment requirement field `{field}`"))
}

pub(super) fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String, String> {
    object_field(object, field)?
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("enchantment requirement field `{field}` must be a string"))
}

pub(super) fn parse_entity_target(raw: &str) -> Result<EntityTargetJson, String> {
    match raw {
        "this" => Ok(EntityTargetJson::This),
        "attacker" => Ok(EntityTargetJson::Attacker),
        "direct_attacker" => Ok(EntityTargetJson::DirectAttacker),
        other => Err(format!("unsupported enchantment entity target `{other}`")),
    }
}

pub(super) fn parse_enchantment_target(raw: &str) -> Result<EnchantmentTargetJson, String> {
    match raw {
        "attacker" => Ok(EnchantmentTargetJson::Attacker),
        "damaging_entity" => Ok(EnchantmentTargetJson::DamagingEntity),
        "victim" => Ok(EnchantmentTargetJson::Victim),
        other => Err(format!(
            "unsupported enchantment post-attack target `{other}`"
        )),
    }
}

pub(super) fn parse_level_based_value_json(
    value: &serde_json::Value,
) -> Result<LevelBasedValueJson, String> {
    serde_json::from_value(value.to_owned())
        .map_err(|error| format!("invalid level-based value: {error}"))
}

pub(super) fn parse_random_chance_value(
    value: &serde_json::Value,
) -> Result<LevelBasedValueJson, String> {
    let Some(object) = value.as_object() else {
        return parse_level_based_value_json(value);
    };
    if object.get("type").and_then(serde_json::Value::as_str) != Some("minecraft:enchantment_level")
    {
        return parse_level_based_value_json(value);
    }
    parse_level_based_value_json(object_field(object, "amount")?)
}

pub(super) fn parse_mob_effect_selection_json(
    value: &serde_json::Value,
) -> Result<MobEffectSelectionJson, String> {
    let raw = value
        .as_str()
        .ok_or_else(|| "mob effect selection must be a string".to_owned())?;
    let Some(tag) = raw.strip_prefix('#') else {
        return Ok(MobEffectSelectionJson::Single(parse_identifier(raw)?));
    };

    Ok(MobEffectSelectionJson::UnsupportedTag(parse_identifier(
        tag,
    )?))
}

pub(super) fn parse_entity_effect_json(
    value: &serde_json::Value,
) -> Result<EntityEffectJson, String> {
    let Some(object) = value.as_object() else {
        return Err("enchantment entity effect must be an object".to_owned());
    };
    let effect_type = string_field(object, "type")?;

    match effect_type.as_str() {
        "minecraft:all_of" => {
            let effects = object_field(object, "effects")?
                .as_array()
                .ok_or_else(|| "all_of entity effect `effects` must be an array".to_owned())?
                .iter()
                .map(parse_entity_effect_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(EntityEffectJson::AllOf(effects))
        }
        "minecraft:change_item_damage" => {
            for key in object.keys() {
                if key != "type" && key != "amount" {
                    return Err(format!(
                        "unsupported change_item_damage effect field `{key}`"
                    ));
                }
            }
            Ok(EntityEffectJson::ChangeItemDamage {
                amount: parse_level_based_value_json(object_field(object, "amount")?)?,
            })
        }
        "minecraft:apply_exhaustion" => {
            for key in object.keys() {
                if key != "type" && key != "amount" {
                    return Err(format!("unsupported apply_exhaustion effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::ApplyExhaustion {
                amount: parse_level_based_value_json(object_field(object, "amount")?)?,
            })
        }
        "minecraft:apply_impulse" => {
            for key in object.keys() {
                if !matches!(
                    key.as_str(),
                    "type" | "direction" | "coordinate_scale" | "magnitude"
                ) {
                    return Err(format!("unsupported apply_impulse effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::ApplyImpulse {
                direction: parse_vec3_json(object_field(object, "direction")?)?,
                coordinate_scale: parse_vec3_json(object_field(object, "coordinate_scale")?)?,
                magnitude: parse_level_based_value_json(object_field(object, "magnitude")?)?,
            })
        }
        "minecraft:damage_entity" => {
            for key in object.keys() {
                if !matches!(
                    key.as_str(),
                    "type" | "min_damage" | "max_damage" | "damage_type"
                ) {
                    return Err(format!("unsupported damage_entity effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::DamageEntity {
                min_damage: parse_level_based_value_json(object_field(object, "min_damage")?)?,
                max_damage: parse_level_based_value_json(object_field(object, "max_damage")?)?,
                damage_type: parse_identifier(&string_field(object, "damage_type")?)?,
            })
        }
        "minecraft:play_sound" => {
            for key in object.keys() {
                if !matches!(key.as_str(), "type" | "sound" | "volume" | "pitch") {
                    return Err(format!("unsupported play_sound effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::PlaySound {
                sounds: parse_sound_list_json(object_field(object, "sound")?)?,
                volume: parse_float_provider_json(object_field(object, "volume")?)?,
                pitch: parse_float_provider_json(object_field(object, "pitch")?)?,
            })
        }
        "minecraft:ignite" => {
            for key in object.keys() {
                if key != "type" && key != "duration" {
                    return Err(format!("unsupported ignite effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::Ignite {
                duration: parse_level_based_value_json(object_field(object, "duration")?)?,
            })
        }
        "minecraft:replace_disk" => {
            for key in object.keys() {
                if !matches!(
                    key.as_str(),
                    "type"
                        | "radius"
                        | "height"
                        | "offset"
                        | "predicate"
                        | "block_state"
                        | "trigger_game_event"
                ) {
                    return Err(format!("unsupported replace_disk effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::ReplaceDisk {
                radius: parse_level_based_value_json(object_field(object, "radius")?)?,
                height: parse_level_based_value_json(object_field(object, "height")?)?,
                offset: match object.get("offset") {
                    Some(value) => parse_vec3i_json(value)?,
                    None => [0, 0, 0],
                },
                predicate: match object.get("predicate") {
                    Some(value) => Some(parse_block_predicate_json(value)?),
                    None => None,
                },
                block_state: parse_block_state_shorthand_json(object_field(
                    object,
                    "block_state",
                )?)?,
                trigger_game_event: match object.get("trigger_game_event") {
                    Some(value) => {
                        let raw = value.as_str().ok_or_else(|| {
                            "replace_disk `trigger_game_event` must be a string".to_owned()
                        })?;
                        Some(parse_identifier(raw)?)
                    }
                    None => None,
                },
            })
        }
        "minecraft:apply_mob_effect" => {
            for key in object.keys() {
                if !matches!(
                    key.as_str(),
                    "type"
                        | "to_apply"
                        | "min_duration"
                        | "max_duration"
                        | "min_amplifier"
                        | "max_amplifier"
                ) {
                    return Err(format!("unsupported apply_mob_effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::ApplyMobEffect {
                to_apply: parse_mob_effect_selection_json(object_field(object, "to_apply")?)?,
                min_duration: parse_level_based_value_json(object_field(object, "min_duration")?)?,
                max_duration: parse_level_based_value_json(object_field(object, "max_duration")?)?,
                min_amplifier: parse_level_based_value_json(object_field(
                    object,
                    "min_amplifier",
                )?)?,
                max_amplifier: parse_level_based_value_json(object_field(
                    object,
                    "max_amplifier",
                )?)?,
            })
        }
        "minecraft:summon_entity" => {
            for key in object.keys() {
                if !matches!(key.as_str(), "type" | "entity" | "join_team") {
                    return Err(format!("unsupported summon_entity effect field `{key}`"));
                }
            }
            Ok(EntityEffectJson::SummonEntity {
                entity_types: parse_entity_type_set_json(object_field(object, "entity")?)?,
                join_team: optional_bool_field(object, "join_team")?.unwrap_or(false),
            })
        }
        "minecraft:spawn_particles" => {
            for key in object.keys() {
                if !matches!(
                    key.as_str(),
                    "type"
                        | "particle"
                        | "horizontal_position"
                        | "vertical_position"
                        | "horizontal_velocity"
                        | "vertical_velocity"
                        | "speed"
                ) {
                    return Err(format!("unsupported spawn_particles effect field `{key}`"));
                }
            }
            let speed = match object.get("speed") {
                Some(value) => parse_float_provider_json(value)?,
                None => FloatProviderJson::Constant(0.0),
            };
            Ok(EntityEffectJson::SpawnParticles {
                particle: parse_particle_options_json(object_field(object, "particle")?)?,
                horizontal_position: parse_particle_position_source_json(object_field(
                    object,
                    "horizontal_position",
                )?)?,
                vertical_position: parse_particle_position_source_json(object_field(
                    object,
                    "vertical_position",
                )?)?,
                horizontal_velocity: parse_particle_velocity_source_json(object_field(
                    object,
                    "horizontal_velocity",
                )?)?,
                vertical_velocity: parse_particle_velocity_source_json(object_field(
                    object,
                    "vertical_velocity",
                )?)?,
                speed,
            })
        }
        "minecraft:attribute" => Ok(EntityEffectJson::Attribute(
            serde_json::from_value(value.clone())
                .map_err(|error| format!("invalid attribute effect: {error}"))?,
        )),
        _ => Ok(EntityEffectJson::Unsupported {
            effect_type: parse_identifier(&effect_type)?,
        }),
    }
}

/// Parses vanilla `ParticleOptions` (`{"type":"minecraft:soul"}` and the
/// option-carrying forms) down to the particle type id.
///
/// Vanilla keeps the per-particle option payload (dust color, block state, …)
/// on the effect; enchantment data in the vanilla datapack only uses
/// option-free particles, so only the `type` field is decoded here and the
/// payload is documented as unsupported rather than silently dropped at runtime.
pub(super) fn parse_particle_options_json(value: &serde_json::Value) -> Result<Identifier, String> {
    match value {
        serde_json::Value::String(raw) => parse_identifier(raw),
        serde_json::Value::Object(object) => {
            let raw = object
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "particle `type` must be a string".to_owned())?;
            parse_identifier(raw)
        }
        _ => Err("particle must be an object with a `type` field".to_owned()),
    }
}

/// Parses vanilla `HolderSet<EntityType>` (`"minecraft:x"`, `"#minecraft:x"` or a list).
pub(super) fn parse_entity_type_set_json(
    value: &serde_json::Value,
) -> Result<EntityTypeSetJson, String> {
    match value {
        serde_json::Value::String(raw) => {
            if let Some(tag) = raw.strip_prefix('#') {
                return Ok(EntityTypeSetJson::Tag(parse_identifier(tag)?));
            }
            Ok(EntityTypeSetJson::Direct(vec![parse_identifier(raw)?]))
        }
        serde_json::Value::Array(entries) => entries
            .iter()
            .map(|entry| {
                let raw = entry
                    .as_str()
                    .ok_or_else(|| "summon_entity entity entries must be strings".to_owned())?;
                parse_identifier(raw)
            })
            .collect::<Result<Vec<_>, _>>()
            .map(EntityTypeSetJson::Direct),
        _ => Err("summon_entity `entity` must be a string or string array".to_owned()),
    }
}

/// Parses vanilla `SpawnParticlesEffect.PositionSource`.
pub(super) fn parse_particle_position_source_json(
    value: &serde_json::Value,
) -> Result<ParticlePositionSourceJson, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid particle position source: {error}"))
}

/// Parses vanilla `SpawnParticlesEffect.VelocitySource`.
pub(super) fn parse_particle_velocity_source_json(
    value: &serde_json::Value,
) -> Result<ParticleVelocitySourceJson, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid particle velocity source: {error}"))
}

/// Parses a vanilla `FloatProvider` (`constant` inline form or a typed object).
pub(super) fn parse_float_provider_json(
    value: &serde_json::Value,
) -> Result<FloatProviderJson, String> {
    serde_json::from_value(value.clone())
        .map_err(|error| format!("invalid float provider: {error}"))
}

pub(super) fn parse_vec3_json(value: &serde_json::Value) -> Result<[f64; 3], String> {
    let Some(values) = value.as_array() else {
        return Err("vec3 must be an array".to_owned());
    };
    let [x, y, z] = values.as_slice() else {
        return Err("vec3 must have exactly three values".to_owned());
    };

    Ok([
        x.as_f64()
            .ok_or_else(|| "vec3 x must be a number".to_owned())?,
        y.as_f64()
            .ok_or_else(|| "vec3 y must be a number".to_owned())?,
        z.as_f64()
            .ok_or_else(|| "vec3 z must be a number".to_owned())?,
    ])
}

/// Parses a vanilla `Vec3i` (`offset`) as three whole-number components,
/// mirroring `Vec3i.CODEC`'s integer stream.
pub(super) fn parse_vec3i_json(value: &serde_json::Value) -> Result<[i32; 3], String> {
    let Some(values) = value.as_array() else {
        return Err("vec3i must be an array".to_owned());
    };
    let [x, y, z] = values.as_slice() else {
        return Err("vec3i must have exactly three values".to_owned());
    };

    let component = |component: &serde_json::Value, axis: char| {
        component
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .ok_or_else(|| format!("vec3i {axis} must be a whole number in i32 range"))
    };
    Ok([component(x, 'x')?, component(y, 'y')?, component(z, 'z')?])
}

/// Parses a vanilla `HolderSet` restricted to direct ids (a single id or an id
/// list). Tag references are rejected: the runtime holder-set data these feed
/// only carries direct refs.
pub(super) fn parse_identifier_list_json(
    value: &serde_json::Value,
    field: &str,
) -> Result<Vec<Identifier>, String> {
    let raw = match value {
        serde_json::Value::String(raw) => vec![raw.as_str()],
        serde_json::Value::Array(values) => values
            .iter()
            .map(|entry| {
                entry
                    .as_str()
                    .ok_or_else(|| format!("`{field}` entries must be strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(format!("`{field}` must be a string or string array")),
    };

    raw.into_iter()
        .map(|entry| {
            if entry.starts_with('#') {
                return Err(format!(
                    "`{field}` tag holder sets are not supported for enchantment predicates"
                ));
            }
            parse_identifier(entry)
        })
        .collect()
}

/// Parses the inline `{"id": ..., "properties": {...}}` block-state shorthand
/// used by `ReplaceDisk.block_state`.
pub(super) fn parse_block_state_shorthand_json(
    value: &serde_json::Value,
) -> Result<BlockStateShorthandJson, String> {
    let Some(object) = value.as_object() else {
        return Err("replace_disk `block_state` must be an inline block state object".to_owned());
    };
    for key in object.keys() {
        if key != "id" && key != "properties" {
            return Err(format!("unsupported block state field `{key}`"));
        }
    }
    let Some(raw) = object.get("id").and_then(serde_json::Value::as_str) else {
        return Err("block state `id` must be a string".to_owned());
    };
    let name = parse_identifier(raw)?;
    let mut properties = Vec::new();
    if let Some(raw) = object.get("properties") {
        let Some(object) = raw.as_object() else {
            return Err("block state `properties` must be an object".to_owned());
        };
        for (key, value) in object {
            let Some(value) = value.as_str() else {
                return Err(format!("block state property `{key}` must be a string"));
            };
            properties.push((key.clone(), value.to_owned()));
        }
    }

    Ok(BlockStateShorthandJson { name, properties })
}

/// Parses a vanilla block predicate used by `ReplaceDisk.predicate`.
/// Only the types vanilla enchantment data uses are accepted.
pub(super) fn parse_block_predicate_json(
    value: &serde_json::Value,
) -> Result<BlockPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("block predicate must be an object".to_owned());
    };
    let predicate_type = string_field(object, "type")?;

    match predicate_type.as_str() {
        "minecraft:all_of" => {
            let predicates = object_field(object, "predicates")?
                .as_array()
                .ok_or_else(|| "all_of block predicate `predicates` must be an array".to_owned())?
                .iter()
                .map(parse_block_predicate_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(BlockPredicateJson::AllOf(predicates))
        }
        "minecraft:matching_block_tag" => {
            for key in object.keys() {
                if key != "type" && key != "tag" && key != "offset" {
                    return Err(format!("unsupported matching_block_tag field `{key}`"));
                }
            }
            Ok(BlockPredicateJson::MatchingBlockTag {
                tag: parse_identifier(&string_field(object, "tag")?)?,
                offset: parse_block_predicate_offset(object)?,
            })
        }
        "minecraft:matching_blocks" => {
            for key in object.keys() {
                if key != "type" && key != "blocks" && key != "offset" {
                    return Err(format!("unsupported matching_blocks field `{key}`"));
                }
            }
            Ok(BlockPredicateJson::MatchingBlocks {
                blocks: parse_identifier_list_json(object_field(object, "blocks")?, "blocks")?,
                offset: parse_block_predicate_offset(object)?,
            })
        }
        "minecraft:matching_fluids" => {
            for key in object.keys() {
                if key != "type" && key != "fluids" && key != "offset" {
                    return Err(format!("unsupported matching_fluids field `{key}`"));
                }
            }
            Ok(BlockPredicateJson::MatchingFluids {
                fluids: parse_identifier_list_json(object_field(object, "fluids")?, "fluids")?,
                offset: parse_block_predicate_offset(object)?,
            })
        }
        "minecraft:unobstructed" => {
            for key in object.keys() {
                if key != "type" && key != "offset" {
                    return Err(format!("unsupported unobstructed field `{key}`"));
                }
            }
            Ok(BlockPredicateJson::Unobstructed {
                offset: parse_block_predicate_offset(object)?,
            })
        }
        other => Err(format!(
            "unsupported block predicate type `{other}` for enchantment effects"
        )),
    }
}

/// Parses the inline block predicate of a `location_check` requirement.
///
/// Loot `LocationPredicate.block` uses `StateTestingPredicate`'s inline form
/// (`{"blocks": "#minecraft:lightning_rods"}`), which omits the `type`
/// discriminator. Returns `None` for a predicate shape the generated
/// `LocationCheckBlock` cannot represent, so the caller keeps the requirement
/// `Unsupported`.
fn parse_location_check_block_json(
    value: &serde_json::Value,
) -> Result<Option<LocationCheckBlockJson>, String> {
    let Some(object) = value.as_object() else {
        return Err("location_check block predicate must be an object".to_owned());
    };

    if object.contains_key("type") {
        return Ok(match parse_block_predicate_json(value)? {
            BlockPredicateJson::MatchingBlockTag {
                tag,
                offset: [0, 0, 0],
            } => Some(LocationCheckBlockJson::Tag(tag)),
            BlockPredicateJson::MatchingBlocks {
                blocks,
                offset: [0, 0, 0],
            } => Some(LocationCheckBlockJson::Blocks(blocks)),
            _ => None,
        });
    }

    for key in object.keys() {
        if key != "blocks" {
            return Ok(None);
        }
    }
    let Some(blocks) = object.get("blocks") else {
        return Ok(None);
    };
    if let Some(raw) = blocks.as_str() {
        return Ok(Some(match raw.strip_prefix('#') {
            Some(tag) => LocationCheckBlockJson::Tag(parse_identifier(tag)?),
            None => LocationCheckBlockJson::Blocks(vec![parse_identifier(raw)?]),
        }));
    }
    let blocks = parse_identifier_list_json(blocks, "blocks")?;
    Ok(Some(LocationCheckBlockJson::Blocks(blocks)))
}

/// Reads a state-testing predicate `offset`, defaulting to zero like vanilla's
/// `stateTestingCodec` (`Vec3i.CODEC.optionalFieldOf("offset", Vec3i.ZERO)`).
fn parse_block_predicate_offset(
    object: &serde_json::Map<String, serde_json::Value>,
) -> Result<[i32; 3], String> {
    match object.get("offset") {
        Some(value) => parse_vec3i_json(value),
        None => Ok([0, 0, 0]),
    }
}

pub(super) fn parse_sound_list_json(value: &serde_json::Value) -> Result<Vec<Identifier>, String> {
    match value {
        serde_json::Value::String(raw) => Ok(vec![parse_identifier(raw)?]),
        serde_json::Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .ok_or_else(|| "sound list entries must be strings".to_owned())
                    .and_then(parse_identifier)
            })
            .collect(),
        _ => Err("play_sound `sound` must be a string or string array".to_owned()),
    }
}

pub(super) fn parse_entity_type_predicate(raw: &str) -> Result<EntityTypePredicateJson, String> {
    let Some(tag) = raw.strip_prefix('#') else {
        return Ok(EntityTypePredicateJson::Type(parse_identifier(raw)?));
    };

    Ok(EntityTypePredicateJson::Tag(parse_identifier(tag)?))
}

pub(super) fn parse_entity_predicate_json(
    value: &serde_json::Value,
) -> Result<EntityPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("entity_properties predicate must be an object".to_owned());
    };
    let unsupported = object.keys().any(|key| {
        !matches!(
            key.as_str(),
            "type"
                | "minecraft:entity_type"
                | "vehicle"
                | "minecraft:vehicle"
                | "flags"
                | "minecraft:flags"
                | "type_specific"
                | "minecraft:type_specific/player"
                | "location"
                | "minecraft:location"
        )
    });
    let entity_type = match aliased_object_field(object, &["type", "minecraft:entity_type"])? {
        Some(serde_json::Value::String(raw)) => parse_entity_type_predicate(raw)?,
        Some(_) => return Err("entity_properties predicate `type` must be a string".to_owned()),
        None => EntityTypePredicateJson::Any,
    };
    let vehicle = match aliased_object_field(object, &["vehicle", "minecraft:vehicle"])? {
        Some(serde_json::Value::Object(vehicle)) if vehicle.is_empty() => {
            EntityVehiclePredicateJson::Present
        }
        Some(serde_json::Value::Object(_)) => EntityVehiclePredicateJson::Unsupported,
        Some(_) => return Err("entity_properties predicate `vehicle` must be an object".to_owned()),
        None => EntityVehiclePredicateJson::Any,
    };
    let flags = aliased_object_field(object, &["flags", "minecraft:flags"])?
        .map(parse_entity_flags_predicate_json)
        .transpose()?
        .unwrap_or_else(EntityFlagsPredicateJson::any);
    let type_specific =
        match aliased_object_field(object, &["type_specific", "minecraft:type_specific/player"])? {
            Some(value) if object.contains_key("minecraft:type_specific/player") => {
                EntityTypeSpecificPredicateJson::Player(parse_player_predicate_json(value, false)?)
            }
            Some(value) => parse_type_specific_predicate_json(value)?,
            None => EntityTypeSpecificPredicateJson::Any,
        };
    let location = aliased_object_field(object, &["location", "minecraft:location"])?
        .map(parse_entity_location_predicate_json)
        .transpose()?
        .unwrap_or_else(EntityLocationPredicateJson::any);

    Ok(EntityPredicateJson {
        entity_type,
        vehicle,
        flags,
        type_specific,
        location: location.has_constraints().then_some(location),
        unsupported,
    })
}

pub(super) fn parse_entity_location_predicate_json(
    value: &serde_json::Value,
) -> Result<EntityLocationPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("entity location predicate must be an object".to_owned());
    };
    let mut unsupported = object.keys().any(|key| key != "can_see_sky");
    let can_see_sky = optional_bool_field(object, "can_see_sky")?;
    if can_see_sky.is_none() && !unsupported {
        // Keep fail-closed semantics: an empty location predicate says nothing about
        // the world, so drop it back into the unknown bucket.
        unsupported = true;
    }

    Ok(EntityLocationPredicateJson {
        can_see_sky,
        unsupported,
    })
}

pub(super) fn aliased_object_field<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    fields: &[&str],
) -> Result<Option<&'a serde_json::Value>, String> {
    let mut found: Option<(&str, &serde_json::Value)> = None;
    for field in fields {
        if let Some(value) = object.get(*field) {
            if let Some(previous) = found {
                let previous_field = previous.0;
                return Err(format!(
                    "entity_properties predicate must not contain both `{previous_field}` and `{field}`"
                ));
            }
            found = Some((*field, value));
        }
    }

    Ok(found.map(|(_, value)| value))
}

pub(super) fn parse_entity_flags_predicate_json(
    value: &serde_json::Value,
) -> Result<EntityFlagsPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("entity flags predicate must be an object".to_owned());
    };
    let unsupported = object
        .keys()
        .any(|key| !(key == "is_fall_flying" || key == "is_in_water" || key == "is_on_ground"));
    let is_fall_flying = optional_bool_field(object, "is_fall_flying")?;
    let is_in_water = optional_bool_field(object, "is_in_water")?;
    let is_on_ground = optional_bool_field(object, "is_on_ground")?;

    Ok(EntityFlagsPredicateJson {
        is_fall_flying,
        is_in_water,
        is_on_ground,
        unsupported,
    })
}

pub(super) fn parse_type_specific_predicate_json(
    value: &serde_json::Value,
) -> Result<EntityTypeSpecificPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("entity type_specific predicate must be an object".to_owned());
    };
    let predicate_type = string_field(object, "type")?;
    if predicate_type != "minecraft:player" {
        return Ok(EntityTypeSpecificPredicateJson::Unsupported);
    }

    Ok(EntityTypeSpecificPredicateJson::Player(
        parse_player_predicate_json(value, true)?,
    ))
}

pub(super) fn parse_player_predicate_json(
    value: &serde_json::Value,
    allow_type_field: bool,
) -> Result<PlayerPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("player predicate must be an object".to_owned());
    };
    let unsupported = object.keys().any(|key| {
        !matches!(key.as_str(), "gamemode" | "food") && !(allow_type_field && key == "type")
    });
    let game_modes = match object.get("gamemode") {
        Some(serde_json::Value::Array(modes)) => modes
            .iter()
            .map(|mode| {
                mode.as_str()
                    .ok_or_else(|| "player gamemode entries must be strings".to_owned())
                    .and_then(parse_game_type)
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("player predicate `gamemode` must be an array".to_owned()),
        None => Vec::new(),
    };
    let food_level_min = object
        .get("food")
        .map(parse_player_food_min_json)
        .transpose()?;

    Ok(PlayerPredicateJson {
        game_modes,
        food_level_min,
        unsupported,
    })
}

pub(super) fn parse_player_food_min_json(value: &serde_json::Value) -> Result<i32, String> {
    let Some(object) = value.as_object() else {
        return Err("player food predicate must be an object".to_owned());
    };
    let level = object_field(object, "level")?;
    let Some(level_object) = level.as_object() else {
        return Err("player food `level` must be an object".to_owned());
    };
    for key in object.keys() {
        if key != "level" {
            return Err(format!("unsupported player food predicate field `{key}`"));
        }
    }
    for key in level_object.keys() {
        if key != "min" {
            return Err(format!("unsupported player food level field `{key}`"));
        }
    }
    let min = object_field(level_object, "min")?
        .as_i64()
        .ok_or_else(|| "player food level `min` must be an integer".to_owned())?;
    i32::try_from(min).map_err(|_| "player food level `min` out of range".to_owned())
}

pub(super) fn optional_bool_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Option<bool>, String> {
    match object.get(field) {
        Some(serde_json::Value::Bool(value)) => Ok(Some(*value)),
        Some(_) => Err(format!("`{field}` must be a bool")),
        None => Ok(None),
    }
}

pub(super) fn parse_game_type(value: &str) -> Result<GameType, String> {
    match value {
        "survival" => Ok(GameType::Survival),
        "creative" => Ok(GameType::Creative),
        "adventure" => Ok(GameType::Adventure),
        "spectator" => Ok(GameType::Spectator),
        other => Err(format!("unknown game type `{other}`")),
    }
}

pub(super) fn parse_damage_source_predicate_json(
    value: &serde_json::Value,
) -> Result<DamageSourcePredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("damage_source_properties predicate must be an object".to_owned());
    };
    for key in object.keys() {
        if key != "tags" && key != "is_direct" {
            return Err(format!(
                "unsupported damage_source_properties predicate field `{key}`"
            ));
        }
    }
    let tags = match object.get("tags") {
        Some(serde_json::Value::Array(tags)) => tags
            .iter()
            .map(parse_damage_source_tag_predicate_json)
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err("damage_source_properties predicate `tags` must be an array".to_owned());
        }
        None => Vec::new(),
    };
    let is_direct = match object.get("is_direct") {
        Some(serde_json::Value::Bool(is_direct)) => Some(*is_direct),
        Some(_) => {
            return Err("damage_source_properties predicate `is_direct` must be a bool".to_owned());
        }
        None => None,
    };

    Ok(DamageSourcePredicateJson { tags, is_direct })
}

pub(super) fn parse_damage_source_tag_predicate_json(
    value: &serde_json::Value,
) -> Result<DamageSourceTagPredicateJson, String> {
    let Some(object) = value.as_object() else {
        return Err("damage source tag predicate must be an object".to_owned());
    };
    let id = string_field(object, "id")?;
    let expected = object_field(object, "expected")?
        .as_bool()
        .ok_or_else(|| "damage source tag predicate `expected` must be a bool".to_owned())?;
    for key in object.keys() {
        if key != "id" && key != "expected" {
            return Err(format!("unsupported damage source tag field `{key}`"));
        }
    }

    // `tags` entries are always a damage-type tag reference (`DamageTypeTags`), written with
    // the vanilla `#` sigil like any other tag id.
    Ok(DamageSourceTagPredicateJson {
        tag: parse_required_tag(&id)?,
        expected,
    })
}

pub(super) fn parse_requirements_json(
    value: &serde_json::Value,
) -> Result<RequirementsJson, String> {
    let Some(object) = value.as_object() else {
        return Err("enchantment effect requirements must be an object".to_owned());
    };
    let condition = string_field(object, "type")?;

    match condition.as_str() {
        "minecraft:all_of" => {
            let terms = object_field(object, "terms")?
                .as_array()
                .ok_or_else(|| "all_of requirements `terms` must be an array".to_owned())?
                .iter()
                .map(parse_requirements_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RequirementsJson::AllOf(terms))
        }
        "minecraft:any_of" => {
            let terms = object_field(object, "terms")?
                .as_array()
                .ok_or_else(|| "any_of requirements `terms` must be an array".to_owned())?
                .iter()
                .map(parse_requirements_json)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(RequirementsJson::AnyOf(terms))
        }
        "minecraft:inverted" => {
            let term = parse_requirements_json(object_field(object, "term")?)?;
            Ok(RequirementsJson::Inverted(Box::new(term)))
        }
        "minecraft:entity_properties" => {
            let entity = parse_entity_target(&string_field(object, "entity")?)?;
            let predicate = parse_entity_predicate_json(object_field(object, "predicate")?)?;
            Ok(RequirementsJson::EntityProperties { entity, predicate })
        }
        "minecraft:damage_source_properties" => {
            let predicate = parse_damage_source_predicate_json(object_field(object, "predicate")?)?;
            Ok(RequirementsJson::DamageSourceProperties(predicate))
        }
        "minecraft:random_chance" => {
            for key in object.keys() {
                if key != "type" && key != "chance" {
                    return Err(format!(
                        "unsupported random_chance requirement field `{key}`"
                    ));
                }
            }
            Ok(RequirementsJson::RandomChance {
                chance: parse_random_chance_value(object_field(object, "chance")?)?,
            })
        }
        "minecraft:weather_check" => {
            for key in object.keys() {
                if key != "type" && key != "raining" && key != "thundering" {
                    return Err(format!(
                        "unsupported weather_check requirement field `{key}`"
                    ));
                }
            }
            let raining = optional_bool_field(object, "raining")?;
            let thundering = optional_bool_field(object, "thundering")?;
            if raining.is_none() && thundering.is_none() {
                return Err("weather_check requirement needs `raining` or `thundering`".to_owned());
            }
            Ok(RequirementsJson::WeatherCheck {
                raining,
                thundering,
            })
        }
        "minecraft:match_tool" => {
            for key in object.keys() {
                if key != "type" && key != "predicate" {
                    return Err(format!("unsupported match_tool requirement field `{key}`"));
                }
            }
            let predicate = object_field(object, "predicate")?
                .as_object()
                .ok_or_else(|| "match_tool `predicate` must be an object".to_owned())?;
            for key in predicate.keys() {
                if key != "items" {
                    return Err(format!("unsupported match_tool predicate field `{key}`"));
                }
            }
            let items = predicate
                .get("items")
                .map(parse_item_holder_set_json)
                .transpose()?;
            Ok(RequirementsJson::MatchTool { items })
        }
        "minecraft:location_check" => {
            // Vanilla's `LocationCheck` carries a full `LocationPredicate`.
            // Only the subset used by shipped enchantment data is modelled;
            // anything else stays `Unsupported` so the fold stays fail-closed.
            let unsupported = |condition: &str| {
                Ok(RequirementsJson::Unsupported {
                    condition: parse_identifier(condition)?,
                })
            };
            for key in object.keys() {
                if key != "type" && key != "predicate" && key != "offset" {
                    return unsupported(&condition);
                }
            }
            let Some(predicate) = object.get("predicate").and_then(|v| v.as_object()) else {
                return unsupported(&condition);
            };
            for key in predicate.keys() {
                if key != "block" && key != "can_see_sky" {
                    return unsupported(&condition);
                }
            }
            let offset = match object.get("offset") {
                Some(value) => parse_vec3i_json(value)?,
                None => [0, 0, 0],
            };
            let can_see_sky = optional_bool_field(predicate, "can_see_sky")?;
            let block = match predicate.get("block") {
                None => None,
                Some(value) => match parse_location_check_block_json(value)? {
                    Some(block) => Some(block),
                    None => return unsupported(&condition),
                },
            };
            Ok(RequirementsJson::LocationCheck {
                offset,
                block,
                can_see_sky,
            })
        }
        _ => Ok(RequirementsJson::Unsupported {
            condition: parse_identifier(&condition)?,
        }),
    }
}

pub(super) fn parse_item_holder_set_json(
    value: &serde_json::Value,
) -> Result<ItemHolderSetJson, String> {
    if let Some(value) = value.as_str() {
        if let Some(tag) = value.strip_prefix('#') {
            return Ok(ItemHolderSetJson::Tag(parse_identifier(tag)?));
        }
        return Ok(ItemHolderSetJson::Direct(vec![parse_identifier(value)?]));
    }

    let values = value
        .as_array()
        .ok_or_else(|| "match_tool `items` must be an item, item tag, or item list".to_owned())?;
    let items = values
        .iter()
        .map(|value| {
            let value = value
                .as_str()
                .ok_or_else(|| "match_tool item list entries must be strings".to_owned())?;
            if value.starts_with('#') {
                return Err("match_tool direct item lists cannot contain tags".to_owned());
            }
            parse_identifier(value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ItemHolderSetJson::Direct(items))
}
