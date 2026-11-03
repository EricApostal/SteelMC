//! Inert attributes scanned by `steel-api-codegen`.
//!
//! The attributes exist so the API source compiles; the build script and
//! `steel-plugin-gen` extract the metadata from the same decorated Rust items.

use proc_macro::TokenStream;

#[proc_macro_attribute]
pub fn steel_api(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn steel_method(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn steel_property(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn steel_signal(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn steel_class(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn steel_class_methods(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
