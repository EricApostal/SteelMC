import 'package:steel_essentials/greeter.dart';
import 'package:steel_extension/steel_plugin.dart';

/// A minimal kernel used by `steel-dart-embed`'s extension-class test.
///
/// It registers the generated `Greeter` class with the host ClassDB and does
/// nothing else, so the test can exercise `register_extension_class`,
/// `classdb_construct`, `classdb_get_method_bind` and `method_bind_ptrcall`
/// without depending on the rest of the Essentials commands.
final class ExtensionOnlyPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {}
}

void main() => runSteelPlugin(ExtensionOnlyPlugin(), classes: steelClasses);