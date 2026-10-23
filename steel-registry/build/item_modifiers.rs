#![expect(
    clippy::unwrap_used,
    reason = "build script must fail immediately on invalid extracted item modifier data"
)]

//! Build script for the vanilla `item_modifier` registry.
//!
//! Vanilla ships no `item_modifier` entries in the built-in data pack, so this
//! generator emits no statics. World data packs supply their own modifiers at
//! reload time; nothing is fabricated here.

use proc_macro2::TokenStream;
use quote::quote;

pub(crate) fn build() -> TokenStream {
    let item_modifier_dir = "../steel-utils/build_assets/builtin_datapacks/minecraft/item_modifier";
    println!("cargo:rerun-if-changed={item_modifier_dir}");

    quote! {
        use crate::item_modifier::ItemModifierRegistry;

        /// Registers the built-in `item_modifier` entries.
        ///
        /// Vanilla ships no `item_modifier` entries; world data packs supply them
        /// at reload time, so this is intentionally empty.
        pub fn register_item_modifiers(_registry: &mut ItemModifierRegistry) {}
    }
}
