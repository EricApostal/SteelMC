/// Internal helpers for working with decoded JSON values.
///
/// The wire protocol is intentionally untyped, so the public API funnels
/// decoded values through these functions once instead of spreading `as` casts
/// through the package.
library;

/// Returns [value] as a JSON object.
///
/// Throws a [FormatException] mentioning [context] when [value] is not a map.
Map<String, Object?> asJsonObject(Object? value, {String context = 'value'}) {
  if (value is Map<String, Object?>) {
    return value;
  }
  if (value is Map) {
    return <String, Object?>{
      for (final entry in value.entries) entry.key.toString(): entry.value,
    };
  }
  throw FormatException(
    'Expected $context to be a JSON object, got ${value.runtimeType}',
  );
}

/// Returns [value] as a JSON array.
///
/// Throws a [FormatException] mentioning [context] when [value] is not a list.
List<Object?> asJsonList(Object? value, {String context = 'value'}) {
  if (value is List<Object?>) {
    return value;
  }
  if (value is List) {
    return List<Object?>.of(value);
  }
  throw FormatException(
    'Expected $context to be a JSON array, got ${value.runtimeType}',
  );
}
