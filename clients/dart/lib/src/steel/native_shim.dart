import 'dart:ffi';
import 'dart:io';

import 'package:ffi/ffi.dart';

import '../bindings.g.dart';
import 'ffi_api.dart';

typedef _ShimRegisterCommandNative = Int32 Function(
  Uint64,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _ShimRegisterCommandDart = int Function(
  int,
  Pointer<Utf8>,
  Pointer<Utf8>,
  Pointer<Utf8>,
);
typedef _ShimRegisterEventNative = Int32 Function(Uint32);
typedef _ShimRegisterEventDart = int Function(int);
typedef _ShimLogNative = Int32 Function(Uint32, Pointer<Utf8>);
typedef _ShimLogDart = int Function(int, Pointer<Utf8>);
typedef _ShimRegisterClassNative = Int32 Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  Size,
  Pointer<Pointer<Utf8>>,
  Pointer<Uint32>,
  Pointer<Uint8>,
  Pointer<Uint8>,
  Pointer<Uint8>,
);
typedef _ShimRegisterClassDart = int Function(
  Pointer<Utf8>,
  Pointer<Utf8>,
  int,
  Pointer<Pointer<Utf8>>,
  Pointer<Uint32>,
  Pointer<Uint8>,
  Pointer<Uint8>,
  Pointer<Uint8>,
);
typedef _ExtensionPathDart = SteelStrView Function();

/// Registration and logging hooks provided by the in-process Dart shim.
///
/// The shim queues command/event registrations and forwards them to the host
/// when the extension reaches the behavior phase. Tests implement this
/// interface to observe registrations without a native library.
abstract interface class NativeShim {
  /// Opens the shim library shipped next to the host extension.
  static NativeShim fromAddress(int address) =>
      _ProcessNativeShim.fromAddress(address);

  /// Queues a command registration and returns the host status.
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  );

  /// Queues an event subscription and returns the host status.
  int registerEvent(int kind);

  /// Queues an extension class registration and returns the host status.
  ///
  /// The metadata arrays describe [methodNames] in order:
  /// [methodHashes] holds one hash per method, [argCounts] the parameter count,
  /// [argTypes] the flattened `SteelValueType` codes, and [returnTypes] one
  /// `SteelValueType` code per method.
  int registerClass({
    required String name,
    required String inherits,
    required List<String> methodNames,
    required List<int> methodHashes,
    required List<int> argCounts,
    required List<int> argTypes,
    required List<int> returnTypes,
  });

  /// Writes a line through the host logger and returns the host status.
  int log(int level, String message);
}

final class _ProcessNativeShim implements NativeShim {
  _ProcessNativeShim._(this._library) {
    _registerCommand = _library.lookupFunction<
        _ShimRegisterCommandNative, _ShimRegisterCommandDart>(
      'steel_shim_register_command',
    );
    _registerEvent = _library.lookupFunction<
        _ShimRegisterEventNative, _ShimRegisterEventDart>(
      'steel_shim_register_event',
    );
    _log = _library.lookupFunction<_ShimLogNative, _ShimLogDart>(
      'steel_shim_log',
    );
  }

  factory _ProcessNativeShim.fromAddress(int address) {
    if (address == 0) {
      throw SteelHostException('Steel interface address is not set');
    }
    final path = _extensionPath(Pointer<SteelInterface>.fromAddress(address));
    final libraryPath =
        path.isNotEmpty ? path : Platform.environment['STEEL_DART_SHIM'];
    if (libraryPath == null || libraryPath.isEmpty) {
      throw SteelHostException(
        'extension_path is empty and STEEL_DART_SHIM is not set',
      );
    }
    return _ProcessNativeShim._(DynamicLibrary.open(libraryPath));
  }

  // Keeps the shim library loaded for the lifetime of the shim.
  // ignore: unused_field
  final DynamicLibrary _library;
  late final _ShimRegisterCommandDart _registerCommand;
  late final _ShimRegisterEventDart _registerEvent;
  late final _ShimLogDart _log;

  // Resolved lazily so plugins that do not declare extension classes keep
  // working with a shim that predates `steel_shim_register_class`.
  late final _ShimRegisterClassDart _registerClass = _library.lookupFunction<
      _ShimRegisterClassNative, _ShimRegisterClassDart>(
    'steel_shim_register_class',
  );

  @override
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  ) {
    final namePointer = name.toNativeUtf8();
    final descriptionPointer = description.toNativeUtf8();
    final permissionPointer = permission.toNativeUtf8();
    try {
      return _registerCommand(
        id,
        namePointer,
        descriptionPointer,
        permissionPointer,
      );
    } finally {
      malloc.free(namePointer);
      malloc.free(descriptionPointer);
      malloc.free(permissionPointer);
    }
  }

  @override
  int registerEvent(int kind) => _registerEvent(kind);

  @override
  int registerClass({
    required String name,
    required String inherits,
    required List<String> methodNames,
    required List<int> methodHashes,
    required List<int> argCounts,
    required List<int> argTypes,
    required List<int> returnTypes,
  }) {
    final methodCount = methodNames.length;
    if (methodHashes.length != methodCount ||
        argCounts.length != methodCount ||
        returnTypes.length != methodCount) {
      throw ArgumentError(
        'extension class metadata must have one entry per method',
      );
    }
    var argumentCount = 0;
    for (final count in argCounts) {
      if (count < 0) {
        throw ArgumentError.value(count, 'argCounts', 'must not be negative');
      }
      argumentCount += count;
    }
    if (argTypes.length != argumentCount) {
      throw ArgumentError.value(
        argTypes,
        'argTypes',
        'expected $argumentCount flattened type codes',
      );
    }
    return using((Arena arena) {
      final namePointer = name.toNativeUtf8(allocator: arena);
      final inheritsPointer = inherits.toNativeUtf8(allocator: arena);
      Pointer<Pointer<Utf8>> names = nullptr;
      if (methodCount > 0) {
        names = arena<Pointer<Utf8>>(methodCount);
        for (var index = 0; index < methodCount; index++) {
          names[index] = methodNames[index].toNativeUtf8(allocator: arena);
        }
      }
      final hashes = _allocateUint32(arena, methodHashes);
      final counts = _allocateUint8(arena, argCounts);
      final types = _allocateUint8(arena, argTypes);
      final returns = _allocateUint8(arena, returnTypes);
      return _registerClass(
        namePointer,
        inheritsPointer,
        methodCount,
        names,
        hashes,
        counts,
        types,
        returns,
      );
    });
  }

  @override
  int log(int level, String message) {
    final pointer = message.toNativeUtf8();
    try {
      return _log(level, pointer);
    } finally {
      malloc.free(pointer);
    }
  }
}

Pointer<Uint32> _allocateUint32(Arena arena, List<int> values) {
  if (values.isEmpty) {
    return nullptr;
  }
  final pointer = arena<Uint32>(values.length);
  pointer.asTypedList(values.length).setAll(0, values);
  return pointer;
}

Pointer<Uint8> _allocateUint8(Arena arena, List<int> values) {
  if (values.isEmpty) {
    return nullptr;
  }
  final pointer = arena<Uint8>(values.length);
  pointer.asTypedList(values.length).setAll(0, values);
  return pointer;
}

String _extensionPath(Pointer<SteelInterface> interface) {
  final function = interface.ref.extension_path;
  if (function == nullptr) {
    return '';
  }
  return _readView(function.asFunction<_ExtensionPathDart>()());
}

String _readView(SteelStrView view) {
  if (view.ptr == nullptr || view.len == 0) {
    return '';
  }
  return view.ptr.cast<Utf8>().toDartString(length: view.len);
}