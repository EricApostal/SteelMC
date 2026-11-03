# Steel extension API for Dart

`steel_extension` provides two layers:

- `lib/steel.dart` — generated `dart:ffi` structs/function types for the
  `SteelInterface` C ABI (`lib/src/bindings.g.dart`).
- `lib/steel_plugin.dart` — the in-process plugin authoring API. Plugins run
  inside the server process in an embedded Dart VM and call host functions
  directly through the C function table. There is no JSON or subprocess
  boundary.

## Generating the bindings

```sh
cargo run -p steel-plugin-gen
cargo run -p steel-plugin-gen -- --check   # fail if generated files are stale
```

This refreshes `steel-abi/steel_api.json`, `steel-abi/include/steel_extension.h`,
`clients/dart/lib/src/bindings.g.dart`, and
`clients/dart/lib/src/plugin/generated_api.dart`.

## Embedded runtime

The `steel-dart-embed` crate loads a Dart VM runtime
(`libdart_engine_jit_shared.so`), starts a kernel, and exposes
`SteelInterface` to Dart through `dart:ffi`. Host callbacks acquire the Dart
isolate, invoke `steelDispatchCommand`/`steelDispatchEvent`, and return the
result synchronously.

Build a plugin kernel with the matching Dart SDK source build:

```sh
STEEL_DART_SDK=/path/to/dart-sdk/out/ReleaseX64 \
  ./examples/dart_essentials/build_kernel.sh
```

The runtime shim is `target/debug/libsteel_dart_embed.so`. Next to it, a
sidecar describes the plugin:

```json
{"id":"essentials","version":"0.1.0","kernel":"essentials.dill",
 "vm":"/path/libdart_engine_jit_shared.so","entry":"main"}
```

Copy the shim as `<name>.so`, write `<name>.so.json`, and point
`[extensions] directory` in `config.toml` at that directory.

## Plugin API

```dart
import 'package:steel_extension/steel_plugin.dart';

final class Greeter implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    context.command('hello')
      .description('Say hello.')
      .executes((command) {
        command.player?.sendMessage('Hello, ${command.player?.name}!');
      });
    context.onChat((event) {
      if (event.text.contains('bad')) event.cancel();
    });
  }
}

void main() => runSteelPlugin(Greeter());
```

`Player`, `World`, and `Server` are generated from `steel-plugin-gen` and call
the `PluginHost` interface, which `NativePluginHost` implements directly over
`SteelInterface`. Tests implement `PluginHost` with canned values.

## Tests

```sh
cargo build -p steel-abi
dart test
```

The FFI conformance tests look for `target/debug/libsteel_abi.so` (or set
`STEEL_ABI_LIB`). The plugin tests use an in-memory `PluginHost`.

See `examples/dart_essentials` for a complete Essentials-style plugin.
