# Steel extension bindings for Dart

`steel_extension` exposes the generated `dart:ffi` bindings for the Steel
extension C ABI. The bindings are produced from `steel-abi/src/lib.rs` by
`steel-plugin-gen`; do not edit `lib/src/bindings.g.dart` by hand.

## Generating the bindings

```sh
cargo run -p steel-plugin-gen
cargo run -p steel-plugin-gen -- --check   # fail if generated files are stale
```

This also refreshes `steel-abi/steel_api.json` and
`steel-abi/include/steel_extension.h`.

## Running the ABI conformance tests

Build the ABI library first so the test can locate it:

```sh
cargo build -p steel-abi
dart test
```

The tests look for `target/debug/libsteel_abi.so` (or the platform-specific
name under `target/release`). Set `STEEL_ABI_LIB` to an explicit library path if
it was built elsewhere:

```sh
STEEL_ABI_LIB=/path/to/libsteel_abi.so dart test
```

When no compiled library is available the ABI tests are skipped.

## Usage

```dart
import 'dart:ffi';
import 'package:steel_extension/steel.dart';

final bindings = SteelBindings.open(DynamicLibrary.open('libsteel_abi.so'));
print(bindings.apiVersion());

// `view` is a SteelStrView received from a native callback.
final SteelStrView view = ...;
print(steelStrViewToString(view));
```

`SteelConstants` mirrors the numeric constants from the ABI, and the generated
struct classes (`SteelInterface`, `SteelExtension`, the hook tables, and their
supporting types) can be used directly with `Pointer<T>` and
`NativeFunction<T>`.
