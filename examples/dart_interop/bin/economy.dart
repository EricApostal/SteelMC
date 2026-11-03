import 'package:steel_extension/steel_plugin.dart';
import 'package:steel_interop/vault_client.dart';

final class EconomyPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    final vault = VaultClient(context.binding);
    final balance = vault.deposit('Steve', 15.5);
    context.log('vault balance: $balance');
  }
}

void main() => runSteelPlugin(EconomyPlugin());
