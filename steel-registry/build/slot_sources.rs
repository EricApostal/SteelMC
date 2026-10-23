//! Build script for the vanilla `slot_source` registry.
//!
//! Vanilla ships no `slot_source` entries in the built-in data pack, so this
//! generator emits no statics. World data packs supply their own slot sources at
//! reload time; nothing is fabricated here.

use proc_macro2::TokenStream;
use quote::quote;

pub(crate) fn build() -> TokenStream {
    let slot_source_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/slot_source";
    println!("cargo:rerun-if-changed={slot_source_dir}");

    quote! {
        use crate::slot_source::SlotSourceRegistry;

        /// Registers the built-in `slot_source` entries.
        ///
        /// Vanilla ships no `slot_source` entries; world data packs supply them at
        /// reload time, so this is intentionally empty.
        pub fn register_slot_sources(_registry: &mut SlotSourceRegistry) {}
    }
}
