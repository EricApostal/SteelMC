import 'json.dart';

/// Severity of a message written to the host log.
enum LogLevel {
  /// Diagnostic detail, usually hidden by the host.
  debug,

  /// Normal informational output.
  info,

  /// Something unexpected that did not stop the plugin.
  warn,

  /// An operation failed.
  error,
}

/// A position in a world.
final class Vec3 {
  const Vec3(this.x, this.y, this.z);

  /// Decodes a position from a JSON object.
  ///
  /// Throws a [FormatException] when a component is missing or not numeric.
  factory Vec3.fromJson(
    Map<String, Object?> json, {
    String context = 'position',
  }) =>
      Vec3(
        _number(json, 'x', context),
        _number(json, 'y', context),
        _number(json, 'z', context),
      );

  /// Decodes a position from a decoded host reply.
  factory Vec3.fromResult(Object? result, {String context = 'position'}) =>
      Vec3.fromJson(asJsonObject(result, context: context), context: context);

  final double x;
  final double y;
  final double z;

  /// Encodes this position as a JSON object.
  Map<String, Object?> toJson() => <String, Object?>{'x': x, 'y': y, 'z': z};

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

  /// Encodes the `register_command` message sent to the host.
  Map<String, Object?> toJson() => <String, Object?>{
        't': 'register_command',
        'name': name,
        'description': description,
        'permission': permission,
      };
}

/// Everything a plugin can ask the server to do.
///
/// The runtime implements this on top of the JSON bridge. Tests implement it
/// with canned replies, which is what makes command logic testable without a
/// subprocess or stdin.
abstract interface class PluginHost {
  /// Calls a host method such as `player.teleport` and returns its result.
  ///
  /// The call is synchronous: the runtime blocks until the host replies. The
  /// returned value is the decoded JSON `result` (or `null`).
  ///
  /// Throws a `HostCallException` when the host reports a failure and an
  /// `RpcClosedException` when the connection closes first.
  Object? call(String method,
      [Map<String, Object?> params = const <String, Object?>{}]);

  /// Writes a message to the host log.
  void log(String message, {LogLevel level = LogLevel.info});

  /// Announces a command to the host.
  void registerCommand(CommandRegistration command);

  /// Subscribes to an event by its wire name, for example `player_join`.
  void registerEvent(String event);
}

/// A player as described by the host.
///
/// The descriptor fields are a snapshot. Every operation is performed through
/// the host, so a [Player] stays useful for as long as the player is online.
final class Player {
  Player({
    required this.entity,
    required this.world,
    required this.name,
    required this.uuid,
    required PluginHost host,
  }) : _host = host;

  /// Decodes a player from a JSON object.
  factory Player.fromJson(Map<String, Object?> json, PluginHost host) => Player(
        entity: _integer(json, 'entity', 'player'),
        world: _integer(json, 'world', 'player'),
        name: _string(json, 'name', 'player'),
        uuid: _string(json, 'uuid', 'player'),
        host: host,
      );

  /// Decodes a player from a decoded host reply.
  factory Player.fromResult(
    Object? result,
    PluginHost host, {
    String context = 'player',
  }) =>
      Player.fromJson(asJsonObject(result, context: context), host);

  /// Entity id within [world].
  final int entity;

  /// World id the player is currently in.
  final int world;

  /// Display name.
  final String name;

  /// Persistent UUID, used for storage such as player homes.
  final String uuid;

  final PluginHost _host;

  Map<String, Object?> get _target =>
      <String, Object?>{'entity': entity, 'world': world};

  /// Sends a chat message to this player.
  void sendMessage(String message) {
    _host.call('player.send_message', <String, Object?>{
      ..._target,
      'message': message,
    });
  }

  /// Teleports this player to [position].
  void teleport(Vec3 position, {double yaw = 0, double pitch = 0}) {
    _host.call('player.teleport', <String, Object?>{
      ..._target,
      ...position.toJson(),
      'yaw': yaw,
      'pitch': pitch,
    });
  }

  /// Returns the player's current position.
  Vec3 position() => Vec3.fromResult(
        _host.call('player.position', _target),
        context: 'player.position',
      );

  /// Returns the player's current health.
  double health() {
    final result = _host.call('player.health', _target);
    if (result is num) {
      return result.toDouble();
    }
    final json = asJsonObject(result, context: 'player.health');
    final health = json['health'];
    if (health is num) {
      return health.toDouble();
    }
    throw FormatException(
      'Expected player.health to return a number, got ${result.runtimeType}',
    );
  }

  /// Sets the player's health.
  void setHealth(double health) {
    _host.call('player.set_health', <String, Object?>{
      ..._target,
      'health': health,
    });
  }

  @override
  bool operator ==(Object other) =>
      other is Player &&
      other.entity == entity &&
      other.world == world &&
      other.uuid == uuid;

  @override
  int get hashCode => Object.hash(entity, world, uuid);

  @override
  String toString() => 'Player($name, entity: $entity, world: $world)';
}

/// Base class for server events delivered to a plugin.
///
/// Events are read-only snapshots. Call [cancel] to tell the host not to apply
/// the event; the runtime includes that decision in its response.
abstract base class PluginEvent {
  PluginEvent({
    required this.id,
    required this.world,
    required this.position,
    this.player,
  });

  /// Id of the host message, echoed in the response.
  final int id;

  /// World the event happened in.
  final int world;

  /// Position associated with the event.
  final Vec3 position;

  /// Player responsible for the event, when there is one.
  final Player? player;

  bool _cancelled = false;

  /// Whether [cancel] has been called.
  bool get isCancelled => _cancelled;

  /// Prevents the host from applying the event.
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

  /// The raw chat text.
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

  /// Numeric block state id sent by the host.
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

  /// Id of the host command message, echoed in the response.
  final int id;

  /// Command name without the leading slash.
  final String command;

  /// Raw argument string exactly as the host sent it.
  ///
  /// Parsing is deliberately left to the plugin. [arguments] offers a
  /// whitespace-tokenized convenience view when that is all a command needs.
  final String args;

  /// Player that ran the command, or `null` for the console.
  final Player? player;

  /// Host used for calls made while handling the command.
  final PluginHost host;

  /// Whitespace-separated tokens of [args].
  ///
  /// Returns an empty list when there are no arguments.
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

double _number(Map<String, Object?> json, String key, String context) {
  final value = json[key];
  if (value is num) {
    return value.toDouble();
  }
  throw FormatException(
    '$context.$key must be a number, got ${value.runtimeType}',
  );
}

int _integer(Map<String, Object?> json, String key, String context) {
  final value = json[key];
  if (value is num) {
    return value.toInt();
  }
  throw FormatException(
    '$context.$key must be an integer, got ${value.runtimeType}',
  );
}

String _string(Map<String, Object?> json, String key, String context) {
  final value = json[key];
  if (value is String) {
    return value;
  }
  throw FormatException(
    '$context.$key must be a string, got ${value.runtimeType}',
  );
}
