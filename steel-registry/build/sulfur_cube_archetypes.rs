//! Build script for generating vanilla `sulfur_cube_archetype` definitions.
//!
//! The `items` field is an item tag reference, so the archetype tags are resolved
//! recursively against every item tag file at build time.

#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted sulfur cube data"
)]

use std::{fs, path::Path};

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use steel_utils::Identifier;

/// Vanilla `SulfurCubeArchetype` JSON.
#[derive(Deserialize, Debug)]
struct SulfurCubeArchetypeJson {
    items: String,
    attribute_modifiers: Vec<AttributeEntryJson>,
    #[serde(default)]
    buoyant: bool,
    #[serde(default)]
    explosion: Option<ExplosionJson>,
    #[serde(default)]
    contact_damage: Option<ContactDamageJson>,
    knockback_modifiers: KnockbackJson,
    sound_settings: SoundSettingsJson,
}

/// Vanilla `SulfurCubeArchetype.AttributeEntry` JSON (flattened modifier).
#[derive(Deserialize, Debug)]
struct AttributeEntryJson {
    amount: f64,
    attribute: Identifier,
    id: Identifier,
    operation: String,
}

#[derive(Deserialize, Debug)]
struct ExplosionJson {
    power: i32,
    causes_fire: bool,
    fuse: i32,
}

#[derive(Deserialize, Debug)]
struct ContactDamageJson {
    amount: f32,
    attribute_to_source: bool,
    damage_type: Identifier,
}

#[derive(Deserialize, Debug)]
struct KnockbackJson {
    horizontal_power: f32,
    vertical_power: f32,
}

#[derive(Deserialize, Debug)]
struct SoundSettingsJson {
    hit_sound: Identifier,
    push_sound: Identifier,
    push_sound_impulse_threshold: f32,
    push_sound_cooldown: f32,
}

#[derive(Deserialize, Debug)]
struct TagJson {
    values: Vec<String>,
}

fn generate_identifier(identifier: &Identifier) -> TokenStream {
    let namespace = identifier.namespace.as_ref();
    let path = identifier.path.as_ref();
    quote! { Identifier::new_static(#namespace, #path) }
}

/// Reads all tag JSON files recursively and returns a map of tag name -> values.
fn read_all_tags(tag_dir: &Path) -> FxHashMap<String, Vec<String>> {
    fn read_directory(dir: &Path, base: &Path, tags: &mut FxHashMap<String, Vec<String>>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                read_directory(&path, base, tags);
            } else if path.extension().and_then(|s| s.to_str()) == Some("json") {
                let tag_name = path
                    .strip_prefix(base)
                    .unwrap()
                    .with_extension("")
                    .to_str()
                    .unwrap()
                    .replace('\\', "/");
                let content = fs::read_to_string(&path).unwrap();
                let tag: TagJson = serde_json::from_str(&content)
                    .unwrap_or_else(|e| panic!("Failed to parse {tag_name}: {e}"));
                tags.insert(tag_name, tag.values);
            }
        }
    }

    let mut tags = FxHashMap::default();
    if tag_dir.exists() {
        read_directory(tag_dir, tag_dir, &mut tags);
    }
    tags
}

/// Resolves tag references recursively, flattening and deduplicating the result.
fn resolve_tag(
    tag_name: &str,
    all_tags: &FxHashMap<String, Vec<String>>,
    resolved_cache: &mut FxHashMap<String, Vec<String>>,
    visiting: &mut Vec<String>,
) -> Vec<String> {
    if let Some(cached) = resolved_cache.get(tag_name) {
        return cached.clone();
    }
    assert!(
        !visiting.contains(&tag_name.to_string()),
        "Circular tag dependency detected: {visiting:?}"
    );
    visiting.push(tag_name.to_string());

    let values = all_tags
        .get(tag_name)
        .unwrap_or_else(|| panic!("Tag not found: {tag_name}"));

    let mut resolved = Vec::new();
    for value in values {
        if let Some(nested_tag) = value.strip_prefix('#') {
            let nested_tag = nested_tag.strip_prefix("minecraft:").unwrap_or(nested_tag);
            resolved.extend(resolve_tag(nested_tag, all_tags, resolved_cache, visiting));
        } else {
            resolved.push(value.clone());
        }
    }
    visiting.pop();

    let mut seen = FxHashSet::default();
    resolved.retain(|x| seen.insert(x.clone()));
    resolved_cache.insert(tag_name.to_string(), resolved.clone());
    resolved
}

fn generate_operation(operation: &str) -> TokenStream {
    match operation {
        "add_value" => quote! { AttributeModifierOperation::AddValue },
        "add_multiplied_base" => quote! { AttributeModifierOperation::AddMultipliedBase },
        "add_multiplied_total" => quote! { AttributeModifierOperation::AddMultipliedTotal },
        other => panic!("Unknown attribute modifier operation: {other}"),
    }
}

fn generate_attribute_entry(entry: &AttributeEntryJson) -> TokenStream {
    let attribute = generate_identifier(&entry.attribute);
    let id = generate_identifier(&entry.id);
    let amount = entry.amount;
    let operation = generate_operation(&entry.operation);
    quote! {
        SulfurAttributeEntry {
            attribute: #attribute,
            id: #id,
            amount: #amount,
            operation: #operation,
        }
    }
}

pub(crate) fn build() -> TokenStream {
    let archetype_dir =
        "../steel-utils/build_assets/builtin_datapacks/minecraft/sulfur_cube_archetype";
    let item_tag_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/tags/item";
    println!("cargo:rerun-if-changed={archetype_dir}");
    println!("cargo:rerun-if-changed={item_tag_dir}");

    let all_tags = read_all_tags(Path::new(item_tag_dir));
    let mut resolved_cache = FxHashMap::default();

    let mut files = Vec::new();
    for entry in fs::read_dir(archetype_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();

    let mut stream = TokenStream::new();
    let mut register_stream = TokenStream::new();

    for path in &files {
        let key = path.file_stem().unwrap().to_str().unwrap().to_string();
        let content = fs::read_to_string(path).unwrap();
        let archetype: SulfurCubeArchetypeJson =
            serde_json::from_str(&content).unwrap_or_else(|e| panic!("Failed to parse {key}: {e}"));

        let tag = archetype
            .items
            .strip_prefix('#')
            .unwrap_or(&archetype.items);
        let tag_name = tag.strip_prefix("minecraft:").unwrap_or(tag);

        let mut visiting = Vec::new();
        let resolved = resolve_tag(tag_name, &all_tags, &mut resolved_cache, &mut visiting);
        let items = resolved.iter().map(|item| {
            let identifier = item.parse::<Identifier>().unwrap();
            generate_identifier(&identifier)
        });

        let attribute_modifiers = archetype
            .attribute_modifiers
            .iter()
            .map(generate_attribute_entry);

        let buoyant = archetype.buoyant;

        let explosion = match &archetype.explosion {
            Some(explosion) => {
                let power = explosion.power;
                let causes_fire = explosion.causes_fire;
                let fuse = explosion.fuse;
                quote! {
                    Some(SulfurExplosionData {
                        power: #power,
                        causes_fire: #causes_fire,
                        fuse: #fuse,
                    })
                }
            }
            None => quote! { None },
        };

        let contact_damage = match &archetype.contact_damage {
            Some(damage) => {
                let damage_type = generate_identifier(&damage.damage_type);
                let amount = damage.amount;
                let attribute_to_source = damage.attribute_to_source;
                quote! {
                    Some(SulfurContactDamage {
                        damage_type: #damage_type,
                        amount: #amount,
                        attribute_to_source: #attribute_to_source,
                    })
                }
            }
            None => quote! { None },
        };

        let horizontal_power = archetype.knockback_modifiers.horizontal_power;
        let vertical_power = archetype.knockback_modifiers.vertical_power;

        let hit_sound = generate_identifier(&archetype.sound_settings.hit_sound);
        let push_sound = generate_identifier(&archetype.sound_settings.push_sound);
        let push_sound_impulse_threshold = archetype.sound_settings.push_sound_impulse_threshold;
        let push_sound_cooldown = archetype.sound_settings.push_sound_cooldown;

        let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());

        stream.extend(quote! {
            pub static #ident: LazyLock<SulfurCubeArchetype> = LazyLock::new(|| SulfurCubeArchetype {
                key: Identifier::vanilla_static(#key),
                items: vec![#(#items),*],
                attribute_modifiers: vec![#(#attribute_modifiers),*],
                buoyant: #buoyant,
                explosion: #explosion,
                contact_damage: #contact_damage,
                knockback_modifiers: SulfurKnockbackModifiers {
                    horizontal_power: #horizontal_power,
                    vertical_power: #vertical_power,
                },
                sound_settings: SulfurSoundSettings {
                    hit_sound: #hit_sound,
                    push_sound: #push_sound,
                    push_sound_impulse_threshold: #push_sound_impulse_threshold,
                    push_sound_cooldown: #push_sound_cooldown,
                },
            });
        });

        register_stream.extend(quote! {
            registry.register(&#ident);
        });
    }

    let mut out = TokenStream::new();
    out.extend(quote! {
        use std::sync::LazyLock;

        use crate::attribute::AttributeModifierOperation;
        use crate::sulfur_cube_archetype::{
            SulfurAttributeEntry, SulfurContactDamage, SulfurCubeArchetype,
            SulfurCubeArchetypeRegistry, SulfurExplosionData, SulfurKnockbackModifiers,
            SulfurSoundSettings,
        };
        use steel_utils::Identifier;

        #stream

        pub fn register_sulfur_cube_archetypes(registry: &mut SulfurCubeArchetypeRegistry) {
            #register_stream
        }
    });
    out
}
