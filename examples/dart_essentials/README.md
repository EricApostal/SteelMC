# Dart Essentials example

An Essentials-style Steel plugin built on the high-level Dart plugin API in
[`clients/dart`](../../clients/dart). It runs out of process and speaks
newline-delimited JSON to the Rust bridge; there is no FFI involved.

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

Homes are persisted in `homes.json`, keyed by player UUID and home name, next
to the process working directory.

The example registers commands without permissions so they work out of the box.
Add `.permission('essentials.home')` in `register` when you want the host to
enforce a permission; the Dart API and bridge support it.

## Layout

```
lib/home_store.dart    JSON persistence for player homes
lib/essentials.dart    Essentials service, plugin, and command handlers
bin/essentials.dart    entry point started by the Rust bridge
test/essentials_test.dart
```

## Running

```sh
dart pub get
dart analyze
dart test
```

The plugin process is normally started by the Rust host. It reads protocol
messages from stdin and writes responses to stdout, so it can also be exercised
by hand:

```sh
printf '%s\n' \
  '{"t":"init","level":1}' \
  '{"t":"command","id":1,"player":{"entity":7,"world":0,"name":"Steve","uuid":"..."},"command":"homes","args":""}' \
  '{"t":"shutdown"}' | dart run bin/essentials.dart
```

`dart run bin/essentials.dart` prints the protocol's JSON replies and log lines
on stdout.

To ship a standalone executable instead of requiring the Dart SDK at runtime:

```sh
dart compile exe bin/essentials.dart -o essentials
```

Then point the sidecar `entry` at `essentials`.

## Loading it into Steel

```sh
cargo build -p steel-dart-bridge
cp target/debug/libsteel_dart_bridge.so examples/dart_essentials/essentials.so
cp examples/dart_essentials/sidecar.example.json examples/dart_essentials/essentials.so.json
```

Point `[extensions] directory` at `examples/dart_essentials` (or copy the two
files into `plugins/` and adjust `cwd`). The bridge reads
`<library>.so.json`, starts `dart run bin/essentials.dart`, and registers the
commands during the behavior phase. Requires the Dart SDK unless `entry` points
at a `dart compile exe` binary.

## Sidecar manifest

The Rust bridge discovers the plugin through a small JSON descriptor:

```json
{"id":"essentials","version":"0.1.0","entry":"bin/essentials.dart","cwd":"."}
```

`entry` is relative to `cwd`, so a compiled binary would use
`{"entry":"essentials"}` instead. The working directory also determines where
`homes.json` is written.

## Testing without a process

Command handlers take a `CommandContext` and perform everything through the
`PluginHost` interface. `test/essentials_test.dart` supplies a
`FakePluginHost` with canned replies, so the full command logic runs in memory:

```dart
final host = FakePluginHost();
final store = HomeStore(path: '${tempDir.path}/homes.json')..load();
final essentials = Essentials(host: host, homes: store);
essentials.setHome(contextForPlayer('/sethome'));
```