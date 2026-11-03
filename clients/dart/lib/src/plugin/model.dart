import 'generated_api.dart';

export 'generated_api.dart';

/// Severity of a message written to the host log.
enum LogLevel {
  debug,
  info,
  warn,
  error,
}

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

/// The host operations available to a plugin.
///
/// The embedded runtime implements this with direct calls through the
/// `SteelInterface` C function table. Tests implement it with canned replies.
abstract interface class PluginHost {
  /// Returns a player snapshot, or `null` if the entity is not online.
  Player? playerData(int world, int entity);

  /// Returns an entity's current position.
  Vec3 entityPosition(int world, int entity);

  /// Teleports an entity.
  void entityTeleport(
    int world,
    int entity,
    Vec3 position, {
    double yaw = 0,
    double pitch = 0,
  });

  /// Sends a chat message to a player.
  void playerSendMessage(int world, int entity, String message);

  /// Returns an entity's health.
  double entityHealth(int world, int entity);

  /// Sets an entity's health.
  void entitySetHealth(int world, int entity, double health);

  /// Returns a world's spawn position.
  Vec3 worldSpawn(int world);

  /// Returns every player in a world.
  List<Player> worldPlayers(int world);

  /// Finds an online player by name in a world.
  Player? findPlayerInWorld(int world, String name);

  /// Writes a message to the host log.
  void log(String message, {LogLevel level = LogLevel.info});

  /// Registers a command handler with the host.
  void registerCommand(int id, CommandRegistration command);

  /// Registers interest in an event kind.
  void registerEvent(int kind);
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

/// Base class for server events delivered to a plugin.
abstract base class PluginEvent {
  PluginEvent({
    required this.id,
    required this.world,
    required this.position,
    this.player,
  });

  final int id;
  final int world;
  final Vec3 position;
  final Player? player;

  bool _cancelled = false;

  bool get isCancelled => _cancelled;

  void cancel() {
    _cancelled = true;
  }
}

/// A chat message sent by a player.
final class ChatEvent extends PluginEvent {
  ChatEvent({
    required super.id,
    required super.world,
    required super.position,
    super.player,
    required this.text,
  });

  final String text;

  @override
  String toString() => 'ChatEvent(${player?.name ?? 'server'}: $text)';
}

/// A block being placed or broken.
final class BlockEvent extends PluginEvent {
  BlockEvent({
    required super.id,
    required super.world,
    required super.position,
    super.player,
    required this.blockState,
  });

  final int blockState;

  @override
  String toString() =>
      'BlockEvent($blockState at $position, player: ${player?.name})';
}

/// Context passed to a command handler.
final class CommandContext {
  const CommandContext({
    required this.id,
    required this.command,
    required this.args,
    required this.player,
    required this.host,
  });

  final int id;
  final String command;
  final String args;
  final Player? player;
  final PluginHost host;

  List<String> get arguments {
    final trimmed = args.trim();
    if (trimmed.isEmpty) {
      return const <String>[];
    }
    return trimmed.split(RegExp(r'\s+'));
  }

  @override
  String toString() => 'CommandContext($command, args: "$args")';
}
