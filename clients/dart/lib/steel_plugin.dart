/// A high-level Dart API for authoring Steel plugins.
///
/// Plugins run inside the server process in an embedded Dart VM. Implement
/// [SteelPlugin], register commands and events in [SteelPlugin.onEnable], and
/// start the runtime with [runSteelPlugin] from the plugin's `main`:
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
/// Host calls go directly through the `SteelInterface` C function table; there
/// is no JSON or subprocess boundary.
library;

export 'src/plugin/context.dart';
export 'src/plugin/native.dart';
export 'src/plugin/model.dart';
export 'src/plugin/runtime.dart';
