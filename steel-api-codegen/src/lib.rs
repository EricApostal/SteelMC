//! Scanner and code generator for the annotated Steel host API.
//!
//! The Rust API source is the single source of truth. This crate extracts
//! classes/methods, emits `steel_api.json` for language bindings, and emits the
//! `MethodBind` thunks used by the host ClassDB.

use std::fs;
use std::path::Path;

use quote::ToTokens;
use serde::Serialize;
use syn::{Attribute, ImplItem, Item, LitStr, ReturnType, Type};

/// A parsed host API.
#[derive(Debug, Clone, Serialize)]
pub struct ApiModel {
    pub api_version: u32,
    pub classes: Vec<Class>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Class {
    pub name: String,
    #[serde(skip)]
    pub rust_name: String,
    pub inherits: Option<String>,
    pub handle: String,
    pub receiver: String,
    pub methods: Vec<Method>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Method {
    pub name: String,
    pub hash: u32,
    #[serde(rename = "static")]
    pub is_static: bool,
    pub args: Vec<Argument>,
    #[serde(rename = "return", skip_serializing_if = "Option::is_none")]
    pub returns: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub returns_class: Option<String>,
    #[serde(skip)]
    pub rust_return: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Argument {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    #[serde(skip)]
    pub rust_type: String,
}

/// Parses one annotated API source file.
pub fn scan_file(path: &Path) -> Result<ApiModel, String> {
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    scan_source(&source)
}

/// Parses annotated API source.
pub fn scan_source(source: &str) -> Result<ApiModel, String> {
    let file = syn::parse_file(source).map_err(|error| format!("invalid API source: {error}"))?;
    let mut classes: Vec<Class> = Vec::new();
    for item in &file.items {
        let Item::Struct(item_struct) = item else {
            continue;
        };
        let Some(attr) = item_struct
            .attrs
            .iter()
            .find(|attr| attr.path().is_ident("steel_api"))
        else {
            continue;
        };
        let meta = ClassMeta::parse(attr)?;
        classes.push(Class {
            name: meta.name,
            rust_name: item_struct.ident.to_string(),
            inherits: meta.inherits,
            handle: meta.handle,
            receiver: meta.receiver,
            methods: Vec::new(),
        });
    }
    for item in &file.items {
        let Item::Impl(item_impl) = item else {
            continue;
        };
        let Type::Path(type_path) = &*item_impl.self_ty else {
            continue;
        };
        let Some(segment) = type_path.path.segments.last() else {
            continue;
        };
        let type_name = segment.ident.to_string();
        let Some(class) = classes
            .iter_mut()
            .find(|class| class.rust_name == type_name)
        else {
            continue;
        };
        for item in &item_impl.items {
            let ImplItem::Fn(function) = item else {
                continue;
            };
            let Some(attr) = function
                .attrs
                .iter()
                .find(|attr| attr.path().is_ident("steel_method"))
            else {
                continue;
            };
            let method_meta = MethodMeta::parse(attr)?;
            let mut args = Vec::new();
            for input in &function.sig.inputs {
                let syn::FnArg::Typed(typed) = input else {
                    continue;
                };
                let name = match typed.pat.as_ref() {
                    syn::Pat::Ident(ident) => ident.ident.to_string(),
                    _ => format!("arg{}", args.len()),
                };
                let rust_type = render_type(&typed.ty);
                let ty = api_type(&rust_type)?;
                args.push(Argument {
                    name,
                    ty,
                    rust_type,
                });
            }
            if method_meta.is_static {
                // Static methods have no receiver; the class handle carries it.
            } else if args.is_empty() {
                return Err(format!(
                    "method {}.{} needs a receiver argument",
                    class.name, function.sig.ident
                ));
            } else {
                let receiver = args.remove(0);
                if receiver.rust_type != class.receiver {
                    return Err(format!(
                        "method {}.{} receiver is {}, expected {}",
                        class.name, function.sig.ident, receiver.rust_type, class.receiver
                    ));
                }
            }
            let rust_return = match &function.sig.output {
                ReturnType::Default => None,
                ReturnType::Type(_, ty) => Some(render_type(ty)),
            };
            let returns = rust_return.as_deref().map(api_type).transpose()?;
            let name = function.sig.ident.to_string();
            let hash = method_hash(
                &class.name,
                &name,
                args.iter().map(|argument| argument.ty.as_str()),
                returns.as_deref(),
            );
            class.methods.push(Method {
                name,
                hash,
                is_static: method_meta.is_static,
                args,
                returns,
                returns_class: method_meta.returns_class,
                rust_return,
            });
        }
    }
    Ok(ApiModel {
        api_version: 1,
        classes,
    })
}

/// Serializes the API model as pretty JSON.
pub fn emit_json(model: &ApiModel) -> Result<String, String> {
    let mut json = serde_json::to_string_pretty(model)
        .map_err(|error| format!("failed to serialize API JSON: {error}"))?;
    json.push('\n');
    Ok(json)
}

/// Emits the Rust `MethodBind` thunks and metadata.
#[must_use]
pub fn emit_method_binds(model: &ApiModel) -> String {
    let mut output = String::from(
        "// Generated by steel-api-codegen. Do not edit.\n\nuse core::ffi::c_void;\nuse steel_abi::*;\nuse crate::from_ref::*;\n\n",
    );
    for class in &model.classes {
        for method in &class.methods {
            output.push_str(&emit_thunk(class, method));
            output.push('\n');
        }
    }
    output.push_str(
        "/// Native host method binds.\npub static METHOD_BINDS: &[SteelMethodBindDesc] = &[\n",
    );
    for class in &model.classes {
        for method in &class.methods {
            output.push_str(&format!(
                "    SteelMethodBindDesc {{ class_name: SteelStrView::from_str({:?}), method_name: SteelStrView::from_str({:?}), hash: {}, thunk: Some(thunk_{}_{}), user_data: core::ptr::null_mut() }},\n",
                class.name, method.name, method.hash, class.name, method.name
            ));
        }
    }
    output.push_str("];\n\n");
    output.push_str("/// Native class metadata used for inheritance lookup.\npub static CLASSES: &[crate::ClassDesc] = &[\n");
    for class in &model.classes {
        output.push_str(&format!(
            "    crate::ClassDesc {{ name: {:?}, inherits: {:?}, handle: {:?} }},\n",
            class.name, class.inherits, class.handle
        ));
    }
    output.push_str("];\n");
    output
}

fn emit_thunk(class: &Class, method: &Method) -> String {
    let thunk_name = format!("thunk_{}_{}", class.name, method.name);
    let mut output = format!(
        "unsafe extern \"C\" fn {thunk_name}(\n    _user_data: *mut c_void,\n    instance: SteelRef,\n    args: *const *const c_void,\n    ret: *mut c_void,\n) {{\n"
    );
    if !method.is_static {
        output.push_str(&format!(
            "    let Some(receiver) = <{} as crate::FromRef>::from_ref(instance) else {{ return; }};\n",
            class.receiver
        ));
    }
    for (index, argument) in method.args.iter().enumerate() {
        output.push_str(&format!(
            "    let arg{index} = unsafe {{ *((*(args.add({index}))) as *const {}) }};\n",
            argument.rust_type
        ));
    }
    let receiver = if method.is_static {
        String::new()
    } else {
        "receiver".to_owned()
    };
    let mut arguments = Vec::new();
    if !receiver.is_empty() {
        arguments.push(receiver);
    }
    arguments.extend((0..method.args.len()).map(|index| format!("arg{index}")));
    let call = format!(
        "crate::api::{}::{}({})",
        class.rust_name,
        method.name,
        arguments.join(", ")
    );
    match method.rust_return.as_deref() {
        None | Some("()") => output.push_str(&format!("    let _ = {call};\n")),
        Some(return_type) => {
            output.push_str(&format!("    let result = {call};\n"));
            output.push_str(&format!(
                "    if !ret.is_null() {{ unsafe {{ *(ret as *mut {return_type}) = result; }} }}\n"
            ));
        }
    }
    output.push_str("}\n");
    output
}

fn method_hash<'a>(
    class: &str,
    method: &str,
    args: impl Iterator<Item = &'a str>,
    returns: Option<&str>,
) -> u32 {
    let mut hash = 2_166_136_261u32;
    for byte in class
        .bytes()
        .chain(b"::".iter().copied())
        .chain(method.bytes())
        .chain(b"(".iter().copied())
        .chain(args.flat_map(str::bytes))
        .chain(b")->".iter().copied())
        .chain(returns.unwrap_or("").bytes())
    {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

fn render_type(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(|| "()".to_owned(), |segment| segment.ident.to_string()),
        Type::Tuple(tuple) if tuple.elems.is_empty() => "()".to_owned(),
        other => quote_type(other),
    }
}

fn quote_type(ty: &Type) -> String {
    ty.to_token_stream().to_string().replace(' ', "")
}

fn api_type(rust_type: &str) -> Result<String, String> {
    let ty = match rust_type {
        "i8" | "i16" | "i32" | "i64" | "isize" => "i32",
        "u8" | "u16" | "u32" | "u64" | "usize" => "u32",
        "f32" => "f32",
        "f64" => "f64",
        "bool" => "bool",
        "SteelStrView" => "SteelStrView",
        "SteelRef" => "SteelRef",
        "SteelVec3" => "SteelVec3",
        "SteelWorldRef" => "SteelWorldRef",
        "SteelEntityRef" => "SteelEntityRef",
        "SteelBlockState" => "SteelBlockState",
        "BlockId" | "BlockStateId" | "ItemId" | "WorldId" | "EntityId" => "u32",
        "()" | "" => return Ok("()".to_owned()),
        other => return Err(format!("unsupported API type `{other}`")),
    };
    Ok(ty.to_owned())
}

struct ClassMeta {
    name: String,
    inherits: Option<String>,
    receiver: String,
    handle: String,
}

impl ClassMeta {
    fn parse(attr: &Attribute) -> Result<Self, String> {
        let mut name = None;
        let mut inherits = None;
        let mut receiver = "()".to_owned();
        let mut handle = "none".to_owned();
        attr.parse_nested_meta(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .ok_or_else(|| meta.error("expected identifier"))?;
            let value: LitStr = meta.value()?.parse()?;
            match key.as_str() {
                "name" => name = Some(value.value()),
                "inherits" => inherits = Some(value.value()),
                "receiver" => receiver = value.value(),
                "handle" => handle = value.value(),
                _ => {}
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            name: name.ok_or_else(|| "steel_api is missing `name`".to_owned())?,
            inherits,
            receiver,
            handle,
        })
    }
}

struct MethodMeta {
    is_static: bool,
    returns_class: Option<String>,
}

impl MethodMeta {
    fn parse(attr: &Attribute) -> Result<Self, String> {
        let mut is_static = false;
        let mut returns_class = None;
        if attr.meta.require_path_only().is_ok() {
            return Ok(Self {
                is_static,
                returns_class,
            });
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("static") {
                is_static = true;
                return Ok(());
            }
            if meta.path.is_ident("returns_class") {
                let value: LitStr = meta.value()?.parse()?;
                returns_class = Some(value.value());
            }
            Ok(())
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            is_static,
            returns_class,
        })
    }
}
