# Dart Essentials example

An Essentials-style Steel plugin written in Dart. It runs inside the server
process in an embedded Dart VM; host calls go directly through the
`SteelInterface` C function table.

## Commands

| Command | Description |
| --- | --- |
| `/spawn` | Teleport to the world spawn. |
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

## Build and test

```sh
dart pub get
dart analyze
dart test
```

## Build the kernel

The embedded runtime loads a Dart kernel, not source. Use the matching Dart SDK
source build (the one that produced `libdart_engine_jit_shared.so`):

```sh
STEEL_DART_SDK=/path/to/dart-sdk/out/ReleaseX64 ./build_kernel.sh
```

This writes `essentials.dill`.

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
lib/home_store.dart    JSON persistence for player homes
lib/essentials.dart    Essentials service, plugin, and command handlers
bin/essentials.dart    main() -> runSteelPlugin(...)
test/essentials_test.dart
build_kernel.sh        kernel build using bootstrap_gen_kernel
sidecar.example.json   runtime manifest template
```

## Testing without the VM

Command handlers take a `CommandContext` and perform everything through the
typed `PluginHost` interface. `test/essentials_test.dart` supplies a fake host,
so the full command logic runs in memory.
