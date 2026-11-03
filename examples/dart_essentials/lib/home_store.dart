import 'dart:convert';
import 'dart:io';

import 'package:steel_extension/steel_plugin.dart';

/// JSON-backed storage for player homes.
///
/// The file maps player UUIDs to home names and positions:
///
/// ```json
/// {
///   "uuid-steve": {
///     "home": {"x": 0.0, "y": 64.0, "z": 0.0}
///   }
/// }
/// ```
///
/// [set] and [remove] write the file immediately, so a crash never loses a home.
/// Call [load] once before using the store; a missing file is treated as empty.
final class HomeStore {
  HomeStore({required this.path});

  /// Location of the backing JSON file.
  final String path;

  final Map<String, Map<String, Vec3>> _homes = <String, Map<String, Vec3>>{};

  /// Replaces the in-memory homes with the contents of [path].
  ///
  /// A missing or empty file leaves the store empty.
  void load() {
    _homes.clear();
    final file = File(path);
    if (!file.existsSync()) {
      return;
    }
    final contents = file.readAsStringSync();
    if (contents.trim().isEmpty) {
      return;
    }
    final decoded = jsonDecode(contents);
    if (decoded is! Map<String, Object?>) {
      throw FormatException('$path does not contain a JSON object');
    }
    for (final entry in decoded.entries) {
      final rawHomes = entry.value;
      if (rawHomes is! Map) {
        continue;
      }
      final homes = <String, Vec3>{};
      for (final rawHome in rawHomes.entries) {
        final rawPosition = rawHome.value;
        if (rawPosition is! Map) {
          continue;
        }
        final x = rawPosition['x'];
        final y = rawPosition['y'];
        final z = rawPosition['z'];
        if (x is num && y is num && z is num) {
          homes[rawHome.key.toString()] =
              Vec3(x.toDouble(), y.toDouble(), z.toDouble());
        }
      }
      _homes[entry.key] = homes;
    }
  }

  /// Writes every home to [path], creating the parent directory if needed.
  void save() {
    final file = File(path);
    final parent = file.parent;
    if (!parent.existsSync()) {
      parent.createSync(recursive: true);
    }
    final uuids = _homes.keys.toList()..sort();
    final encoded = <String, Object?>{};
    for (final uuid in uuids) {
      final homes = _homes[uuid]!;
      final names = homes.keys.toList()..sort();
      encoded[uuid] = <String, Object?>{
        for (final name in names)
          name: <String, Object?>{
            'x': homes[name]!.x,
            'y': homes[name]!.y,
            'z': homes[name]!.z,
          },
      };
    }
    const encoder = JsonEncoder.withIndent('  ');
    file.writeAsStringSync('${encoder.convert(encoded)}\n');
  }

  /// Returns an unmodifiable view of every home owned by [uuid].
  Map<String, Vec3> homesOf(String uuid) => Map<String, Vec3>.unmodifiable(
        _homes[uuid] ?? const <String, Vec3>{},
      );

  /// Returns the home named [name] for [uuid], or `null`.
  Vec3? get(String uuid, String name) => _homes[uuid]?[name];

  /// Stores [position] as the home named [name] for [uuid] and saves.
  void set(String uuid, String name, Vec3 position) {
    _homes.putIfAbsent(uuid, () => <String, Vec3>{})[name] = position;
    save();
  }

  /// Removes the home named [name] for [uuid] and saves.
  ///
  /// Returns whether a home existed.
  bool remove(String uuid, String name) {
    final homes = _homes[uuid];
    if (homes == null || !homes.containsKey(name)) {
      return false;
    }
    homes.remove(name);
    if (homes.isEmpty) {
      _homes.remove(uuid);
    }
    save();
    return true;
  }

  /// Returns the sorted home names owned by [uuid].
  List<String> namesOf(String uuid) {
    final homes = _homes[uuid];
    if (homes == null) {
      return <String>[];
    }
    return homes.keys.toList()..sort();
  }
}
