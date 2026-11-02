//! Parser for `steel-abi/src/lib.rs`.

use std::collections::BTreeMap;

use syn::{Attribute, Expr, Fields, Item, Type, Visibility};

use crate::model::{
    Api, ConstantDef, ExportFn, FieldDef, FnSignature, FunctionType, RustType, StructDef,
};

const ABI_SOURCE: &str = "steel-abi/src/lib.rs";
const HOST_INTERFACE: &str = "SteelInterface";

/// Parses the extension ABI source into a model.
pub fn parse_source(source: &str) -> Result<Api, String> {
    let file =
        syn::parse_file(source).map_err(|error| format!("failed to parse Rust source: {error}"))?;

    let mut constants = Vec::new();
    let mut structs = Vec::new();
    let mut aliases = Vec::new();
    let mut exports = Vec::new();
    let mut constant_values: BTreeMap<String, i64> = BTreeMap::new();

    for item in &file.items {
        match item {
            Item::Const(constant) => {
                if !matches!(constant.vis, Visibility::Public(_)) {
                    continue;
                }
                let name = constant.ident.to_string();
                if !name.starts_with("STEEL_") || !is_integer_type(&constant.ty) {
                    continue;
                }
                let value = eval_integer(&constant.expr, &constant_values)
                    .ok_or_else(|| format!("could not evaluate constant `{name}`"))?;
                constant_values.insert(name.clone(), value);
                constants.push(ConstantDef { name, value });
            }
            Item::Struct(definition) => {
                if !matches!(definition.vis, Visibility::Public(_))
                    || !has_repr_c(&definition.attrs)
                {
                    continue;
                }
                let fields = struct_fields(&definition.fields);
                structs.push(StructDef {
                    name: definition.ident.to_string(),
                    fields,
                });
            }
            Item::Type(alias) => {
                if !matches!(alias.vis, Visibility::Public(_)) {
                    continue;
                }
                aliases.push((alias.ident.to_string(), AliasTarget::from_type(&alias.ty)));
            }
            Item::Fn(function) => {
                if !matches!(function.vis, Visibility::Public(_)) || !has_no_mangle(&function.attrs)
                {
                    continue;
                }
                let signature = FnSignature::from_signature(&function.sig);
                if !signature.is_c_abi() {
                    continue;
                }
                let arg_names = function
                    .sig
                    .inputs
                    .iter()
                    .enumerate()
                    .filter_map(|(index, argument)| match argument {
                        syn::FnArg::Typed(typed) => {
                            Some(pattern_name(&typed.pat).unwrap_or_else(|| format!("arg{index}")))
                        }
                        syn::FnArg::Receiver(_) => None,
                    })
                    .collect();
                exports.push(ExportFn {
                    name: function.sig.ident.to_string(),
                    signature,
                    arg_names,
                });
            }
            _ => {}
        }
    }

    let function_types = resolve_function_types(&aliases)?;
    let api_version = constant_values
        .get("STEEL_ABI_VERSION")
        .copied()
        .unwrap_or(1);
    let hook_tables = structs
        .iter()
        .filter(|definition| definition.name.ends_with("Hooks"))
        .map(|definition| definition.name.clone())
        .collect();
    if !structs
        .iter()
        .any(|definition| definition.name == HOST_INTERFACE)
    {
        return Err(format!("`{HOST_INTERFACE}` not found in {ABI_SOURCE}"));
    }

    Ok(Api {
        api_version,
        source: ABI_SOURCE.to_owned(),
        constants,
        structs,
        function_types,
        host_interface: HOST_INTERFACE.to_owned(),
        hook_tables,
        exports,
    })
}

enum AliasTarget {
    Signature(FnSignature),
    Alias(String),
    Unsupported,
}

impl AliasTarget {
    fn from_type(ty: &Type) -> Self {
        match ty {
            Type::FnPtr(fn_ptr) => Self::Signature(FnSignature::from_type_fn_ptr(fn_ptr)),
            Type::Path(path)
                if path.qself.is_none()
                    && path.path.segments.len() == 1
                    && matches!(path.path.segments[0].arguments, syn::PathArguments::None) =>
            {
                Self::Alias(path.path.segments[0].ident.to_string())
            }
            _ => Self::Unsupported,
        }
    }
}

fn resolve_alias<'a>(
    name: &'a str,
    targets: &BTreeMap<&'a str, &'a AliasTarget>,
    stack: &mut Vec<&'a str>,
) -> Option<FnSignature> {
    if stack.contains(&name) {
        return None;
    }
    match targets.get(name)? {
        AliasTarget::Signature(signature) => Some(signature.clone()),
        AliasTarget::Alias(target) => {
            stack.push(name);
            let resolved = resolve_alias(target, targets, stack);
            stack.pop();
            resolved
        }
        AliasTarget::Unsupported => None,
    }
}

fn resolve_function_types(aliases: &[(String, AliasTarget)]) -> Result<Vec<FunctionType>, String> {
    let targets: BTreeMap<&str, &AliasTarget> = aliases
        .iter()
        .map(|(name, target)| (name.as_str(), target))
        .collect();

    let mut functions = Vec::new();
    for (name, target) in aliases {
        match target {
            AliasTarget::Signature(signature) => {
                functions.push(function_type(name, signature.clone(), None));
            }
            AliasTarget::Alias(_) => {
                let signature = resolve_alias(name, &targets, &mut Vec::new())
                    .ok_or_else(|| format!("could not resolve function type alias `{name}`"))?;
                let alias_of = match target {
                    AliasTarget::Alias(target) => Some(target.clone()),
                    _ => None,
                };
                functions.push(function_type(name, signature, alias_of));
            }
            AliasTarget::Unsupported => {}
        }
    }
    Ok(functions)
}

fn function_type(name: &str, signature: FnSignature, alias_of: Option<String>) -> FunctionType {
    FunctionType {
        name: name.to_owned(),
        args: signature.inputs.iter().map(RustType::render).collect(),
        return_type: signature
            .output
            .as_ref()
            .map_or_else(|| "()".to_owned(), RustType::render),
        signature,
        alias_of,
    }
}

fn struct_fields(fields: &Fields) -> Vec<FieldDef> {
    match fields {
        Fields::Named(named) => named
            .named
            .iter()
            .enumerate()
            .map(|(index, field)| {
                let name = field
                    .ident
                    .as_ref()
                    .map_or_else(|| index.to_string(), ToString::to_string);
                field_def(name, &field.ty)
            })
            .collect(),
        Fields::Unnamed(unnamed) => unnamed
            .unnamed
            .iter()
            .enumerate()
            .map(|(index, field)| field_def(index.to_string(), &field.ty))
            .collect(),
        Fields::Unit => Vec::new(),
    }
}

fn field_def(name: String, ty: &Type) -> FieldDef {
    let rust_type = RustType::from_syn(ty);
    FieldDef {
        name,
        rust_type: rust_type.render(),
        ty: rust_type,
    }
}

fn has_repr_c(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        if !attribute.path().is_ident("repr") {
            return false;
        }
        match &attribute.meta {
            syn::Meta::List(list) => list.tokens.to_string() == "C",
            _ => false,
        }
    })
}

fn has_no_mangle(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        if attribute.path().is_ident("no_mangle") {
            return true;
        }
        attribute.path().is_ident("unsafe")
            && attribute
                .meta
                .require_list()
                .is_ok_and(|list| list.tokens.to_string() == "no_mangle")
    })
}

fn pattern_name(pattern: &syn::Pat) -> Option<String> {
    match pattern {
        syn::Pat::Ident(ident) => Some(ident.ident.to_string()),
        _ => None,
    }
}

fn is_integer_type(ty: &Type) -> bool {
    match ty {
        Type::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
            let name = path.path.segments[0].ident.to_string();
            matches!(
                name.as_str(),
                "u8" | "u16" | "u32" | "u64" | "usize" | "i8" | "i16" | "i32" | "i64" | "isize"
            )
        }
        _ => false,
    }
}

fn eval_integer(expression: &Expr, constants: &BTreeMap<String, i64>) -> Option<i64> {
    match expression {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Int(value) => value.base10_digits().parse().ok(),
            _ => None,
        },
        Expr::Paren(paren) => eval_integer(&paren.expr, constants),
        Expr::Group(group) => eval_integer(&group.expr, constants),
        Expr::Path(path) => {
            let name = path.path.segments.last()?.ident.to_string();
            constants.get(&name).copied()
        }
        Expr::Unary(unary) => {
            let value = eval_integer(&unary.expr, constants)?;
            match unary.op {
                syn::UnOp::Neg(_) => value.checked_neg(),
                syn::UnOp::Not(_) => Some(!value),
                _ => None,
            }
        }
        Expr::Binary(binary) => {
            let left = eval_integer(&binary.left, constants)?;
            let right = eval_integer(&binary.right, constants)?;
            match binary.op {
                syn::BinOp::Add(_) => left.checked_add(right),
                syn::BinOp::Sub(_) => left.checked_sub(right),
                syn::BinOp::Mul(_) => left.checked_mul(right),
                syn::BinOp::Div(_) => left.checked_div(right),
                syn::BinOp::Rem(_) => left.checked_rem(right),
                syn::BinOp::BitXor(_) => Some(left ^ right),
                syn::BinOp::BitAnd(_) => Some(left & right),
                syn::BinOp::BitOr(_) => Some(left | right),
                syn::BinOp::Shl(_) => u32::try_from(right)
                    .ok()
                    .and_then(|shift| left.checked_shl(shift)),
                syn::BinOp::Shr(_) => u32::try_from(right)
                    .ok()
                    .and_then(|shift| left.checked_shr(shift)),
                _ => None,
            }
        }
        Expr::Cast(cast) => eval_integer(&cast.expr, constants),
        _ => None,
    }
}
