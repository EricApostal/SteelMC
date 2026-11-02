/// Dart bindings for the Steel extension ABI.
library;

import 'dart:convert';
import 'dart:ffi';

import 'src/abi.dart';
import 'src/bindings.g.dart';

export 'src/abi.dart';
export 'src/bindings.g.dart';

/// Decodes the borrowed UTF-8 string described by [view].
///
/// The bytes are copied into a Dart `Uint8List` before decoding, so the
/// returned string owns its data and remains valid after the native view is
/// invalidated by the host.
String steelStrViewToString(SteelStrView view) {
  if (view.ptr == nullptr || view.len == 0) {
    return '';
  }
  final bytes = view.ptr.cast<Utf8>().asUtf8Bytes(view.len);
  return utf8.decode(bytes);
}