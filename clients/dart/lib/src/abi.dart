import 'dart:ffi';
import 'dart:typed_data';

/// Minimal stand-in for `package:ffi`'s `Utf8` type.
///
/// The bindings deliberately have no third-party dependencies, so the package
/// defines the opaque element type it needs locally.
final class Utf8 extends Opaque {}

/// Helpers for reading UTF-8 data exposed through a [Pointer] of [Utf8].
extension Utf8Pointer on Pointer<Utf8> {
  /// Views the first [length] bytes as a [Uint8List].
  Uint8List asUtf8Bytes(int length) => cast<Uint8>().asTypedList(length);
}