//! Shared build-time helpers for the villager trade registries.
//!
//! Reads vendored datapack registries and tags, applying datapack overlays with
//! vanilla's `replace` semantics, and resolves tag references against the
//! extracted vanilla registry statics.

use heck::ToShoutySnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use steel_utils::Identifier;

/// A tag file, including vanilla's `replace` flag.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TagFileJson {
    #[serde(default)]
    replace: bool,
    values: Vec<String>,
}

/// Reads every `<registry>/**/*.json` in a vendored datapack directory.
///
/// Returns `(path, json)` pairs, where `path` is the resource path inside the
/// namespace (e.g. `farmer/1/wheat_emerald`).
pub(crate) fn read_registry_dir(dir: &str, registry: &str) -> Vec<(String, String)> {
    let base = Path::new(dir);
    let mut entries = Vec::new();
    if !base.exists() {
        return entries;
    }
    read_dir_recursive(base, base, registry, &mut entries);
    entries.sort();
    entries
}

fn read_dir_recursive(
    base: &Path,
    dir: &Path,
    registry: &str,
    entries: &mut Vec<(String, String)>,
) {
    let mut children: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|error| {
            panic!(
                "Cannot read {registry} directory {}: {error}",
                dir.display()
            )
        })
        .map(|entry| entry.expect("directory entry").path())
        .collect();
    children.sort();
    for path in children {
        if path.is_dir() {
            read_dir_recursive(base, &path, registry, entries);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("json") {
            continue;
        }
        let key = path
            .strip_prefix(base)
            .expect("entry inside the registry directory")
            .with_extension("")
            .to_str()
            .expect("utf-8 registry path")
            .replace('\\', "/");
        let json = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("Cannot read {}: {error}", path.display()));
        entries.push((key, json));
    }
}

/// Reads a tag directory, then applies overlay tag directories in order,
/// honouring vanilla's `replace` flag.
pub(crate) fn read_tags(
    base_dir: &str,
    overlay_dirs: Option<&[&str]>,
    label: &str,
) -> BTreeMap<String, Vec<String>> {
    let mut tags: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut dirs = vec![base_dir];
    dirs.extend(overlay_dirs.unwrap_or_default());
    for dir in dirs {
        for (name, json) in read_registry_dir(dir, label) {
            let tag: TagFileJson = serde_json::from_str(&json)
                .unwrap_or_else(|error| panic!("Failed to parse {label} tag {name}: {error}"));
            if tag.replace {
                tags.insert(name, tag.values);
            } else {
                tags.entry(name).or_default().extend(tag.values);
            }
        }
    }
    tags
}

/// Resolves a tag into the ordered, de-duplicated list of referenced entry paths.
pub(crate) fn resolve_tag(
    name: &str,
    tags: &BTreeMap<String, Vec<String>>,
    label: &str,
) -> Vec<String> {
    let mut resolved = Vec::new();
    let mut visiting = Vec::new();
    resolve_tag_into(name, tags, label, &mut resolved, &mut visiting);

    let mut seen = BTreeSet::new();
    resolved.retain(|entry| seen.insert(entry.clone()));
    resolved
}

fn resolve_tag_into(
    name: &str,
    tags: &BTreeMap<String, Vec<String>>,
    label: &str,
    resolved: &mut Vec<String>,
    visiting: &mut Vec<String>,
) {
    assert!(
        !visiting.iter().any(|visiting| visiting == name),
        "circular {label} tag reference: {visiting:?} -> {name}"
    );
    let values = tags
        .get(name)
        .unwrap_or_else(|| panic!("{label} tag not found: {name}"));
    visiting.push(name.to_string());
    for value in values {
        if let Some(nested) = value.strip_prefix('#') {
            let nested = nested.strip_prefix("minecraft:").unwrap_or(nested);
            resolve_tag_into(nested, tags, label, resolved, visiting);
        } else {
            let entry = value.strip_prefix("minecraft:").unwrap_or(value);
            resolved.push(entry.to_string());
        }
    }
    visiting.pop();
}

/// Vanilla item paths from the extracted item registry.
pub(crate) fn item_names() -> BTreeSet<String> {
    #[derive(Deserialize)]
    struct ItemsJson {
        items: Vec<ItemJson>,
    }

    #[derive(Deserialize)]
    struct ItemJson {
        name: String,
    }

    let items: ItemsJson = serde_json::from_str(
        &fs::read_to_string("build_assets/items.json").expect("missing extracted item registry"),
    )
    .expect("invalid extracted item registry");
    items.items.into_iter().map(|item| item.name).collect()
}

/// Vanilla enchantment paths (the `enchantment` datapack file stems).
pub(crate) fn enchantment_names() -> BTreeSet<String> {
    let dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/enchantment";
    fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("Cannot read enchantment directory: {error}"))
        .map(|entry| {
            entry
                .expect("directory entry")
                .path()
                .file_stem()
                .expect("enchantment file stem")
                .to_str()
                .expect("utf-8 enchantment name")
                .to_string()
        })
        .collect()
}

/// Vanilla villager type paths from the extracted villager type registry.
pub(crate) fn villager_type_statics() -> BTreeSet<String> {
    #[derive(Deserialize)]
    struct VillagerTypeJson {
        key: Identifier,
    }

    let types: Vec<VillagerTypeJson> = serde_json::from_str(
        &fs::read_to_string("build_assets/villager_types.json")
            .expect("missing extracted villager type registry"),
    )
    .expect("invalid extracted villager type registry");
    types
        .into_iter()
        .map(|villager_type| villager_type.key.path.as_ref().to_string())
        .collect()
}

/// Emits an `ItemRef` static for a vanilla item id.
pub(crate) fn generate_item_ref(
    id: &Identifier,
    context: &str,
    item_names: &BTreeSet<String>,
) -> TokenStream {
    assert_eq!(
        id.namespace.as_ref(),
        "minecraft",
        "{context} references a non-vanilla item: {id}"
    );
    assert!(
        item_names.contains(id.path.as_ref()),
        "{context} references unknown item {id}"
    );
    let ident = proc_macro2::Ident::new(
        &id.path.to_shouty_snake_case(),
        proc_macro2::Span::call_site(),
    );
    quote! { &*crate::vanilla_items::#ident }
}
