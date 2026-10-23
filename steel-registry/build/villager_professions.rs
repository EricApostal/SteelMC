use crate::generator_functions::{
    generate_identifier, generate_option, generate_sound_event_ref, read_json_asset,
    sort_contiguous_registry_entries,
};
use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use steel_utils::Identifier;

#[derive(Deserialize)]
struct VillagerProfessionEntry {
    id: usize,
    key: Identifier,
    work_sound: Option<Identifier>,
    /// Vanilla `VillagerProfession.requestedItems`.
    ///
    /// Vanilla hard-codes these in its bootstrap; the vendored extractor output
    /// does not carry them, so only professions that declare a set list them here
    /// (currently just `farmer`). Replace with extractor output once
    /// `SteelExtractor` covers `VillagerProfessions`.
    #[serde(default)]
    requested_items: Option<Vec<Identifier>>,
    /// Vanilla `VillagerProfession.secondaryPoi`; same provenance note as above.
    #[serde(default)]
    secondary_poi: Option<Vec<Identifier>>,
}

#[derive(Deserialize)]
struct SoundEventEntry {
    key: Identifier,
}

#[derive(Deserialize)]
struct PoiTypeEntry {
    name: String,
}

pub(crate) fn build() -> TokenStream {
    const ASSET: &str = "build_assets/villager_professions.json";

    let mut villager_professions: Vec<VillagerProfessionEntry> = read_json_asset(ASSET);
    sort_contiguous_registry_entries(&mut villager_professions, ASSET, |entry| entry.id);
    let sound_events: Vec<SoundEventEntry> = read_json_asset("build_assets/sound_events.json");
    let sound_event_keys: BTreeSet<String> = sound_events
        .into_iter()
        .map(|entry| entry.key.to_string())
        .collect();

    // Vanilla registers each job-site profession with the `PoiTypes` entry of the
    // same name (e.g. `FARMER -> PoiTypes.FARMER`), so the held job site is
    // derived from the POI types that share the profession's key.
    #[derive(Deserialize)]
    struct PoiTypesAsset {
        poi_types: Vec<PoiTypeEntry>,
    }
    let poi_type_names: BTreeSet<String> = {
        let asset: PoiTypesAsset = read_json_asset("build_assets/poi_types.json");
        asset.poi_types.into_iter().map(|poi| poi.name).collect()
    };

    // Vanilla's `VillagerProfession.tradeSetsByLevel` is code-only, but every
    // referenced trade set exists as a `trade_set/<profession>/level_<n>` registry
    // entry, so the mapping is derived from those keys.
    let trade_sets = crate::trade_sets::read_trade_set_keys();
    let mut trade_sets_by_profession: BTreeMap<String, Vec<(i32, String)>> = BTreeMap::new();
    for key in &trade_sets {
        let Some((profession, level)) = key.split_once("/level_") else {
            continue;
        };
        let level: i32 = level
            .parse()
            .unwrap_or_else(|error| panic!("trade set {key} has an invalid level: {error}"));
        trade_sets_by_profession
            .entry(profession.to_string())
            .or_default()
            .push((level, key.clone()));
    }

    let mut constants = TokenStream::new();
    let mut registrations = TokenStream::new();

    for villager_profession in &villager_professions {
        let ident = Ident::new(
            &villager_profession.key.path.to_shouty_snake_case(),
            Span::call_site(),
        );
        let key = generate_identifier(&villager_profession.key);
        if let Some(work_sound) = &villager_profession.work_sound {
            assert!(
                sound_event_keys.contains(&work_sound.to_string()),
                "Villager profession {} references missing work sound {}",
                villager_profession.key,
                work_sound
            );
        }
        let work_sound = generate_option(&villager_profession.work_sound, generate_sound_event_ref);

        let held_job_site = if poi_type_names.contains(villager_profession.key.path.as_ref()) {
            let poi_ident = Ident::new(
                &villager_profession.key.path.to_shouty_snake_case(),
                Span::call_site(),
            );
            quote! { Some(&vanilla_poi_types::#poi_ident) }
        } else {
            quote! { None }
        };

        let mut levels: Vec<(i32, String)> = trade_sets_by_profession
            .remove(villager_profession.key.path.as_ref())
            .unwrap_or_default();
        levels.sort_by_key(|(level, _)| *level);

        let trade_sets: Vec<TokenStream> = levels
            .iter()
            .map(|(level, key)| {
                let ident = Ident::new(&key.to_shouty_snake_case(), Span::call_site());
                quote! { (#level, &*vanilla_trade_sets::#ident) }
            })
            .collect();

        let trade_sets_ident = Ident::new(
            &format!(
                "{}_TRADE_SETS",
                villager_profession.key.path.to_shouty_snake_case()
            ),
            Span::call_site(),
        );

        constants.extend(quote! {
            static #trade_sets_ident: LazyLock<Vec<(i32, &'static crate::trade::TradeSet)>> =
                LazyLock::new(|| vec![#(#trade_sets),*]);

            pub static #ident: LazyLock<VillagerProfession> = LazyLock::new(|| VillagerProfession {
                key: #key,
                work_sound: #work_sound,
                held_job_site: #held_job_site,
                requested_items: &[],
                secondary_poi: &[],
                trade_sets_by_level: #trade_sets_ident.as_slice(),
            });
        });

        registrations.extend(quote! {
            registry.register(&#ident);
        });
    }

    assert!(
        trade_sets_by_profession.is_empty(),
        "trade sets reference unknown villager professions: {:?}",
        trade_sets_by_profession.keys().collect::<Vec<_>>()
    );

    quote! {
        use crate::villager_profession::{VillagerProfession, VillagerProfessionRegistry};
        use crate::vanilla_poi_types;
        use crate::vanilla_trade_sets;
        use std::borrow::Cow;
        use std::sync::LazyLock;
        use steel_utils::Identifier;

        #constants

        pub fn register_villager_professions(registry: &mut VillagerProfessionRegistry) {
            #registrations
        }
    }
}
