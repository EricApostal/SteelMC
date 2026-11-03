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
///       command.player?.sendMessage('Hello, ${command.player?.name()}!');
///     });
///   }
/// }
///
/// void main() => runSteelPlugin(Greeter());
/// ```
///
/// Host calls go through ClassDB MethodBinds; the generated classes in
/// `src/steel/generated.dart` carry the typed signatures and the transport in
/// `src/steel/binding.dart` performs the marshalling.
library;

export 'annotations.dart';
export 'src/plugin/context.dart';
export 'src/plugin/runtime.dart';
export 'src/steel/binding.dart';
export 'src/steel/extension.dart';
export 'src/steel/generated.dart';
export 'src/steel/model.dart';
export 'src/steel/native_shim.dart';