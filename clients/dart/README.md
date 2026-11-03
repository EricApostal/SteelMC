# Steel extension API for Dart

`steel_extension` provides two layers:

- `lib/steel.dart` — generated `dart:ffi` structs/function types for the
  `SteelInterface` C ABI (`lib/src/bindings.g.dart`).
- `lib/steel_plugin.dart` — the in-process plugin authoring API. Plugins run
  inside the server process in an embedded Dart VM and call host methods
  through ClassDB MethodBinds. There is no JSON or subprocess boundary.
- `lib/src/steel/generated.dart` — the generated host classes (`World`,
  `Entity`, `Player`, `Server`, `Block`, ...). Every method is a typed call
  through `SteelBinding`; plugins never see FFI signatures.

## Generating the bindings

```sh
cargo run -p steel-plugin-gen
cargo run -p steel-plugin-gen -- --check   # fail if generated files are stale
```

This refreshes `steel-abi/steel_api.json`, `steel-abi/include/steel_extension.h`,
`clients/dart/lib/src/bindings.g.dart`, `clients/dart/lib/src/steel/ffi_api.dart`,
`steel-api/steel_api.json`, and `clients/dart/lib/src/steel/generated.dart`.

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
        command.player?.sendMessage('Hello, ${command.player?.name()}!');
      });
    context.onChat((event) {
      if (event.text.contains('bad')) event.cancel();
    });
  }
}

void main() => runSteelPlugin(Greeter());
```

`Player`, `World`, `Block`, and the other host classes are generated from
`steel-api/steel_api.json` by `steel-plugin-gen`. They call host methods through
`SteelBinding.ptrcall`, which caches ClassDB MethodBinds and handles the native
argument/return layout. Tests implement `SteelBinding` (and `NativeShim`) with
canned values.

## Extension classes

A plugin can also expose Dart classes to the host ClassDB. Annotate the class
with `@SteelClass` and its instance methods with `@SteelMethod`, then add a
`part` directive and run the `steel_builder` generator:

```dart
import 'package:steel_extension/annotations.dart';

part 'greeter.steel.dart';

@SteelClass('Greeter')
final class Greeter {
  @SteelMethod()
  String greet(String name) => 'Hello, $name!';

  @SteelMethod()
  int count() => 0;
}
```

```sh
dart run build_runner build
```

This writes `greeter.steel.dart` with a `List<SteelClassRegistration>
get steelClasses` getter. Pass it to the runtime:

```dart
void main() => runSteelPlugin(MyPlugin(), classes: steelClasses);
```

`runSteelPlugin` registers each class through the native shim, which forwards
`SteelExtensionClassDesc`/`SteelExtensionMethodDesc` entries to the host
ClassDB immediately (and retries at the behavior phase if the host was not
ready). The host can then construct instances with `classdb_construct` and
call methods through `classdb_get_method_bind` + `method_bind_ptrcall`.
Instance handles use `SteelRef { kind: STEEL_REF_EXTENSION (11), index:
instance_id, generation: 1 }`.

### Calling another Dart plugin

All plugins share one host ClassDB. A plugin does not get a private ClassDB;
it contributes classes to the shared table. That is what makes cross-plugin
calls possible without RPC or IPC:

```dart
final vault = binding.construct('Vault');
final balance = binding
    .ptrcall(
      'Vault',
      'deposit',
      steelMethodHash('Vault::deposit(String,double)->double'),
      vault,
      <SteelArg>[SteelArg.string('Steve'), SteelArg.f64(15.5)],
      SteelReturnKind.f64,
    )
    .asDouble();
```

The host resolves `Vault` in the same ClassDB and invokes the provider's thunk.
The provider's thunk acquires the provider isolate, calls its Dart method, and
returns. Each shim gives Dart a local interface copy whose `classdb_construct`
and `method_bind_ptrcall` wrappers release the caller isolate around the host
call, so the callee shim can `DartEngine_AcquireIsolate` its own isolate. This
is the only isolate-handoff detail a plugin author needs to know.

Provider and consumer must agree on the class name, method name, argument
types, and hash. Share a small Dart interface package, or pass `0` as the hash
to resolve by name. Both plugins must be loaded before the first cross-plugin
call; resolve binds lazily if load order is not guaranteed.

### Plugins folder

`ExtensionManager::load_dir` walks subdirectories, so a server can use:

```text
plugins/
  01_vault/
    plugin.so          # copy of libsteel_dart_embed.so
    plugin.so.json     # {"id":"vault", "kernel":"vault.dill", ...}
    vault.dill
  02_economy/
    plugin.so
    plugin.so.json
    economy.dill
```

Each plugin needs its own physical shim copy: the shim's VM engine, class
tables, and registrations are process-global per loaded library. A shared
`libdart_engine_jit_shared.so` may be referenced by every sidecar.

Method signatures support required positional parameters of `int`, `double`,
`bool` and `String`, and return one of those types or `void`. The generator
reports any other type (including named or optional parameters, static methods
and classes without an unnamed no-argument constructor) as a build error.
Method hashes are FNV-1a 32 over `HostClass::method(ArgType,...)->ReturnType`,
computed by `steelMethodHash`.

`@SteelProperty` is recorded for tooling only; expose a getter annotated with
`@SteelMethod` to make a value reachable from the host.

The builder is `clients/dart_builder`; it is applied automatically to packages
that depend on it (see `build.yaml` here and in `examples/dart_essentials`).

## Tests

```sh
cargo build -p steel-abi
dart test
```

The FFI conformance tests look for `target/debug/libsteel_abi.so` (or set
`STEEL_ABI_LIB`). The plugin tests use an in-memory `SteelBinding`.

See `examples/dart_essentials` for a complete Essentials-style plugin.
