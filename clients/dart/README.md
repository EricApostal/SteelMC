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

## Plugin API

`lib/steel_plugin.dart` is the out-of-process authoring API. Unlike `steel.dart`
(FFI bindings), it speaks newline-delimited JSON to the Rust bridge over stdio,
so a plugin is an ordinary Dart program:

```dart
import 'package:steel_extension/steel_plugin.dart';

final class Greeter implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    context
        .command('hello')
        .description('Say hello.')
        .executes((command) {
      command.player?.sendMessage('Hello, ${command.player?.name}!');
      return null;
    });
    context.onChat((event) {
      if (event.text.contains('bad')) {
        event.cancel();
      }
    });
  }
}

void main() => runSteelPlugin(Greeter());
```

Player operations, host queries and registration all go through `PluginHost`.
The runtime implements it over `RpcClient`; tests implement it with canned
replies, so command logic runs without stdin, a subprocess, or the Rust bridge.

- `LineTransport` / `StdioTransport` — newline-delimited JSON framing.
- `RpcClient` — `host_call`/`host_result` routing, including nested calls made
  while a handler waits for a reply.
- `PluginContext` — command/event registration plus `onlinePlayers`,
  `findPlayer`, `spawn`, and `log`.
- `Player`, `Vec3`, `ChatEvent`, `BlockEvent`, `CommandContext` — typed values.

See `examples/dart_essentials` for a complete Essentials-style plugin.
