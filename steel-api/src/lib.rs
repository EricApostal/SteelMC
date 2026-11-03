//! Annotated host API and generated MethodBind metadata.

pub mod api;
pub mod classdb;
mod from_ref;
pub mod host;

pub use from_ref::FromRef;

/// Class metadata used for MethodBind inheritance lookup.
#[derive(Clone, Copy)]
pub struct ClassDesc {
    pub name: &'static str,
    pub inherits: Option<&'static str>,
    pub handle: &'static str,
}

include!(concat!(env!("OUT_DIR"), "/method_binds.rs"));
