import 'dart:convert';
import 'dart:ffi';
import 'dart:io';

import 'package:steel_extension/steel.dart';
import 'package:test/test.dart';

/// Locates the compiled `steel-abi` library.
///
/// Set `STEEL_ABI_LIB` to an explicit path, otherwise the usual Cargo output
/// locations are checked. The test suite is skipped when no library exists so
/// that a checkout without a Rust build does not fail.
String? _findLibrary() {
  final explicit = Platform.environment['STEEL_ABI_LIB'];
  if (explicit != null && explicit.isNotEmpty) {
    return File(explicit).existsSync() ? explicit : null;
  }

  final suffix = Platform.isWindows
      ? '.dll'
      : Platform.isMacOS
          ? '.dylib'
          : '.so';
  final fileName = 'libsteel_abi$suffix';
  final candidates = [
    'target/debug/$fileName',
    'target/release/$fileName',
    '../../target/debug/$fileName',
    '../../target/release/$fileName',
  ];
  for (final candidate in candidates) {
    if (File(candidate).existsSync()) {
      return File(candidate).absolute.path;
    }
  }
  return null;
}

final String? _libraryPath = _findLibrary();
final Object? _skip = _libraryPath == null
    ? 'steel-abi library not built (run `cargo build -p steel-abi`)'
    : false;

final DynamicLibrary _process = DynamicLibrary.process();

typedef _MallocNative = Pointer<Uint8> Function(Size);
typedef _MallocDart = Pointer<Uint8> Function(int);

typedef _FreeNative = Void Function(Pointer<Uint8>);
typedef _FreeDart = void Function(Pointer<Uint8>);

Pointer<Uint8> _malloc(int length) =>
    _process.lookupFunction<_MallocNative, _MallocDart>('malloc')(length);

void _free(Pointer<Uint8> pointer) =>
    _process.lookupFunction<_FreeNative, _FreeDart>('free')(pointer);

SteelBindings _bindings() => SteelBindings.open(DynamicLibrary.open(_libraryPath!));

void main() {
  test('steel_abi_version reports the ABI version', () {
    expect(_bindings().apiVersion(), 1);
    expect(SteelConstants.steelApiVersion, 1);
  }, skip: _skip);

  test('steel_abi_test_add adds two integers', () {
    expect(_bindings().testAdd(2, 3), 5);
  }, skip: _skip);

  test('SteelStrView round-trips through steel_abi_test_len', () {
    final bindings = _bindings();
    final message = 'steel-abi';
    final encoded = utf8.encode(message);

    final bytes = _malloc(encoded.length);
    final view = _malloc(sizeOf<SteelStrView>()).cast<SteelStrView>();
    try {
      bytes.asTypedList(encoded.length).setAll(0, encoded);
      view.ref.ptr = bytes;
      view.ref.len = encoded.length;

      expect(bindings.testLen(view.ref), encoded.length);
      expect(steelStrViewToString(view.ref), message);
    } finally {
      _free(bytes);
      _free(view.cast<Uint8>());
    }
  }, skip: _skip);
}