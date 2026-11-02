//! Dart `dart:ffi` binding emitter.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::model::{
    Api, ExportFn, FnSignature, RustType, collect_synthetic_fn_types,
    function_types_in_dependency_order, synthetic_fn_name,
};

struct Context {
    aliases: BTreeSet<String>,
    synthetic: BTreeMap<String, FnSignature>,
}

impl Context {
    fn new(api: &Api) -> Self {
        Self {
            aliases: api
                .function_types
                .iter()
                .map(|function| function.name.clone())
                .collect(),
            synthetic: collect_synthetic_fn_types(api),
        }
    }

    fn native_type(&self, ty: &RustType) -> String {
        match ty {
            RustType::Named(name) => {
                if let Some(native) = native_primitive(name) {
                    native.to_owned()
                } else if ty.is_c_void() {
                    "Void".to_owned()
                } else if self.aliases.contains(name) {
                    format!("Pointer<NativeFunction<{name}>>")
                } else {
                    name.clone()
                }
            }
            RustType::Option(inner) => match inner.as_ref() {
                RustType::Named(name) if self.aliases.contains(name) => {
                    format!("Pointer<NativeFunction<{name}>>")
                }
                RustType::FnPtr(signature) => {
                    format!("Pointer<NativeFunction<{}>>", synthetic_fn_name(signature))
                }
                other => self.native_type(other),
            },
            RustType::Pointer { elem, .. } => format!("Pointer<{}>", self.pointer_element(elem)),
            RustType::FnPtr(signature) => {
                format!("Pointer<NativeFunction<{}>>", synthetic_fn_name(signature))
            }
            RustType::Unit | RustType::Tuple(_) => "Void".to_owned(),
            RustType::Unknown(rust_type) => rust_type.clone(),
        }
    }

    fn dart_type(&self, ty: &RustType) -> String {
        match ty {
            RustType::Named(name) => {
                if native_primitive(name).is_some() {
                    dart_primitive(name).to_owned()
                } else if self.aliases.contains(name) {
                    format!("Pointer<NativeFunction<{name}>>")
                } else {
                    name.clone()
                }
            }
            RustType::Option(inner) => match inner.as_ref() {
                RustType::Named(name) if self.aliases.contains(name) => {
                    format!("Pointer<NativeFunction<{name}>>")
                }
                RustType::FnPtr(signature) => {
                    format!("Pointer<NativeFunction<{}>>", synthetic_fn_name(signature))
                }
                other => self.dart_type(other),
            },
            RustType::Pointer { elem, .. } => format!("Pointer<{}>", self.pointer_element(elem)),
            RustType::FnPtr(signature) => {
                format!("Pointer<NativeFunction<{}>>", synthetic_fn_name(signature))
            }
            RustType::Unit | RustType::Tuple(_) => "void".to_owned(),
            RustType::Unknown(rust_type) => rust_type.clone(),
        }
    }

    fn pointer_element(&self, elem: &RustType) -> String {
        match elem {
            RustType::Named(_) if elem.is_c_void() => "Void".to_owned(),
            RustType::Named(name) if self.aliases.contains(name) => {
                format!("NativeFunction<{name}>")
            }
            other => self.native_type(other),
        }
    }

    fn function_type(&self, signature: &FnSignature) -> String {
        let output = signature
            .output
            .as_ref()
            .map_or_else(|| "Void".to_owned(), |output| self.native_type(output));
        let inputs = signature
            .inputs
            .iter()
            .map(|input| self.native_type(input))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{output} Function({inputs})")
    }

    fn dart_function_type(&self, signature: &FnSignature) -> String {
        let output = signature
            .output
            .as_ref()
            .map_or_else(|| "void".to_owned(), |output| self.dart_type(output));
        let inputs = signature
            .inputs
            .iter()
            .map(|input| self.dart_type(input))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{output} Function({inputs})")
    }
}

fn native_primitive(name: &str) -> Option<&'static str> {
    Some(match name {
        "u8" | "bool" => "Uint8",
        "u16" => "Uint16",
        "u32" => "Uint32",
        "u64" => "Uint64",
        "i8" => "Int8",
        "i16" => "Int16",
        "i32" => "Int32",
        "i64" => "Int64",
        "usize" => "Size",
        "isize" => "IntPtr",
        "f32" => "Float",
        "f64" => "Double",
        _ => return None,
    })
}

fn native_annotation(name: &str) -> Option<&'static str> {
    Some(match name {
        "u8" | "bool" => "Uint8",
        "u16" => "Uint16",
        "u32" => "Uint32",
        "u64" => "Uint64",
        "i8" => "Int8",
        "i16" => "Int16",
        "i32" => "Int32",
        "i64" => "Int64",
        "usize" => "Size",
        "isize" => "IntPtr",
        "f32" => "Float",
        "f64" => "Double",
        _ => return None,
    })
}

fn dart_primitive(name: &str) -> &'static str {
    match name {
        "f32" | "f64" => "double",
        _ => "int",
    }
}

fn snake_to_lower_camel(name: &str) -> String {
    let mut result = String::new();
    for (index, segment) in name
        .split('_')
        .filter(|segment| !segment.is_empty())
        .enumerate()
    {
        if index == 0 {
            result.push_str(&segment.to_ascii_lowercase());
        } else {
            let mut characters = segment.chars();
            if let Some(first) = characters.next() {
                result.push(first.to_ascii_uppercase());
                result.push_str(&characters.as_str().to_ascii_lowercase());
            }
        }
    }
    if result.is_empty() {
        name.to_ascii_lowercase()
    } else {
        result
    }
}

fn snake_to_upper_camel(name: &str) -> String {
    let mut result = String::new();
    for segment in name.split('_').filter(|segment| !segment.is_empty()) {
        let mut characters = segment.chars();
        if let Some(first) = characters.next() {
            result.push(first.to_ascii_uppercase());
            result.push_str(&characters.as_str().to_ascii_lowercase());
        }
    }
    if result.is_empty() {
        "Generated".to_owned()
    } else {
        result
    }
}

fn method_name(export: &str) -> String {
    if export == "steel_abi_version" {
        return "apiVersion".to_owned();
    }
    let short = export.strip_prefix("steel_abi_").unwrap_or(export);
    snake_to_lower_camel(short)
}

fn field_name(name: &str) -> String {
    if name.chars().all(|character| character.is_ascii_digit()) {
        format!("field{name}")
    } else {
        name.to_owned()
    }
}

fn write_struct_field(context: &Context, out: &mut String, name: &str, ty: &RustType) {
    let field = field_name(name);
    match ty {
        RustType::Named(primitive) if native_annotation(primitive).is_some() => {
            let annotation = native_annotation(primitive).unwrap_or("Uint8");
            let dart = dart_primitive(primitive);
            let _ = writeln!(out, "  @{annotation}()");
            let _ = writeln!(out, "  external {dart} {field};");
        }
        RustType::Named(alias) if context.aliases.contains(alias) => {
            let _ = writeln!(out, "  external Pointer<NativeFunction<{alias}>> {field};");
        }
        RustType::Named(_) | RustType::Pointer { .. } => {
            let dart = context.native_type(ty);
            let _ = writeln!(out, "  external {dart} {field};");
        }
        RustType::Option(inner) => match inner.as_ref() {
            RustType::Named(alias) if context.aliases.contains(alias) => {
                let _ = writeln!(out, "  external Pointer<NativeFunction<{alias}>> {field};");
            }
            RustType::FnPtr(signature) => {
                let synthetic = synthetic_fn_name(signature);
                let _ = writeln!(
                    out,
                    "  external Pointer<NativeFunction<{synthetic}>> {field};"
                );
            }
            other => write_struct_field(context, out, name, other),
        },
        RustType::FnPtr(signature) => {
            let synthetic = synthetic_fn_name(signature);
            let _ = writeln!(
                out,
                "  external Pointer<NativeFunction<{synthetic}>> {field};"
            );
        }
        RustType::Tuple(_) | RustType::Unit | RustType::Unknown(_) => {}
    }
}

fn write_bindings(out: &mut String, context: &Context, exports: &[ExportFn]) {
    for export in exports {
        let type_name = snake_to_upper_camel(&export.name);
        let _ = writeln!(
            out,
            "typedef _{type_name}Native = {};",
            context.function_type(&export.signature)
        );
        let _ = writeln!(
            out,
            "typedef _{type_name}Dart = {};",
            context.dart_function_type(&export.signature)
        );
        out.push('\n');
    }

    out.push_str("/// Typed lookups for the conformance helpers exported by `steel-abi`.\n");
    out.push_str("final class SteelBindings {\n");
    out.push_str("  final DynamicLibrary library;\n\n");

    for export in exports {
        let type_name = snake_to_upper_camel(&export.name);
        let field = format!("_{}", snake_to_lower_camel(&export.name));
        let _ = writeln!(out, "  late final _{type_name}Dart {field};");
    }
    out.push('\n');

    out.push_str("  SteelBindings._(this.library) {\n");
    for export in exports {
        let type_name = snake_to_upper_camel(&export.name);
        let field = format!("_{}", snake_to_lower_camel(&export.name));
        let _ = writeln!(
            out,
            "    {field} = library.lookupFunction<_{type_name}Native, _{type_name}Dart>('{}');",
            export.name
        );
    }
    out.push_str("  }\n\n");
    out.push_str("  factory SteelBindings.open(DynamicLibrary lib) =>\n");
    out.push_str("      SteelBindings._(lib);\n\n");

    for export in exports {
        let field = format!("_{}", snake_to_lower_camel(&export.name));
        let method = method_name(&export.name);
        let parameters: Vec<_> = export
            .signature
            .inputs
            .iter()
            .zip(export.arg_names.iter())
            .map(|(ty, name)| format!("{} {name}", context.dart_type(ty)))
            .collect();
        let arguments: Vec<_> = export
            .arg_names
            .iter()
            .map(|name| name.replace('$', ""))
            .collect();
        let return_type = export
            .signature
            .output
            .as_ref()
            .map_or_else(|| "void".to_owned(), |output| context.dart_type(output));
        let call = format!("{field}({})", arguments.join(", "));
        if return_type == "void" {
            let _ = writeln!(
                out,
                "  void {method}({}) {{\n    {call};\n  }}\n",
                parameters.join(", ")
            );
        } else {
            let _ = writeln!(
                out,
                "  {return_type} {method}({}) => {call};\n",
                parameters.join(", ")
            );
        }
    }
    out.push_str("}\n");
}

/// Renders `bindings.g.dart`.
#[must_use]
pub fn emit_bindings(api: &Api) -> String {
    let context = Context::new(api);
    let mut out = String::new();
    out.push_str("// Generated by steel-plugin-gen. Do not edit.\n");
    out.push_str(
        "// ignore_for_file: non_constant_identifier_names, constant_identifier_names\n\n",
    );
    out.push_str("import 'dart:ffi';\n\n");

    out.push_str("/// Numeric constants declared by the Steel extension ABI.\n");
    out.push_str("abstract final class SteelConstants {\n");
    for constant in &api.constants {
        let name = if constant.name == "STEEL_ABI_VERSION" {
            "steelApiVersion".to_owned()
        } else {
            snake_to_lower_camel(&constant.name)
        };
        let _ = writeln!(out, "  static const int {name} = {};", constant.value);
    }
    out.push_str("}\n\n");

    out.push_str("// Function pointer signatures.\n");
    for (name, signature) in &context.synthetic {
        let _ = writeln!(
            out,
            "typedef {name} = {};",
            context.function_type(signature)
        );
    }
    for function in function_types_in_dependency_order(api) {
        if let Some(target) = &function.alias_of {
            let _ = writeln!(out, "typedef {} = {target};", function.name);
        } else {
            let _ = writeln!(
                out,
                "typedef {} = {};",
                function.name,
                context.function_type(&function.signature)
            );
        }
    }
    out.push('\n');

    out.push_str("/// Native signature of an extension entry point.\n");
    out.push_str(
        "typedef SteelExtensionEntryNative = Pointer<SteelExtension> Function(Pointer<SteelInterface>);\n",
    );
    out.push_str("/// Dart-side signature of an extension entry point.\n");
    out.push_str(
        "typedef SteelExtensionEntryDart = Pointer<SteelExtension> Function(Pointer<SteelInterface>);\n\n",
    );

    out.push_str("// Struct definitions.\n");
    for definition in &api.structs {
        let _ = writeln!(out, "final class {} extends Struct {{", definition.name);
        for field in &definition.fields {
            write_struct_field(&context, &mut out, &field.name, &field.ty);
        }
        out.push_str("}\n\n");
    }

    write_bindings(&mut out, &context, &api.exports);
    out
}
