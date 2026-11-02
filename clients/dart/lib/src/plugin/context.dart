import 'json.dart';
import 'model.dart';

/// The entry point of a Dart plugin.
///
/// Implement this and pass an instance to `runSteelPlugin`.
abstract interface class SteelPlugin {
  /// Called once, after the host sends `init` with level 1 or higher.
  ///
  /// Register commands and events here. The runtime sends `ready` after this
  /// method returns, so all `register_command` and `register_event` messages
  /// are guaranteed to reach the host before it dispatches anything.
  void onEnable(PluginContext context);
}

/// Handles one invocation of a command.
///
/// The value returned becomes the command response's `result` and must be
/// JSON-encodable.
typedef CommandHandler = Object? Function(CommandContext context);

/// Fluent builder for a command registration.
///
/// ```dart
/// context
///     .command('home')
///     .description('Teleport to your home.')
///     .permission('essentials.home')
///     .executes(handleHome);
/// ```
final class CommandBuilder {
  CommandBuilder(this._context, this._name);

  final PluginContext _context;
  final String _name;
  String _description = '';
  String? _permission;

  /// Sets the help text shown by the host.
  CommandBuilder description(String description) {
    _description = description;
    return this;
  }

  /// Sets the permission required to run the command.
  ///
  /// Pass `null` (the default) to allow everyone.
  CommandBuilder permission(String? permission) {
    _permission = permission;
    return this;
  }

  /// Registers the command with the given [handler].
  ///
  /// The registration message is written to the host immediately.
  void executes(CommandHandler handler) {
    _context._addCommand(
      CommandRegistration(
        name: _name,
        description: _description,
        permission: _permission,
      ),
      handler,
    );
  }
}

/// The plugin's view of the host while it is enabled.
///
/// A context both provides host operations (it implements [PluginHost]) and
/// collects the plugin's registrations. The runtime constructs exactly one per
/// plugin process and passes it to [SteelPlugin.onEnable].
final class PluginContext implements PluginHost {
  PluginContext(this._host);

  final PluginHost _host;
  final Map<String, CommandHandler> _commands = <String, CommandHandler>{};
  final List<void Function(Player player)> _playerJoinHandlers =
      <void Function(Player player)>[];
  final List<void Function(Player player)> _playerLeaveHandlers =
      <void Function(Player player)>[];
  final List<void Function(ChatEvent event)> _chatHandlers =
      <void Function(ChatEvent event)>[];
  final List<void Function(BlockEvent event)> _blockPlaceHandlers =
      <void Function(BlockEvent event)>[];
  final List<void Function(BlockEvent event)> _blockBreakHandlers =
      <void Function(BlockEvent event)>[];
  final Set<String> _registeredEvents = <String>{};

  /// Starts building a command named [name] (with or without a leading slash).
  CommandBuilder command(String name) => CommandBuilder(this, name);

  /// Registers [handler] for players joining the server.
  void onPlayerJoin(void Function(Player player) handler) {
    _playerJoinHandlers.add(handler);
    registerEvent('player_join');
  }

  /// Registers [handler] for players leaving the server.
  void onPlayerLeave(void Function(Player player) handler) {
    _playerLeaveHandlers.add(handler);
    registerEvent('player_leave');
  }

  /// Registers [handler] for chat messages.
  ///
  /// Call [PluginEvent.cancel] on the event to suppress the message.
  void onChat(void Function(ChatEvent event) handler) {
    _chatHandlers.add(handler);
    registerEvent('chat');
  }

  /// Registers [handler] for block placement.
  void onBlockPlace(void Function(BlockEvent event) handler) {
    _blockPlaceHandlers.add(handler);
    registerEvent('block_place');
  }

  /// Registers [handler] for block breaking.
  void onBlockBreak(void Function(BlockEvent event) handler) {
    _blockBreakHandlers.add(handler);
    registerEvent('block_break');
  }

  /// Returns every player in [world].
  List<Player> onlinePlayers({int world = 0}) {
    final result = call('world.players', <String, Object?>{'world': world});
    return <Player>[
      for (final entry in asJsonList(result, context: 'world.players'))
        Player.fromResult(entry, this, context: 'world.players entry'),
    ];
  }

  /// Finds an online player by name in [world], or returns `null`.
  Player? findPlayer(String name, {int world = 0}) {
    final result = call(
      'server.find_player',
      <String, Object?>{'world': world, 'name': name},
    );
    if (result == null) {
      return null;
    }
    return Player.fromResult(result, this, context: 'server.find_player');
  }

  /// Returns the spawn position of [world].
  Vec3 spawn({int world = 0}) => Vec3.fromResult(
        call('world.spawn', <String, Object?>{'world': world}),
        context: 'world.spawn',
      );

  @override
  Object? call(
    String method, [
    Map<String, Object?> params = const <String, Object?>{},
  ]) =>
      _host.call(method, params);

  @override
  void log(String message, {LogLevel level = LogLevel.info}) {
    _host.log(message, level: level);
  }

  @override
  void registerCommand(CommandRegistration command) {
    _host.registerCommand(command);
  }

  @override
  void registerEvent(String event) {
    if (_registeredEvents.add(event)) {
      _host.registerEvent(event);
    }
  }

  /// Runs the handler registered for `context.command`.
  ///
  /// Called by the runtime. Unknown commands are logged and answered with a
  /// `null` result.
  Object? executeCommand(CommandContext context) {
    final handler = _commands[_normalize(context.command)];
    if (handler == null) {
      log('Unknown command: /${context.command}', level: LogLevel.warn);
      return null;
    }
    return handler(context);
  }

  /// Delivers [player] to the `player_join` handlers.
  ///
  /// Called by the runtime.
  void dispatchPlayerJoin(Player player) {
    for (final handler in List<void Function(Player)>.of(_playerJoinHandlers)) {
      handler(player);
    }
  }

  /// Delivers [player] to the `player_leave` handlers.
  ///
  /// Called by the runtime.
  void dispatchPlayerLeave(Player player) {
    for (final handler
        in List<void Function(Player)>.of(_playerLeaveHandlers)) {
      handler(player);
    }
  }

  /// Delivers [event] to the `chat` handlers.
  ///
  /// Called by the runtime.
  void dispatchChat(ChatEvent event) {
    for (final handler in List<void Function(ChatEvent)>.of(_chatHandlers)) {
      handler(event);
    }
  }

  /// Delivers [event] to the `block_place` handlers.
  ///
  /// Called by the runtime.
  void dispatchBlockPlace(BlockEvent event) {
    for (final handler
        in List<void Function(BlockEvent)>.of(_blockPlaceHandlers)) {
      handler(event);
    }
  }

  /// Delivers [event] to the `block_break` handlers.
  ///
  /// Called by the runtime.
  void dispatchBlockBreak(BlockEvent event) {
    for (final handler
        in List<void Function(BlockEvent)>.of(_blockBreakHandlers)) {
      handler(event);
    }
  }

  void _addCommand(CommandRegistration command, CommandHandler handler) {
    final name = _normalize(command.name);
    _commands[name] = handler;
    _host.registerCommand(
      CommandRegistration(
        name: name,
        description: command.description,
        permission: command.permission,
      ),
    );
  }

  static String _normalize(String name) {
    final trimmed = name.trim();
    return trimmed.startsWith('/') ? trimmed.substring(1) : trimmed;
  }
}
