//! Code generation for item behaviors.
//!
//! Scans `src/behavior/items/**/*.rs` for structs annotated with `#[item_behavior]`,
//! cross-references with `classes.json`, and generates `register_item_behaviors()`.

use crate::common::{self, JsonArgKind, scan_object_behaviors};
use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span};
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Deserialize)]
pub struct ItemClass {
    pub name: String,
    pub class: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Indexes behaviors that key on item names instead of a vanilla class, so an
/// item with no class of its own (the spears are plain `Item` built by
/// `Item.Properties.spear`) can still register a behavior.
fn index_behaviors_by_item_name(
    discovered: &HashMap<String, common::DiscoveredObject>,
) -> HashMap<String, &common::DiscoveredObject> {
    let mut by_item_name = HashMap::new();
    for info in discovered.values() {
        for name in &info.item_names {
            assert!(
                by_item_name.insert(name.clone(), info).is_none(),
                "Two item behaviors are registered for '{name}'"
            );
        }
    }
    by_item_name
}

pub fn build(items: &[ItemClass]) -> String {
    let discovered = scan_object_behaviors("items", "item_behavior");

    let mut enum_imports: BTreeMap<String, String> = BTreeMap::new();
    let mut registry_modules_used: BTreeSet<String> = BTreeSet::new();
    let mut registrations = Vec::new();
    let mut matched_classes = BTreeSet::new();
    let mut matched_item_names = BTreeSet::new();

    // Item-name keying for behaviors with no vanilla class of their own.
    let by_item_name = index_behaviors_by_item_name(&discovered);

    for item in items {
        let info = if let Some(info) = discovered.get(&item.class) {
            matched_classes.insert(&item.class);
            info
        } else if let Some(info) = by_item_name.get(&item.name) {
            matched_item_names.insert(&item.name);
            info
        } else {
            continue;
        };

        let struct_ident = Ident::new(&info.struct_name, Span::call_site());
        let item_field = Ident::new(&item.name.to_shouty_snake_case(), Span::call_site());

        for field in &info.fields {
            match &field.kind {
                JsonArgKind::Enum {
                    type_name,
                    module_path,
                } => {
                    if let Some(path) = module_path {
                        enum_imports.insert(type_name.clone(), path.clone());
                    }
                }
                JsonArgKind::Registry(module) => {
                    registry_modules_used.insert(module.clone());
                }
                JsonArgKind::Value | JsonArgKind::IntProvider => {}
            }
        }

        // Need to divide here into two cases because blocks always have a block property while items don't have that.
        let registration = if info.fields.is_empty() {
            // Unit struct or struct with no json_args — instantiate directly
            quote! {
                registry.set_behavior(
                    &*vanilla_items::#item_field,
                    Box::new(#struct_ident),
                );
            }
        } else {
            let mut args = Vec::new();
            for field in &info.fields {
                args.push(common::generate_arg(field, &item.extra, &item.name));
            }

            quote! {
                registry.set_behavior(
                    &*vanilla_items::#item_field,
                    Box::new(#struct_ident::new(#(#args),*)),
                );
            }
        };

        registrations.push(registration);
    }

    for (class_name, info) in &discovered {
        if info.item_names.is_empty() {
            assert!(
                matched_classes.contains(class_name),
                "Item behavior struct `{}` maps to class '{}' which doesn't exist in classes.json",
                info.struct_name,
                class_name
            );
        } else {
            for name in &info.item_names {
                assert!(
                    matched_item_names.contains(name),
                    "Item behavior struct `{}` names item '{name}' which doesn't exist in classes.json",
                    info.struct_name,
                );
            }
        }
    }

    let enum_import_tokens: Vec<_> = enum_imports
        .iter()
        .map(|(type_name, module_path)| {
            let type_ident = Ident::new(type_name, Span::call_site());
            let path: syn::Path = syn::parse_str(module_path).unwrap_or_else(|_| {
                panic!("Invalid module path '{module_path}' for enum '{type_name}'")
            });
            quote! { use #path::#type_ident; }
        })
        .collect();

    let registry_import_tokens: Vec<_> = registry_modules_used
        .iter()
        .filter(|module| module.as_str() != "vanilla_items")
        .map(|module| {
            let module_ident = Ident::new(module, Span::call_site());
            quote! { , #module_ident }
        })
        .collect();

    let output = quote! {
        //! Generated item behavior assignments.

        use steel_registry::{vanilla_items #(#registry_import_tokens)*};
        use crate::behavior::ItemBehaviorRegistry;
        #[expect(
            clippy::wildcard_imports,
            reason = "the registry intentionally imports every item behavior implementation"
        )]
        use crate::behavior::items::*;
        #(#enum_import_tokens)*

        pub fn register_item_behaviors(registry: &mut ItemBehaviorRegistry) {
            #(#registrations)*
        }
    };

    output.to_string()
}
