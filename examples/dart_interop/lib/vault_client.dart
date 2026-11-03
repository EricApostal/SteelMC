import 'package:steel_extension/steel_plugin.dart';

/// Consumer-side wrapper for the `Vault` extension class registered by another
/// Dart plugin. Both plugins share the same hash/signature convention, so this
/// package can call the provider through the host ClassDB.
final class VaultClient {
  VaultClient(this.binding);

  final SteelBinding binding;

  late final SteelRef _vault = binding.construct('Vault');

  double deposit(String player, double amount) {
    final hash = steelMethodHash(
      'Vault::deposit(String,double)->double',
    );
    return binding
        .ptrcall(
          'Vault',
          'deposit',
          hash,
          _vault,
          <SteelArg>[SteelArg.string(player), SteelArg.f64(amount)],
          SteelReturnKind.f64,
        )
        .asDouble();
  }

  double balance(String player) {
    final hash = steelMethodHash('Vault::balance(String)->double');
    return binding
        .ptrcall(
          'Vault',
          'balance',
          hash,
          _vault,
          <SteelArg>[SteelArg.string(player)],
          SteelReturnKind.f64,
        )
        .asDouble();
  }
}
