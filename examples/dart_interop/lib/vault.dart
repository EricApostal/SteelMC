import 'package:steel_extension/annotations.dart';

part 'vault.steel.dart';

/// Provider-side economy service registered in the host ClassDB.
@SteelClass('Vault')
final class Vault {
  final Map<String, double> _balances = <String, double>{};

  /// Adds [amount] to [player]'s balance and returns the new balance.
  @SteelMethod()
  double deposit(String player, double amount) {
    final balance = (_balances[player] ?? 0) + amount;
    _balances[player] = balance;
    return balance;
  }

  /// Returns [player]'s current balance.
  @SteelMethod()
  double balance(String player) => _balances[player] ?? 0;
}
