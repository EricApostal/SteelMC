//! Generators for the vanilla brewing recipe serializer.

use proc_macro2::TokenStream;
use quote::quote;
use serde_json::Value;

use super::shared::{field, ingredient_tokens, result_tokens, vanilla_ident};

pub(super) fn generate(value: &Value) -> TokenStream {
    let input = potion_ingredient_tokens(field(value, "input"));
    let reagent = potion_ingredient_tokens(field(value, "reagent"));
    let output = result_tokens(field(value, "output"));
    quote! {
        BrewingRecipe {
            properties: RecipeProperties::special(),
            input: #input,
            reagent: #reagent,
            output: #output,
        }
    }
}

/// Generates a `PotionIngredient` from an extracted `item` + optional
/// `potion_contents.potions` pair.
fn potion_ingredient_tokens(value: &Value) -> TokenStream {
    let ingredient = ingredient_tokens(field(value, "item"));
    let potions = match value.get("potion_contents") {
        None => quote! { None },
        Some(contents) => {
            let values = match field(contents, "potions") {
                Value::String(single) => {
                    if let Some(tag) = single.strip_prefix('#') {
                        let identifier = identifier_tokens(tag);
                        quote! { crate::RegistryHolderSet::Tag(#identifier) }
                    } else {
                        let potion = vanilla_ident(single);
                        quote! { crate::RegistryHolderSet::Direct(vec![&vanilla_potions::#potion]) }
                    }
                }
                Value::Array(list) => {
                    let potions: Vec<_> = list
                        .iter()
                        .map(|entry| {
                            let potion = entry.as_str().unwrap_or_else(|| {
                                panic!("Brewing potion entry is not a string: {entry}")
                            });
                            let potion = vanilla_ident(potion);
                            quote! { &vanilla_potions::#potion }
                        })
                        .collect();
                    quote! { crate::RegistryHolderSet::Direct(vec![#(#potions),*]) }
                }
                other => panic!("Unsupported extracted brewing potion set {other}"),
            };
            quote! {
                Some(crate::data_component_predicate::PotionsPredicate::new(#values))
            }
        }
    };
    quote! {
        PotionIngredient {
            ingredient: #ingredient,
            potions: #potions,
        }
    }
}

fn identifier_tokens(identifier: &str) -> TokenStream {
    match identifier.split_once(':') {
        Some(("minecraft", path)) => quote! { Identifier::vanilla_static(#path) },
        Some((namespace, path)) => quote! { Identifier::new_static(#namespace, #path) },
        None => quote! { Identifier::vanilla_static(#identifier) },
    }
}
