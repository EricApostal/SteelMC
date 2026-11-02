import 'context.dart';
import 'model.dart';
import 'rpc.dart';
import 'transport.dart';

/// Runs [plugin] until the host sends `shutdown` or closes the transport.
///
/// The loop is synchronous: each host message is handled to completion before
/// the next one is read. A command or event handler that calls the host blocks
/// inside [RpcClient.request] until the reply arrives, and unrelated inbound
/// messages are queued by the client instead of being lost.
///
/// When the host sends `init` with level 1 or higher, [SteelPlugin.onEnable] is
/// called once and a `ready` message follows the plugin's registrations.
void runSteelPlugin(SteelPlugin plugin, {LineTransport? transport}) {
  final rpc = RpcClient(transport ?? StdioTransport());
  final host = _RpcPluginHost(rpc);
  final context = PluginContext(host);
  var enabled = false;

  while (true) {
    final message = rpc.receive();
    if (message == null) {
      return;
    }
    switch (message['t']) {
      case 'init':
        final level = _int(message, 'level') ?? 0;
        if (!enabled && level >= 1) {
          plugin.onEnable(context);
          rpc.ready();
          enabled = true;
        }
        break;
      case 'command':
        _handleCommand(rpc, context, host, message);
        break;
      case 'event':
        _handleEvent(rpc, context, host, message);
        break;
      case 'host_result':
      case 'host_error':
        // Replies are matched by RpcClient.request. A reply nobody asked for
        // has no handler, so it is ignored.
        break;
      case 'shutdown':
        return;
      default:
        host.log(
          'Ignoring unknown host message type: ${message['t']}',
          level: LogLevel.warn,
        );
    }
  }
}

void _handleCommand(
  RpcClient rpc,
  PluginContext context,
  PluginHost host,
  Map<String, Object?> message,
) {
  final id = _int(message, 'id') ?? 0;
  final name = _normalize(_string(message, 'command') ?? '');
  final playerJson = message['player'];
  final player = playerJson == null
      ? null
      : Player.fromResult(playerJson, host, context: 'command.player');
  final commandContext = CommandContext(
    id: id,
    command: name,
    args: _string(message, 'args') ?? '',
    player: player,
    host: host,
  );

  Object? result;
  try {
    result = context.executeCommand(commandContext);
  } catch (error, stackTrace) {
    host.log(
      'Command "/$name" failed: $error\n$stackTrace',
      level: LogLevel.error,
    );
    result = null;
  }
  rpc.respond(id, result: result);
}

void _handleEvent(
  RpcClient rpc,
  PluginContext context,
  PluginHost host,
  Map<String, Object?> message,
) {
  final id = _int(message, 'id') ?? 0;
  final world = _int(message, 'world') ?? 0;
  final playerJson = message['player'];
  final player = playerJson == null
      ? null
      : Player.fromResult(playerJson, host, context: 'event.player');
  final position = Vec3(
    _double(message, 'x'),
    _double(message, 'y'),
    _double(message, 'z'),
  );
  final eventName = _string(message, 'event') ?? '';
  var cancel = false;

  switch (eventName) {
    case 'player_join':
      if (player != null) {
        context.dispatchPlayerJoin(player);
      }
      break;
    case 'player_leave':
      if (player != null) {
        context.dispatchPlayerLeave(player);
      }
      break;
    case 'chat':
      final event = ChatEvent(
        id: id,
        world: world,
        position: position,
        player: player,
        text: _string(message, 'text') ?? '',
      );
      context.dispatchChat(event);
      cancel = event.isCancelled;
      break;
    case 'block_place':
    case 'block_break':
      final event = BlockEvent(
        id: id,
        world: world,
        position: position,
        player: player,
        blockState: _int(message, 'block_state') ?? 0,
      );
      if (eventName == 'block_place') {
        context.dispatchBlockPlace(event);
      } else {
        context.dispatchBlockBreak(event);
      }
      cancel = event.isCancelled;
      break;
    default:
      host.log('Ignoring unknown event "$eventName"', level: LogLevel.warn);
  }

  rpc.respond(id, cancel: cancel);
}

String _normalize(String name) {
  final trimmed = name.trim();
  return trimmed.startsWith('/') ? trimmed.substring(1) : trimmed;
}

String? _string(Map<String, Object?> message, String key) {
  final value = message[key];
  return value is String ? value : null;
}

int? _int(Map<String, Object?> message, String key) {
  final value = message[key];
  return value is num ? value.toInt() : null;
}

double _double(Map<String, Object?> message, String key) {
  final value = message[key];
  return value is num ? value.toDouble() : 0;
}

/// [PluginHost] backed by a [RpcClient].
final class _RpcPluginHost implements PluginHost {
  _RpcPluginHost(this._rpc);

  final RpcClient _rpc;

  @override
  Object? call(
    String method, [
    Map<String, Object?> params = const <String, Object?>{},
  ]) =>
      _rpc.call(method, params);

  @override
  void log(String message, {LogLevel level = LogLevel.info}) {
    _rpc.log(message, level: level.name);
  }

  @override
  void registerCommand(CommandRegistration command) {
    _rpc.send(command.toJson());
  }

  @override
  void registerEvent(String event) {
    _rpc.send(<String, Object?>{'t': 'register_event', 'event': event});
  }
}
