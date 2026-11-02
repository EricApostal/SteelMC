/// A high-level Dart API for authoring Steel plugins.
///
/// Plugins run in a separate Dart process and speak newline-delimited JSON to
/// the Rust bridge. Implement [SteelPlugin], register commands and events in
/// [SteelPlugin.onEnable], and start the process with [runSteelPlugin]:
///
/// ```dart
/// import 'package:steel_extension/steel_plugin.dart';
///
/// final class Greeter implements SteelPlugin {
///   @override
///   void onEnable(PluginContext context) {
///     context
///         .command('hello')
///         .description('Greet the player.')
///         .executes((command) {
///       command.player?.sendMessage('Hello, ${command.player?.name}!');
///     });
///   }
/// }
///
/// void main() => runSteelPlugin(Greeter());
/// ```
///
/// This layer has no `dart:ffi` dependency; it is the out-of-process transport
/// counterpart to the generated bindings in `steel.dart`.
library;

export 'src/plugin/context.dart';
export 'src/plugin/model.dart';
export 'src/plugin/rpc.dart';
export 'src/plugin/run.dart';
export 'src/plugin/transport.dart';
