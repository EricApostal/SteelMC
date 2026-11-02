//! Data model for the parsed extension ABI.
//!
//! The JSON artifact is produced directly from [`Api`] with `serde`; the
//! internal type information is skipped during serialization and used by the C
//! and Dart emitters instead.

use std::collections::BTreeMap;

use serde::Serialize;
use syn::{
    GenericArgument, LitStr, Path, PathArguments, ReturnType, Type, TypeFnPtr,
    punctuated::Punctuated,
};

/// A parsed extension ABI.
#[derive(Debug, Clone, Serialize)]
pub struct Api {
    /// Version of the ABI described by this document.
    pub api_version: i64,
    /// Workspace-relative path of the Rust source that was parsed.
    pub source: String,
    /// Numeric `pub const` items whose names start with `STEEL_`.
    pub constants: Vec<ConstantDef>,
    /// `#[repr(C)] pub struct` definitions in declaration order.
    pub structs: Vec<StructDef>,
    /// `pub type ... = unsafe extern "C" fn(...)` declarations.
    pub function_types: Vec<FunctionType>,
    /// Name of the host function table struct.
    pub host_interface: String,
    /// Names of the hook table structs, in declaration order.
    pub hook_tables: Vec<String>,
    /// Exported `#[no_mangle] extern "C"` helper functions.
    #[serde(skip_serializing)]
    pub exports: Vec<ExportFn>,
}

/// A numeric constant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConstantDef {
    /// Constant name.
    pub name: String,
    /// Evaluated integer value.
    pub value: i64,
}

/// A `#[repr(C)]` struct.
#[derive(Debug, Clone, Serialize)]
pub struct StructDef {
    /// Struct name.
    pub name: String,
    /// Fields in declaration order.
    pub fields: Vec<FieldDef>,
}

/// A struct field.
#[derive(Debug, Clone, Serialize)]
pub struct FieldDef {
    /// Field name. Tuple-struct fields use their numeric index (`"0"`).
    pub name: String,
    /// Canonical rendering of the Rust field type.
    pub rust_type: String,
    /// Structured field type used by the emitters.
    #[serde(skip_serializing)]
    pub ty: RustType,
}

/// A function-pointer type alias.
#[derive(Debug, Clone, Serialize)]
pub struct FunctionType {
    /// Alias name.
    pub name: String,
    /// Canonical rendering of every argument type.
    pub args: Vec<String>,
    /// Canonical rendering of the return type (`()` when omitted).
    #[serde(rename = "return")]
    pub return_type: String,
    /// Resolved signature, including aliases of other aliases.
    #[serde(skip_serializing)]
    pub signature: FnSignature,
    /// Immediate alias target when this type aliases another function type.
    #[serde(skip_serializing)]
    pub alias_of: Option<String>,
}

/// An exported C function such as a conformance helper.
#[derive(Debug, Clone)]
pub struct ExportFn {
    /// Exported symbol name.
    pub name: String,
    /// Parsed signature.
    pub signature: FnSignature,
    /// Argument names, falling back to `arg0`, `arg1`, ... when absent.
    pub arg_names: Vec<String>,
}

/// A Rust type restricted to the shapes used by the extension ABI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RustType {
    /// A path type such as `u32`, `usize`, or `SteelWorldRef`.
    Named(String),
    /// `Option<T>`.
    Option(Box<RustType>),
    /// A raw pointer.
    Pointer {
        /// Whether the pointer is mutable (`*mut`).
        mutable: bool,
        /// Pointee type.
        elem: Box<RustType>,
    },
    /// A function pointer type.
    FnPtr(Box<FnSignature>),
    /// A tuple type.
    Tuple(Vec<RustType>),
    /// The unit type `()`.
    Unit,
    /// A type the parser does not model structurally.
    Unknown(String),
}

impl RustType {
    /// Parses a `syn` type into the restricted ABI type model.
    pub fn from_syn(ty: &Type) -> Self {
        match ty {
            Type::Path(path) => Self::from_path(path),
            Type::Ptr(ptr) => Self::Pointer {
                mutable: matches!(ptr.mutability, syn::PointerMutability::Mut(_)),
                elem: Box::new(Self::from_syn(&ptr.elem)),
            },
            Type::FnPtr(fn_ptr) => Self::FnPtr(Box::new(FnSignature::from_type_fn_ptr(fn_ptr))),
            Type::Paren(paren) => Self::from_syn(&paren.elem),
            Type::Group(group) => Self::from_syn(&group.elem),
            Type::Tuple(tuple) if tuple.elems.is_empty() => Self::Unit,
            Type::Tuple(tuple) => Self::Tuple(tuple.elems.iter().map(Self::from_syn).collect()),
            other => Self::Unknown(render_tokens(other)),
        }
    }

    fn from_path(path: &syn::TypePath) -> Self {
        if let Some(segment) = path.path.segments.last()
            && segment.ident == "Option"
            && let PathArguments::AngleBracketed(arguments) = &segment.arguments
            && let Some(GenericArgument::Type(inner)) = arguments.args.first()
        {
            return Self::Option(Box::new(Self::from_syn(inner)));
        }
        Self::Named(render_path(&path.path))
    }

    /// Canonical, deterministic rendering of the type.
    pub fn render(&self) -> String {
        match self {
            Self::Named(name) | Self::Unknown(name) => name.clone(),
            Self::Option(inner) => format!("Option<{}>", inner.render()),
            Self::Pointer { mutable, elem } => {
                let qualifier = if *mutable { "mut" } else { "const" };
                format!("*{qualifier} {}", elem.render())
            }
            Self::FnPtr(signature) => signature.render(),
            Self::Tuple(fields) => {
                let rendered: Vec<_> = fields.iter().map(Self::render).collect();
                format!("({})", rendered.join(", "))
            }
            Self::Unit => "()".to_owned(),
        }
    }

    /// Returns the final path segment when this is a named type.
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Named(name) => Some(name),
            _ => None,
        }
    }

    /// Returns whether this is the opaque `c_void` type.
    pub fn is_c_void(&self) -> bool {
        matches!(self, Self::Named(name) if name.rsplit("::").next() == Some("c_void"))
    }
}

/// A C-compatible function signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FnSignature {
    /// Argument types.
    pub inputs: Vec<RustType>,
    /// Return type; `None` means `()`.
    pub output: Option<RustType>,
    /// Whether the function pointer is `unsafe`.
    pub unsafety: bool,
    /// ABI string, normally `"C"`.
    pub abi: Option<String>,
}

impl FnSignature {
    /// Parses a bare function pointer type.
    pub fn from_type_fn_ptr(fn_ptr: &TypeFnPtr) -> Self {
        Self {
            inputs: fn_ptr
                .inputs
                .iter()
                .map(|argument| RustType::from_syn(&argument.ty))
                .collect(),
            output: return_type(&fn_ptr.output),
            unsafety: fn_ptr.unsafety.is_some(),
            abi: fn_ptr.abi.as_ref().and_then(abi_name),
        }
    }

    /// Parses a Rust function signature.
    pub fn from_signature(signature: &syn::Signature) -> Self {
        Self {
            inputs: signature
                .inputs
                .iter()
                .filter_map(|argument| match argument {
                    syn::FnArg::Typed(typed) => Some(RustType::from_syn(&typed.ty)),
                    syn::FnArg::Receiver(_) => None,
                })
                .collect(),
            output: return_type(&signature.output),
            unsafety: matches!(signature.safety, syn::Safety::Unsafe(_)),
            abi: signature.abi.as_ref().and_then(abi_name),
        }
    }

    /// Canonical rendering of the function pointer type.
    pub fn render(&self) -> String {
        let unsafety = if self.unsafety { "unsafe " } else { "" };
        let abi = self
            .abi
            .as_ref()
            .map_or_else(String::new, |abi| format!("extern \"{abi}\" "));
        let arguments: Vec<_> = self.inputs.iter().map(RustType::render).collect();
        let output = self
            .output
            .as_ref()
            .map_or_else(String::new, |output| format!(" -> {}", output.render()));
        format!("{unsafety}{abi}fn({}){output}", arguments.join(", "))
    }

    /// Returns whether this signature is declared as an `extern "C"` function.
    pub fn is_c_abi(&self) -> bool {
        self.abi.as_deref() == Some("C")
    }
}

fn return_type(output: &ReturnType) -> Option<RustType> {
    match output {
        ReturnType::Default => None,
        ReturnType::Type(_, ty) => Some(RustType::from_syn(ty)),
    }
}

fn abi_name(abi: &syn::Abi) -> Option<String> {
    abi.name.as_ref().map(LitStr::value)
}

fn render_path(path: &Path) -> String {
    let segments: Vec<_> = path
        .segments
        .iter()
        .map(|segment| {
            let ident = segment.ident.to_string();
            match &segment.arguments {
                PathArguments::None => ident,
                PathArguments::AngleBracketed(arguments) => {
                    let arguments = render_generic_arguments(&arguments.args);
                    format!("{ident}<{arguments}>")
                }
                PathArguments::Parenthesized(arguments) => {
                    let inputs: Vec<_> = arguments
                        .inputs
                        .iter()
                        .map(|argument| RustType::from_syn(&argument.ty).render())
                        .collect();
                    format!("{ident}({})", inputs.join(", "))
                }
            }
        })
        .collect();
    segments.join("::")
}

fn render_generic_arguments(arguments: &Punctuated<GenericArgument, syn::Token![,]>) -> String {
    let arguments: Vec<_> = arguments
        .iter()
        .map(|argument| match argument {
            GenericArgument::Type(ty) => RustType::from_syn(ty).render(),
            other => render_tokens(other),
        })
        .collect();
    arguments.join(", ")
}

fn render_tokens<T: quote::ToTokens>(value: &T) -> String {
    quote::quote!(#value).to_string().replace(" :: ", "::")
}

/// A stable synthetic typedef name for an anonymous function pointer type.
pub fn synthetic_fn_name(signature: &FnSignature) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in signature.render().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("SteelFnPtr_{hash:016x}")
}

/// Collects every anonymous function pointer type that needs a synthetic
/// typedef, keyed by the deterministic typedef name.
pub fn collect_synthetic_fn_types(api: &Api) -> BTreeMap<String, FnSignature> {
    fn walk(ty: &RustType, out: &mut BTreeMap<String, FnSignature>) {
        match ty {
            RustType::FnPtr(signature) => {
                out.entry(synthetic_fn_name(signature))
                    .or_insert_with(|| signature.as_ref().clone());
                walk_signature(signature, out);
            }
            RustType::Option(inner) => walk(inner, out),
            RustType::Pointer { elem, .. } => walk(elem, out),
            RustType::Tuple(fields) => {
                for field in fields {
                    walk(field, out);
                }
            }
            RustType::Named(_) | RustType::Unit | RustType::Unknown(_) => {}
        }
    }

    fn walk_signature(signature: &FnSignature, out: &mut BTreeMap<String, FnSignature>) {
        for input in &signature.inputs {
            walk(input, out);
        }
        if let Some(output) = &signature.output {
            walk(output, out);
        }
    }

    let mut out = BTreeMap::new();
    for function in &api.function_types {
        walk_signature(&function.signature, &mut out);
    }
    for definition in &api.structs {
        for field in &definition.fields {
            walk(&field.ty, &mut out);
        }
    }
    out
}

/// Orders function typedefs so that alias targets are emitted before their
/// aliases while otherwise preserving declaration order.
pub fn function_types_in_dependency_order(api: &Api) -> Vec<&FunctionType> {
    fn visit<'a>(
        index: usize,
        api: &'a Api,
        by_name: &BTreeMap<&'a str, usize>,
        state: &mut [u8],
        out: &mut Vec<&'a FunctionType>,
    ) {
        if state[index] != 0 {
            return;
        }
        state[index] = 1;
        if let Some(target) = &api.function_types[index].alias_of
            && let Some(&target_index) = by_name.get(target.as_str())
            && target_index != index
        {
            visit(target_index, api, by_name, state, out);
        }
        state[index] = 2;
        out.push(&api.function_types[index]);
    }

    let by_name: BTreeMap<_, _> = api
        .function_types
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.as_str(), index))
        .collect();
    let mut state = vec![0_u8; api.function_types.len()];
    let mut out = Vec::with_capacity(api.function_types.len());
    for index in 0..api.function_types.len() {
        visit(index, api, &by_name, &mut state, &mut out);
    }
    out
}
