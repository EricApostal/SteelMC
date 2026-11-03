/// Annotations for Dart extension classes.
///
/// Annotate a class with [SteelClass] and its instance methods with
/// [SteelMethod], add `part '<file>.steel.dart';` and import this library, then
/// run build_runner (`dart run build_runner build`) to generate the
/// `SteelClassRegistration` list:
///
/// ```dart
/// import 'package:steel_extension/annotations.dart';
///
/// part 'greeter.steel.dart';
///
/// @SteelClass('Greeter')
/// final class Greeter {
///   @SteelMethod()
///   String greet(String name) => 'Hello, $name!';
/// }
/// ```
///
/// Pass the generated `steelClasses` to `runSteelPlugin`:
///
/// ```dart
/// void main() => runSteelPlugin(MyPlugin(), classes: steelClasses);
/// ```
///
/// The native host registers the classes with its ClassDB, so other extensions
/// and native code can call them through MethodBinds.
library;

export 'src/steel/extension.dart';

/// Declares a class as a host extension class.
///
/// [name] is the class name used by the host ClassDB; it does not have to match
/// the Dart class name. [inherits] names the host class whose methods the
/// extension class should also resolve, or `Object` when none.
final class SteelClass {
  const SteelClass(this.name, {this.inherits = 'Object'});

  /// The ClassDB name of the class.
  final String name;

  /// The ClassDB name this class inherits method lookups from.
  final String inherits;
}

/// Declares an instance method as a callable extension method.
///
/// The MethodBind name defaults to the Dart method name. Parameters must be
/// positional and have a supported type (`int`, `double`, `bool`, `String`);
/// the method must return `int`, `double`, `bool`, `String` or `void`.
final class SteelMethod {
  const SteelMethod({this.name});

  /// The ClassDB name of the method; defaults to the Dart method name.
  final String? name;
}

/// Declares a field or getter as extension-class metadata.
///
/// Properties are recorded for tooling but are not registered as MethodBinds
/// yet; expose a getter annotated with [SteelMethod] to make a value reachable
/// from the host.
final class SteelProperty {
  const SteelProperty({this.name});

  /// The name recorded for the property; defaults to the Dart member name.
  final String? name;
}