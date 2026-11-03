import 'package:steel_extension/annotations.dart';

part 'greeter.steel.dart';

/// A host extension class implemented in Dart.
///
/// The generated [steelClasses] getter registers `Greeter` with the host
/// ClassDB, so native code and other extensions can construct it with
/// `classdb_construct` and call its methods through `classdb_get_method_bind`
/// plus `method_bind_ptrcall`.
@SteelClass('Greeter')
final class Greeter {
  Greeter();

  int _count = 0;

  /// Returns a greeting for [name] and counts the call.
  @SteelMethod()
  String greet(String name) {
    _count++;
    return 'Hello, $name!';
  }

  /// Number of greetings served by this instance.
  @SteelMethod()
  int count() => _count;

  /// Adds [amount] to the counter and returns the new value.
  @SteelMethod()
  int increment(int amount) => _count += amount;

  /// Halves [value]; exercises the `double` argument and return path.
  @SteelMethod()
  double half(double value) => value / 2;

  /// Inverts [value]; exercises the `bool` argument and return path.
  @SteelMethod()
  bool toggle(bool value) => !value;

  /// Resets the counter; exercises the `void` return path.
  @SteelMethod()
  void reset() => _count = 0;

  /// Whether the greeter accepts calls.
  ///
  /// Properties are metadata only; annotate a getter with `@SteelMethod` to
  /// make it reachable from the host.
  @SteelProperty()
  bool get enabled => true;
}