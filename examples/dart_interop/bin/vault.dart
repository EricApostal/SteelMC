import 'package:steel_extension/steel_plugin.dart';
import 'package:steel_interop/vault.dart';

final class VaultPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {}
}

void main() => runSteelPlugin(VaultPlugin(), classes: steelClasses);
