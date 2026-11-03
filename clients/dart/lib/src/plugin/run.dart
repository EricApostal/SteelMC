import 'context.dart';
import 'model.dart';
import 'protocol.dart';
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
    switch (message) {
      case InitMessage(:final level):
        if (!enabled && level >= 1) {
          plugin.onEnable(context);
          rpc.ready();
          enabled = true;
        }
      case CommandMessage():
        _handleCommand(rpc, context, host, message);
      case EventMessage():
        _handleEvent(rpc, context, host, message);
      case HostResultMessage():
      case HostErrorMessage():
        // Replies are matched by RpcClient.request. A reply nobody asked for
        // has no handler, so it is ignored.
        break;
      case ShutdownMessage():
        return;
    }
  }
}

void _handleCommand(
  RpcClient rpc,
  PluginContext context,
  PluginHost host,
  CommandMessage message,
) {
  final name = _normalize(message.command);
  final data = message.player;
  final player = data == null ? null : Player.fromData(data, host);
  final commandContext = CommandContext(
    id: message.id,
    command: name,
    args: message.args,
    player: player,
    host: host,
  );

  try {
    context.executeCommand(commandContext);
  } catch (error, stackTrace) {
    host.log(
      'Command "/$name" failed: $error\n$stackTrace',
      level: LogLevel.error,
    );
  }
  rpc.respond(message.id);
}

void _handleEvent(
  RpcClient rpc,
  PluginContext context,
  PluginHost host,
  EventMessage message,
) {
  final data = message.player;
  final player = data == null ? null : Player.fromData(data, host);
  final position = Vec3(message.x, message.y, message.z);
  var cancel = false;

  switch (message.event) {
    case 'player_join':
      if (player != null) {
        context.dispatchPlayerJoin(player);
      }
    case 'player_leave':
      if (player != null) {
        context.dispatchPlayerLeave(player);
      }
    case 'chat':
      final event = ChatEvent(
        id: message.id,
        world: message.world,
        position: position,
        player: player,
        text: message.text,
      );
      context.dispatchChat(event);
      cancel = event.isCancelled;
    case 'block_place':
    case 'block_break':
      final event = BlockEvent(
        id: message.id,
        world: message.world,
        position: position,
        player: player,
        blockState: message.blockState,
      );
      if (message.event == 'block_place') {
        context.dispatchBlockPlace(event);
      } else {
        context.dispatchBlockBreak(event);
      }
      cancel = event.isCancelled;
    default:
      host.log(
        'Ignoring unknown event "${message.event}"',
        level: LogLevel.warn,
      );
  }

  rpc.respond(message.id, cancel: cancel);
}

String _normalize(String name) {
  final trimmed = name.trim();
  return trimmed.startsWith('/') ? trimmed.substring(1) : trimmed;
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
    _rpc.registerCommand(command);
  }

  @override
  void registerEvent(String event) {
    _rpc.registerEvent(event);
  }
}
