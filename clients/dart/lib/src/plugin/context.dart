import 'model.dart';

/// The entry point of a Dart plugin.
abstract interface class SteelPlugin {
  /// Called once when the embedded runtime starts.
  ///
  /// Register commands and events here. The native shim collects the
  /// registrations and forwards them to the host after this returns.
  void onEnable(PluginContext context);
}

/// Handles one invocation of a command.
typedef CommandHandler = void Function(CommandContext context);

/// Fluent builder for a command registration.
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
  CommandBuilder permission(String? permission) {
    _permission = permission;
    return this;
  }

  /// Registers the command with the given [handler].
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
final class PluginContext implements PluginHost {
  PluginContext(this._host);

  final PluginHost _host;
  final Map<int, CommandHandler> _commands = <int, CommandHandler>{};
  final Map<String, CommandHandler> _commandsByName = <String, CommandHandler>{};
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
  final Set<int> _registeredEvents = <int>{};
  int _nextCommandId = 1;

  /// Starts building a command named [name].
  CommandBuilder command(String name) => CommandBuilder(this, name);

  void onPlayerJoin(void Function(Player player) handler) {
    _playerJoinHandlers.add(handler);
    registerEvent(EventKind.playerJoin);
  }

  void onPlayerLeave(void Function(Player player) handler) {
    _playerLeaveHandlers.add(handler);
    registerEvent(EventKind.playerLeave);
  }

  void onChat(void Function(ChatEvent event) handler) {
    _chatHandlers.add(handler);
    registerEvent(EventKind.chat);
  }

  void onBlockPlace(void Function(BlockEvent event) handler) {
    _blockPlaceHandlers.add(handler);
    registerEvent(EventKind.blockPlace);
  }

  void onBlockBreak(void Function(BlockEvent event) handler) {
    _blockBreakHandlers.add(handler);
    registerEvent(EventKind.blockBreak);
  }

  /// Returns a typed handle for [id] (world `0` by default).
  World world([int id = 0]) => World(this, id);

  /// Server-wide queries.
  Server get server => Server(this);

  List<Player> onlinePlayers({int world = 0}) => this.world(world).players();

  Player? findPlayer(String name, {int world = 0}) =>
      server.findPlayer(name, world: world);

  Vec3 spawn({int world = 0}) => this.world(world).spawn();

  @override
  Player? playerData(int world, int entity) => _host.playerData(world, entity);

  @override
  Vec3 entityPosition(int world, int entity) =>
      _host.entityPosition(world, entity);

  @override
  void entityTeleport(
    int world,
    int entity,
    Vec3 position, {
    double yaw = 0,
    double pitch = 0,
  }) =>
      _host.entityTeleport(world, entity, position, yaw: yaw, pitch: pitch);

  @override
  void playerSendMessage(int world, int entity, String message) =>
      _host.playerSendMessage(world, entity, message);

  @override
  double entityHealth(int world, int entity) =>
      _host.entityHealth(world, entity);

  @override
  void entitySetHealth(int world, int entity, double health) =>
      _host.entitySetHealth(world, entity, health);

  @override
  Vec3 worldSpawn(int world) => _host.worldSpawn(world);

  @override
  List<Player> worldPlayers(int world) => _host.worldPlayers(world);

  @override
  Player? findPlayerInWorld(int world, String name) =>
      _host.findPlayerInWorld(world, name);

  @override
  void log(String message, {LogLevel level = LogLevel.info}) =>
      _host.log(message, level: level);

  @override
  void registerCommand(int id, CommandRegistration command) =>
      _host.registerCommand(id, command);

  @override
  void registerEvent(int kind) {
    if (_registeredEvents.add(kind)) {
      _host.registerEvent(kind);
    }
  }

  void _addCommand(CommandRegistration command, CommandHandler handler) {
    final id = _nextCommandId++;
    _commands[id] = handler;
    _commandsByName[_normalize(command.name)] = handler;
    _host.registerCommand(id, command);
  }

  /// Runs the handler for [context], if any.
  void executeCommand(CommandContext context) {
    final handler = _commandsByName[_normalize(context.command)] ??
        _commands[context.id];
    if (handler == null) {
      log('Unknown command: /${context.command}', level: LogLevel.warn);
      return;
    }
    handler(context);
  }

  void dispatchPlayerJoin(Player player) {
    for (final handler in List<void Function(Player)>.of(_playerJoinHandlers)) {
      handler(player);
    }
  }

  void dispatchPlayerLeave(Player player) {
    for (final handler
        in List<void Function(Player)>.of(_playerLeaveHandlers)) {
      handler(player);
    }
  }

  void dispatchChat(ChatEvent event) {
    for (final handler in List<void Function(ChatEvent)>.of(_chatHandlers)) {
      handler(event);
    }
  }

  void dispatchBlockPlace(BlockEvent event) {
    for (final handler
        in List<void Function(BlockEvent)>.of(_blockPlaceHandlers)) {
      handler(event);
    }
  }

  void dispatchBlockBreak(BlockEvent event) {
    for (final handler
        in List<void Function(BlockEvent)>.of(_blockBreakHandlers)) {
      handler(event);
    }
  }
}

String _normalize(String name) {
  final trimmed = name.trim();
  return trimmed.startsWith('/') ? trimmed.substring(1) : trimmed;
}
