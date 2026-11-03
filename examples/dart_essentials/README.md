# Dart Essentials example

An Essentials-style Steel plugin written in Dart. It runs inside the server
process in an embedded Dart VM; host calls go directly through the
`SteelInterface` C function table.

## Commands

| Command | Description |
| --- | --- |
| `/spawn` | Teleport to the world spawn. |
| `/setblock <x> <y> <z> <block>` | Place a block's default state. |
| `/sethome [name]` | Save the current position as a home (default `home`). |
| `/home [name]` | Teleport to a saved home. |
| `/delhome [name]` | Delete a saved home. |
| `/homes` | List saved homes. |
| `/tpa <player>` | Ask another player to teleport to them. |
| `/tpaccept` | Accept the latest pending request. |
| `/tpdeny` | Deny the latest pending request. |
| `/msg <player> <message>` | Send a private message. |
| `/who` | List online players in your world. |
| `/heal` | Restore full health. |

Homes persist in `homes.json` next to the server working directory, or at
`STEEL_ESSENTIALS_HOMES` if set.

## Extension class

`lib/greeter.dart` declares a host class with `@SteelClass('Greeter')`. The
build_runner generator writes `lib/greeter.steel.dart`, whose `steelClasses`
list is passed to `runSteelPlugin` in `bin/essentials.dart`:

```dart
@SteelClass('Greeter')
final class Greeter {
  @SteelMethod()
  String greet(String name) => 'Hello, $name!';
}
```

```dart
void main() => runSteelPlugin(EssentialsPlugin(...), classes: steelClasses);
```

The host can then call `classdb_construct("Greeter", ...)` followed by
`classdb_get_method_bind("Greeter", "greet", 0)` and `method_bind_ptrcall`.

## Build and test

```sh
dart pub get
dart run build_runner build   # regenerates lib/greeter.steel.dart
dart analyze
dart test
```

## Build the kernel

The embedded runtime loads a Dart kernel, not source. Use the matching Dart SDK
source build (the one that produced `libdart_engine_jit_shared.so`):

```sh
STEEL_DART_SDK=/path/to/dart-sdk/out/ReleaseX64 ./build_kernel.sh
```

This writes `essentials.dill`. Set `STEEL_DART_ENTRY` and
`STEEL_DART_OUTPUT` to build another entry point, for example the minimal
extension-class kernel used by the Rust embedding test:

```sh
STEEL_DART_SDK=/path/to/dart-sdk/out/ReleaseX64 \
STEEL_DART_ENTRY=$PWD/bin/extension_only.dart \
STEEL_DART_OUTPUT=$PWD/extension_only.dill ./build_kernel.sh
```

Then run the embedded tests (they skip when the kernel or VM library is not
configured):

```sh
STEEL_DART_VM_LIB=/path/to/libdart_engine_jit_shared.so \
STEEL_DART_ESSENTIALS_KERNEL=$PWD/examples/dart_essentials/essentials.dill \
STEEL_DART_EXTENSION_KERNEL=$PWD/examples/dart_essentials/extension_only.dill \
cargo test -p steel-dart-embed --test embed_smoke
```

## Load into Steel

```sh
cargo build -p steel-dart-embed
cp target/debug/libsteel_dart_embed.so examples/dart_essentials/essentials.so
cp examples/dart_essentials/sidecar.example.json \
  examples/dart_essentials/essentials.so.json
```

Edit the sidecar's `vm` path, then point `[extensions] directory` at
`examples/dart_essentials` (or copy the three files into `plugins/`).

## Layout

```
lib/home_store.dart       JSON persistence for player homes
lib/essentials.dart       Essentials service, plugin, and command handlers
lib/greeter.dart          @SteelClass host extension class
lib/greeter.steel.dart    generated SteelClassRegistration metadata
bin/essentials.dart       main() -> runSteelPlugin(..., classes: steelClasses)
bin/extension_only.dart   minimal plugin for the Rust extension-class test
test/essentials_test.dart
test/greeter_test.dart    generated metadata and invocation
build.yaml                enables the steel_builder generator
build_kernel.sh           kernel build using bootstrap_gen_kernel
sidecar.example.json      runtime manifest template
```

## Testing without the VM

Command handlers take a `CommandContext` and perform everything through the
generated `Player`/`World`/`Server` classes. `test/essentials_test.dart` supplies
a fake `SteelBinding`, so the full command logic runs in memory.
