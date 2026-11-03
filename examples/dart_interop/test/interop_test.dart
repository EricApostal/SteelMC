import 'package:steel_interop/vault_client.dart';
import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

final class FakeBinding implements SteelBinding {
  final List<(String, String, int, SteelRef, List<SteelArg>)> calls =
      <(String, String, int, SteelRef, List<SteelArg>)>[];

  @override
  SteelRef construct(String className, [List<SteelArg> args = const <SteelArg>[]]) =>
      steelRef(11, 7, 1);

  @override
  SteelReturn ptrcall(
    String className,
    String methodName,
    int hash,
    SteelRef instance,
    List<SteelArg> args,
    SteelReturnKind returnKind,
  ) {
    calls.add((className, methodName, hash, instance, args));
    return SteelReturn(15.5);
  }
}

void main() {
  test('client calls the vault class through ClassDB', () {
    final binding = FakeBinding();
    final vault = VaultClient(binding);

    expect(vault.deposit('Steve', 15.5), 15.5);
    final call = binding.calls.single;
    expect(call.$1, 'Vault');
    expect(call.$2, 'deposit');
    expect(call.$4.kind, 11);
    expect(call.$5.first.value, 'Steve');
    expect(call.$5.last.value, 15.5);
  });
}
