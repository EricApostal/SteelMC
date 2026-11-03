import 'dart:convert';
import 'dart:ffi';

import 'package:ffi/ffi.dart';

import '../bindings.g.dart';
import 'ffi_api.dart';
import 'model.dart';

export '../bindings.g.dart' show SteelConstants, SteelRef;
export 'ffi_api.dart' show SteelHostException;

/// Native representation of one host method argument.
enum SteelArgKind {
  i32,
  u32,
  f32,
  f64,
  boolean,
  string,
  vec3,
  ref,
}

/// One argument passed to a host method.
///
/// Generated code builds these values; the transport encodes them into the
/// native layout expected by the ClassDB thunk.
sealed class SteelArg {
  const SteelArg();

  /// A signed 32-bit integer.
  factory SteelArg.i32(int value) = I32Arg;

  /// An unsigned 32-bit integer.
  factory SteelArg.u32(int value) = U32Arg;

  /// A 32-bit floating point number.
  factory SteelArg.f32(double value) = F32Arg;

  /// A 64-bit floating point number.
  factory SteelArg.f64(double value) = F64Arg;

  /// A boolean.
  factory SteelArg.boolean(bool value) = BoolArg;

  /// A borrowed UTF-8 string.
  factory SteelArg.string(String value) = StringArg;

  /// A position.
  factory SteelArg.vec3(Vec3 value) = Vec3Arg;

  /// A host handle.
  factory SteelArg.ref(SteelRef value) = RefArg;

  /// Discriminator of this argument's native type.
  SteelArgKind get kind;

  /// The Dart value carried by this argument.
  Object get value;
}

final class I32Arg extends SteelArg {
  const I32Arg(this.value);

  @override
  final int value;

  @override
  SteelArgKind get kind => SteelArgKind.i32;
}

final class U32Arg extends SteelArg {
  const U32Arg(this.value);

  @override
  final int value;

  @override
  SteelArgKind get kind => SteelArgKind.u32;
}

final class F32Arg extends SteelArg {
  const F32Arg(this.value);

  @override
  final double value;

  @override
  SteelArgKind get kind => SteelArgKind.f32;
}

final class F64Arg extends SteelArg {
  const F64Arg(this.value);

  @override
  final double value;

  @override
  SteelArgKind get kind => SteelArgKind.f64;
}

final class BoolArg extends SteelArg {
  const BoolArg(this.value);

  @override
  final bool value;

  @override
  SteelArgKind get kind => SteelArgKind.boolean;
}

final class StringArg extends SteelArg {
  const StringArg(this.value);

  @override
  final String value;

  @override
  SteelArgKind get kind => SteelArgKind.string;
}

final class Vec3Arg extends SteelArg {
  const Vec3Arg(this.value);

  @override
  final Vec3 value;

  @override
  SteelArgKind get kind => SteelArgKind.vec3;
}

final class RefArg extends SteelArg {
  const RefArg(this.value);

  @override
  final SteelRef value;

  @override
  SteelArgKind get kind => SteelArgKind.ref;
}

/// Native type of a host method's return value.
enum SteelReturnKind {
  nothing,
  i32,
  u32,
  f32,
  f64,
  boolean,
  string,
  vec3,
  ref,
}

/// A decoded host method result.
///
/// The transport copies the native result into Dart memory before the call
/// arena is released, so a [SteelReturn] outlives the call.
final class SteelReturn {
  const SteelReturn(this._value);

  final Object? _value;

  /// The result as a signed integer.
  int asInt() => _value! as int;

  /// The result as a floating point number.
  double asDouble() => _value! as double;

  /// The result as a boolean.
  bool asBool() => _value! as bool;

  /// The result as an owned string.
  String asString() => _value! as String;

  /// The result as a position.
  Vec3 asVec3() => _value! as Vec3;

  /// The result as a host handle.
  SteelRef asRef() => _value! as SteelRef;
}

/// Transport used by the generated host API for every method call.
abstract interface class SteelBinding {
  /// Calls `className.methodName` through its ClassDB MethodBind.
  ///
  /// [instance] is the receiver handle for instance methods and
  /// [steelNoneRef] for static methods. Non-zero host statuses are reported
  /// with a [SteelHostException].
  SteelReturn ptrcall(
    String className,
    String methodName,
    int hash,
    SteelRef instance,
    List<SteelArg> args,
    SteelReturnKind returnKind,
  );

  /// Constructs an extension-class instance and returns its handle.
  SteelRef construct(String className, [List<SteelArg> args = const <SteelArg>[]]);
}

/// Creates a host reference with explicit fields.
SteelRef steelRef(int kind, int index, [int generation = 0]) {
  final reference = Struct.create<SteelRef>();
  reference.kind = kind;
  reference.index = index;
  reference.generation = generation;
  return reference;
}

/// An empty reference, used for static method receivers and invalid results.
SteelRef steelNoneRef() => steelRef(SteelConstants.steelRefNone, 0);

/// A world handle.
SteelRef steelWorldRef(int id) => steelRef(SteelConstants.steelRefWorld, id);

/// An entity handle: the entity id lives in the generation field.
SteelRef steelEntityRef(int world, int entity) =>
    steelRef(SteelConstants.steelRefEntity, world, entity);

/// A registry block handle.
SteelRef steelBlockRef(int id) => steelRef(SteelConstants.steelRefBlock, id);

/// A registry block state handle.
SteelRef steelBlockStateRef(int id) =>
    steelRef(SteelConstants.steelRefBlockState, id);

/// A registry item handle.
SteelRef steelItemRef(int id) => steelRef(SteelConstants.steelRefItem, id);

/// [SteelBinding] backed by the in-process `SteelInterface` table.
final class NativeSteelBinding implements SteelBinding {
  NativeSteelBinding(this._ffi);

  /// Opens the ClassDB entry points published by the host interface.
  factory NativeSteelBinding.fromAddress(int address) {
    if (address == 0) {
      throw SteelHostException('Steel interface address is not set');
    }
    return NativeSteelBinding(
      SteelFfi(Pointer<SteelInterface>.fromAddress(address)),
    );
  }

  final SteelFfi _ffi;
  final Map<(String, String, int), Pointer<SteelMethodBind>> _binds =
      <(String, String, int), Pointer<SteelMethodBind>>{};

  Pointer<SteelMethodBind> _methodBind(
    String className,
    String methodName,
    int hash,
  ) {
    final key = (className, methodName, hash);
    final cached = _binds[key];
    if (cached != null) {
      return cached;
    }
    final bind = using((Arena arena) {
      final classView = _stringView(arena, className);
      final methodView = _stringView(arena, methodName);
      return _ffi.classdbGetMethodBind(classView, methodView, hash);
    });
    if (bind == nullptr) {
      throw SteelHostException(
        'host has no method bind for $className.$methodName ($hash)',
      );
    }
    _binds[key] = bind;
    return bind;
  }

  @override
  SteelReturn ptrcall(
    String className,
    String methodName,
    int hash,
    SteelRef instance,
    List<SteelArg> args,
    SteelReturnKind returnKind,
  ) {
    final bind = _methodBind(className, methodName, hash);
    return using((Arena arena) {
      final encoded = <Pointer<Void>>[
        for (final arg in args) _encodeArg(arena, arg),
      ];
      Pointer<Pointer<Void>> arguments = nullptr;
      if (encoded.isNotEmpty) {
        arguments = arena<Pointer<Void>>(encoded.length);
        for (var index = 0; index < encoded.length; index++) {
          arguments[index] = encoded[index];
        }
      }
      final result = _allocateReturn(arena, returnKind);
      final status = _ffi.methodBindPtrcall(
        bind,
        instance,
        encoded.isEmpty ? nullptr : arguments,
        result,
      );
      if (status != SteelConstants.steelOk) {
        throw SteelHostException(
          '$className.$methodName failed with status $status',
        );
      }
      return _decodeReturn(returnKind, result);
    });
  }

  @override
  SteelRef construct(String className, [List<SteelArg> args = const <SteelArg>[]]) {
    return using((Arena arena) {
      final classView = _stringView(arena, className);
      final encoded = <Pointer<Void>>[
        for (final arg in args) _encodeArg(arena, arg),
      ];
      Pointer<Pointer<Void>> arguments = nullptr;
      if (encoded.isNotEmpty) {
        arguments = arena<Pointer<Void>>(encoded.length);
        for (var index = 0; index < encoded.length; index++) {
          arguments[index] = encoded[index];
        }
      }
      final result = arena<SteelRef>();
      final status = _ffi.classdbConstruct(
        classView,
        encoded.isEmpty ? nullptr : arguments,
        result,
      );
      if (status != SteelConstants.steelOk) {
        throw SteelHostException('construct $className failed with status $status');
      }
      return steelRef(result.ref.kind, result.ref.index, result.ref.generation);
    });
  }
}

SteelStrView _stringView(Arena arena, String value) {
  final view = arena<SteelStrView>();
  final encoded = utf8.encode(value);
  if (encoded.isEmpty) {
    view.ref.ptr = nullptr;
    view.ref.len = 0;
  } else {
    final bytes = arena<Uint8>(encoded.length);
    bytes.asTypedList(encoded.length).setAll(0, encoded);
    view.ref.ptr = bytes;
    view.ref.len = encoded.length;
  }
  return view.ref;
}

Pointer<Void> _encodeArg(Arena arena, SteelArg arg) {
  switch (arg) {
    case I32Arg(:final value):
      final pointer = arena<Int32>();
      pointer.value = value;
      return pointer.cast();
    case U32Arg(:final value):
      final pointer = arena<Uint32>();
      pointer.value = value;
      return pointer.cast();
    case F32Arg(:final value):
      final pointer = arena<Float>();
      pointer.value = value;
      return pointer.cast();
    case F64Arg(:final value):
      final pointer = arena<Double>();
      pointer.value = value;
      return pointer.cast();
    case BoolArg(:final value):
      final pointer = arena<Uint8>();
      pointer.value = value ? 1 : 0;
      return pointer.cast();
    case StringArg(:final value):
      final pointer = arena<SteelStrView>();
      pointer.ref = _stringView(arena, value);
      return pointer.cast();
    case Vec3Arg(:final value):
      final pointer = arena<SteelVec3>();
      pointer.ref.x = value.x;
      pointer.ref.y = value.y;
      pointer.ref.z = value.z;
      return pointer.cast();
    case RefArg(:final value):
      final pointer = arena<SteelRef>();
      pointer.ref.kind = value.kind;
      pointer.ref.index = value.index;
      pointer.ref.generation = value.generation;
      return pointer.cast();
  }
}

Pointer<Void> _allocateReturn(Arena arena, SteelReturnKind kind) {
  switch (kind) {
    case SteelReturnKind.nothing:
      return nullptr;
    case SteelReturnKind.i32:
      return arena<Int32>().cast();
    case SteelReturnKind.u32:
      return arena<Uint32>().cast();
    case SteelReturnKind.f32:
      return arena<Float>().cast();
    case SteelReturnKind.f64:
      return arena<Double>().cast();
    case SteelReturnKind.boolean:
      return arena<Uint8>().cast();
    case SteelReturnKind.string:
      return arena<SteelStrView>().cast();
    case SteelReturnKind.vec3:
      return arena<SteelVec3>().cast();
    case SteelReturnKind.ref:
      return arena<SteelRef>().cast();
  }
}

SteelReturn _decodeReturn(SteelReturnKind kind, Pointer<Void> result) {
  switch (kind) {
    case SteelReturnKind.nothing:
      return const SteelReturn(null);
    case SteelReturnKind.i32:
      return SteelReturn(result.cast<Int32>().value);
    case SteelReturnKind.u32:
      return SteelReturn(result.cast<Uint32>().value);
    case SteelReturnKind.f32:
      return SteelReturn(result.cast<Float>().value);
    case SteelReturnKind.f64:
      return SteelReturn(result.cast<Double>().value);
    case SteelReturnKind.boolean:
      return SteelReturn(result.cast<Uint8>().value != 0);
    case SteelReturnKind.string:
      final view = result.cast<SteelStrView>().ref;
      if (view.ptr == nullptr || view.len == 0) {
        return const SteelReturn('');
      }
      return SteelReturn(
        view.ptr.cast<Utf8>().toDartString(length: view.len),
      );
    case SteelReturnKind.vec3:
      final value = result.cast<SteelVec3>().ref;
      return SteelReturn(Vec3(value.x, value.y, value.z));
    case SteelReturnKind.ref:
      final value = result.cast<SteelRef>().ref;
      return SteelReturn(steelRef(value.kind, value.index, value.generation));
  }
}