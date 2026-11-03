import 'dart:convert';

import 'ffi_api.dart';
import 'native_shim.dart';

/// Native representation of one extension method value.
///
/// The ordinals are the type codes exchanged with the native shim and must stay
/// in sync with the `STEEL_VALUE_*` constants in `steel-dart-embed`.
///
/// `void` cannot be an enum constant in Dart, so the empty type is exposed as
/// [voidType].
enum SteelValueType {
  /// A signed 32-bit integer.
  i32,

  /// An unsigned 32-bit integer.
  u32,

  /// A 32-bit floating point number.
  f32,

  /// A 64-bit floating point number.
  f64,

  /// A boolean, passed as one byte over the ABI.
  boolean,

  /// A borrowed UTF-8 string.
  string,

  /// A host handle, encoded as `[kind, index, generation]`.
  ref,

  /// No value; only valid as a return type.
  voidType,
}

/// Calls one extension method on a constructed instance.
///
/// [args] holds decoded Dart values in declaration order: `int` for `i32` and
/// `u32`, `double` for `f32` and `f64`, `bool`, `String`, and `List<int>` for
/// `ref` handles.
typedef SteelMethodInvoker = Object? Function(
  Object instance,
  List<Object?> args,
);

/// One method registered with the host ClassDB for an extension class.
final class SteelMethodRegistration {
  const SteelMethodRegistration({
    required this.name,
    required this.hash,
    required this.argTypes,
    required this.returnType,
    required this.invoke,
  });

  /// ClassDB method name.
  final String name;

  /// Stable method hash; see [steelMethodHash].
  final int hash;

  /// Value types of the positional parameters, in order.
  final List<SteelValueType> argTypes;

  /// Value type of the result.
  final SteelValueType returnType;

  /// Invokes the Dart implementation.
  final SteelMethodInvoker invoke;
}

/// One extension class registered with the host ClassDB.
final class SteelClassRegistration {
  const SteelClassRegistration({
    required this.name,
    required this.inherits,
    required this.construct,
    this.methods = const <SteelMethodRegistration>[],
  });

  /// ClassDB class name.
  final String name;

  /// ClassDB base class used for method lookup fallback.
  final String inherits;

  /// Creates a new instance when the host constructs the class.
  final Object Function() construct;

  /// Methods exposed through MethodBinds.
  final List<SteelMethodRegistration> methods;
}

const int _fnvOffsetBasis = 0x811c9dc5;
const int _fnvPrime = 0x01000193;

/// Computes the FNV-1a 32-bit hash of a method signature.
///
/// The generator hashes `Class::method(ArgType,...)->ReturnType` so a method
/// keeps the same hash when unrelated methods change.
int steelMethodHash(String signature) {
  var hash = _fnvOffsetBasis;
  for (final byte in utf8.encode(signature)) {
    hash = ((hash ^ byte) * _fnvPrime) & 0xffffffff;
  }
  return hash;
}

final Map<int, Object> _instances = <int, Object>{};
final Map<int, SteelMethodRegistration> _methods =
    <int, SteelMethodRegistration>{};
final Map<int, SteelClassRegistration> _classes =
    <int, SteelClassRegistration>{};
final Map<(int, String), int> _methodIds = <(int, String), int>{};

int _nextInstanceId = 1;
int _nextMethodId = 0;
int _nextClassId = 0;

/// Registers [classes] with the host through [shim].
///
/// `runSteelPlugin` calls this before the plugin's `onEnable`. The native shim
/// queues the metadata and forwards it to the host ClassDB when the extension
/// reaches the behavior phase.
///
/// Returns the ids assigned to the registered classes. The ids are also the
/// `class_id` values the host passes back to `steelConstructExtension`.
List<int> registerExtensionClasses(
  List<SteelClassRegistration> classes,
  NativeShim shim,
) {
  final ids = <int>[];
  for (final registration in classes) {
    final classId = _nextClassId++;
    final methodNames = <String>[];
    final methodHashes = <int>[];
    final argCounts = <int>[];
    final argTypes = <int>[];
    final returnTypes = <int>[];
    final methodBase = _nextMethodId;
    for (var index = 0; index < registration.methods.length; index++) {
      final method = registration.methods[index];
      if (method.argTypes.contains(SteelValueType.voidType)) {
        throw ArgumentError.value(
          method.name,
          'classes',
          'a parameter cannot have type void',
        );
      }
      _methods[methodBase + index] = method;
      _methodIds[(classId, method.name)] = methodBase + index;
      methodNames.add(method.name);
      methodHashes.add(method.hash);
      argCounts.add(method.argTypes.length);
      for (final type in method.argTypes) {
        argTypes.add(type.index);
      }
      returnTypes.add(method.returnType.index);
    }
    _nextMethodId = methodBase + registration.methods.length;

    final status = shim.registerClass(
      name: registration.name,
      inherits: registration.inherits,
      methodNames: methodNames,
      methodHashes: methodHashes,
      argCounts: argCounts,
      argTypes: argTypes,
      returnTypes: returnTypes,
    );
    if (status != 0) {
      throw SteelHostException(
        'failed to register extension class ${registration.name} '
        'with the native shim ($status)',
      );
    }
    _classes[classId] = registration;
    ids.add(classId);
  }
  return ids;
}

/// Returns the method id assigned to [className.methodName], if registered.
///
/// Useful for tests and diagnostics; the host uses the MethodBind instead.
int? steelExtensionMethodId(String className, String methodName) {
  final classId = steelExtensionClassId(className);
  if (classId == null) {
    return null;
  }
  return _methodIds[(classId, methodName)];
}

/// Returns the class id assigned to [name], if registered.
int? steelExtensionClassId(String name) {
  for (final entry in _classes.entries) {
    if (entry.value.name == name) {
      return entry.key;
    }
  }
  return null;
}

/// Constructs an extension-class instance for the native shim.
///
/// Invoked by the shim on a host thread after acquiring the Dart isolate. The
/// returned id is the `index` of the `SteelRef` the host receives.
@pragma('vm:entry-point')
int steelConstructExtension(int classId) {
  final registration = _classes[classId];
  if (registration == null) {
    throw StateError('unknown Steel extension class id $classId');
  }
  final instanceId = _nextInstanceId++;
  _instances[instanceId] = registration.construct();
  return instanceId;
}

/// Invokes an extension method for the native shim.
///
/// Invoked by the shim on a host thread after acquiring the Dart isolate.
/// [args] contains the values decoded from the MethodBind ptrcall.
@pragma('vm:entry-point')
Object? steelInvokeExtension(
  int methodId,
  int instanceId,
  List<Object?> args,
) {
  final registration = _methods[methodId];
  if (registration == null) {
    throw StateError('unknown Steel extension method id $methodId');
  }
  final instance = _instances[instanceId];
  if (instance == null) {
    throw StateError('unknown Steel extension instance id $instanceId');
  }
  if (args.length != registration.argTypes.length) {
    throw ArgumentError.value(
      args,
      'args',
      'expected ${registration.argTypes.length} arguments for '
          '${registration.name}',
    );
  }
  return registration.invoke(instance, args);
}