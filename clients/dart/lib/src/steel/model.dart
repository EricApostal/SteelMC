/// Small value types shared by the plugin runtime and generated host API.
library;

/// A position in a world.
final class Vec3 {
  const Vec3(this.x, this.y, this.z);

  final double x;
  final double y;
  final double z;

  @override
  bool operator ==(Object other) =>
      other is Vec3 && other.x == x && other.y == y && other.z == z;

  @override
  int get hashCode => Object.hash(x, y, z);

  @override
  String toString() => 'Vec3($x, $y, $z)';
}

/// Severity of a message written to the host log.
enum LogLevel {
  debug,
  info,
  warn,
  error,
}

/// A command announced to the host.
final class CommandRegistration {
  const CommandRegistration({
    required this.name,
    this.description = '',
    this.permission,
  });

  /// Name without the leading slash, for example `home`.
  final String name;

  /// Help text shown by the host.
  final String description;

  /// Permission required to run the command, or `null` when unrestricted.
  final String? permission;
}

/// Event kind discriminators shared with the native bridge.
abstract final class EventKind {
  static const int blockBreak = 1;
  static const int blockPlace = 2;
  static const int playerJoin = 3;
  static const int playerLeave = 4;
  static const int chat = 5;

  static const Map<String, int> byName = <String, int>{
    'block_break': blockBreak,
    'block_place': blockPlace,
    'player_join': playerJoin,
    'player_leave': playerLeave,
    'chat': chat,
  };
}