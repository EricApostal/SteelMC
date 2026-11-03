import 'context.dart';
import 'model.dart';
import 'native.dart';

int _interfaceAddress = 0;
PluginContext? _context;

/// Receives the host interface address from the native shim before `main`.
///
/// The shim invokes this top-level function by name; plugin authors never call
/// it directly.
@pragma('vm:entry-point')
void steelSetInterface(int address) {
  _interfaceAddress = address;
}

/// Starts [plugin] inside the embedded Dart VM.
///
/// The native shim invokes the plugin's `main`, which should call this once.
/// This function performs registration only; the isolate stays alive and the
/// shim calls back into `steelDispatchCommand`/`steelDispatchEvent`.
void runSteelPlugin(SteelPlugin plugin) {
  final host = NativePluginHost.fromAddress(_interfaceAddress);
  final context = PluginContext(host);
  _context = context;
  plugin.onEnable(context);
}

/// Dispatches a command from the native shim.
///
/// Invoked by the shim on a host thread after acquiring the Dart isolate.
@pragma('vm:entry-point')
int steelDispatchCommand(
  int id,
  int world,
  int entity,
  int hasPlayer,
  String command,
  String args,
) {
  final context = _context;
  if (context == null) {
    return 0;
  }
  final host = context;
  final player = hasPlayer != 0 ? host.playerData(world, entity) : null;
  context.executeCommand(
    CommandContext(
      id: id,
      command: command,
      args: args,
      player: player,
      host: host,
    ),
  );
  return 1;
}

/// Dispatches an event from the native shim. Returns `1` to cancel.
@pragma('vm:entry-point')
int steelDispatchEvent(
  int kind,
  int world,
  int x,
  int y,
  int z,
  int blockState,
  int entity,
  int hasPlayer,
  String text,
) {
  final context = _context;
  if (context == null) {
    return 0;
  }
  final player = hasPlayer != 0 ? context.playerData(world, entity) : null;
  final position = Vec3(x.toDouble(), y.toDouble(), z.toDouble());

  switch (kind) {
    case EventKind.playerJoin:
      if (player != null) {
        context.dispatchPlayerJoin(player);
      }
    case EventKind.playerLeave:
      if (player != null) {
        context.dispatchPlayerLeave(player);
      }
    case EventKind.chat:
      final event = ChatEvent(
        id: 0,
        world: world,
        position: position,
        player: player,
        text: text,
      );
      context.dispatchChat(event);
      return event.isCancelled ? 1 : 0;
    case EventKind.blockPlace:
    case EventKind.blockBreak:
      final event = BlockEvent(
        id: 0,
        world: world,
        position: position,
        player: player,
        blockState: blockState,
      );
      if (kind == EventKind.blockPlace) {
        context.dispatchBlockPlace(event);
      } else {
        context.dispatchBlockBreak(event);
      }
      return event.isCancelled ? 1 : 0;
  }
  return 0;
}
