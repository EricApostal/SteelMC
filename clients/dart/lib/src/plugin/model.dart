import 'package:dart_mappable/dart_mappable.dart';

import 'generated_api.dart';
import 'json.dart';

export 'generated_api.dart';

part 'model.mapper.dart';

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
@MappableClass()
final class Vec3 with Vec3Mappable {
  const Vec3(this.x, this.y, this.z);

  /// Decodes a position from a JSON object.
  ///
  /// Throws a [FormatException] when a component is missing or not numeric.
  factory Vec3.fromJson(
    Map<String, Object?> json, {
    String context = 'position',
  }) =>
      Vec3Mapper.fromMap(json.cast<String, dynamic>());

  /// Decodes a position from a decoded host reply.
  factory Vec3.fromResult(Object? result, {String context = 'position'}) =>
      Vec3.fromJson(asJsonObject(result, context: context), context: context);

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
