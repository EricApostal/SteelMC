import '../steel/binding.dart';
import '../steel/generated.dart';
import '../steel/model.dart';
import '../steel/native_shim.dart';

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
  String toString() => 'ChatEvent(${player?.name() ?? 'server'}: $text)';
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
      'BlockEvent($blockState at $position, player: ${player?.name()})';
}

/// Context passed to a command handler.
final class CommandContext {
  const CommandContext({
    required this.id,
    required this.command,
    required this.args,
    required this.player,
  });

  final int id;
  final String command;
  final String args;
  final Player? player;

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
///
/// Host objects are generated classes that call the host through
/// [binding]; command and event registrations travel through the in-process
/// [shim].
final class PluginContext {
  PluginContext({required this.binding, required this.shim});

  /// Transport used for every generated host call.
  final SteelBinding binding;

  /// In-process registration and logging hooks.
  final NativeShim shim;

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

  /// Registers a handler for players joining the server.
  void onPlayerJoin(void Function(Player player) handler) {
    _playerJoinHandlers.add(handler);
    registerEvent(EventKind.playerJoin);
  }

  /// Registers a handler for players leaving the server.
  void onPlayerLeave(void Function(Player player) handler) {
    _playerLeaveHandlers.add(handler);
    registerEvent(EventKind.playerLeave);
  }

  /// Registers a handler for chat messages.
  void onChat(void Function(ChatEvent event) handler) {
    _chatHandlers.add(handler);
    registerEvent(EventKind.chat);
  }

  /// Registers a handler for block placement.
  void onBlockPlace(void Function(BlockEvent event) handler) {
    _blockPlaceHandlers.add(handler);
    registerEvent(EventKind.blockPlace);
  }

  /// Registers a handler for block breaking.
  void onBlockBreak(void Function(BlockEvent event) handler) {
    _blockBreakHandlers.add(handler);
    registerEvent(EventKind.blockBreak);
  }

  /// Returns a typed handle for [id].
  World world(int id) => World(binding, id);

  /// Server-wide queries.
  Server get server => Server(binding);

  /// Resolves a block by registry key such as `minecraft:stone`.
  Block? block(String key) => Block.byKey(binding, key);

  /// Returns a player handle for [entity] in [world].
  Player player(int world, int entity) => Player(binding, world, entity);

  /// Returns the spawn position of [world].
  Vec3 spawn({int world = 0}) => this.world(world).spawn();

  /// Returns every player currently in [world].
  List<Player> onlinePlayers({int world = 0}) {
    final handle = this.world(world);
    final count = handle.playerCount();
    final players = <Player>[];
    for (var index = 0; index < count; index++) {
      final player = handle.playerAt(index);
      if (player != null) {
        players.add(player);
      }
    }
    return players;
  }

  /// Finds an online player by name in [world].
  Player? findPlayer(String name, {int world = 0}) =>
      Server.findPlayer(binding, world, name);

  /// Writes a message to the host log.
  void log(String message, {LogLevel level = LogLevel.info}) {
    shim.log(_levelValue(level), message);
  }

  void _addCommand(CommandRegistration command, CommandHandler handler) {
    final id = _nextCommandId++;
    _commands[id] = handler;
    _commandsByName[_normalize(command.name)] = handler;
    final status = shim.registerCommand(
      id,
      command.name,
      command.description,
      command.permission ?? '',
    );
    if (status != 0) {
      throw SteelHostException('register_command failed with status $status');
    }
  }

  /// Registers interest in an event kind.
  void registerEvent(int kind) {
    if (!_registeredEvents.add(kind)) {
      return;
    }
    final status = shim.registerEvent(kind);
    if (status != 0) {
      throw SteelHostException('register_event failed with status $status');
    }
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

int _levelValue(LogLevel level) => switch (level) {
      LogLevel.error => 0,
      LogLevel.warn => 1,
      LogLevel.info => 2,
      LogLevel.debug => 3,
    };