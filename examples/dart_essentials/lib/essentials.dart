import 'package:steel_extension/steel_plugin.dart';

import 'home_store.dart';

const String _defaultHomeName = 'home';
final RegExp _homeNamePattern = RegExp(r'^[a-z0-9_-]{1,32}$');
final RegExp _whitespace = RegExp(r'\s+');
final RegExp _messagePattern = RegExp(r'^(\S+)\s+([\s\S]+)$');

/// Essentials-style services: spawn, homes, teleport requests, messaging and
/// player utilities.
///
/// The class talks to the host through a [PluginContext], so command handlers
/// can be exercised in unit tests with a fake binding and no stdin:
///
/// ```dart
/// final essentials = Essentials(context: fakeContext, homes: store);
/// essentials.setHome(contextForPlayer('/sethome'));
/// ```
final class Essentials {
  Essentials({
    required PluginContext context,
    required HomeStore homes,
    int defaultWorld = 0,
  })  : _context = context,
        _homes = homes,
        _defaultWorld = defaultWorld;

  /// Maximum vanilla player health, used by `/heal`.
  static const double maxHealth = 20;

  final PluginContext _context;
  final HomeStore _homes;
  final int _defaultWorld;
  final Map<String, _TeleportRequest> _teleportRequests =
      <String, _TeleportRequest>{};

  /// Registers every Essentials command and the teleport-request cleanup.
  void register() {
    final context = _context;
    context
        .command('spawn')
        .description('Teleport to the world spawn.')
        .executes(spawn);
    context
        .command('setblock')
        .description('Place a block at a position.')
        .executes(setBlock);
    context
        .command('sethome')
        .description('Set a home at your current position.')
        .executes(setHome);
    context
        .command('home')
        .description('Teleport to one of your homes.')
        .executes(home);
    context
        .command('delhome')
        .description('Delete one of your homes.')
        .executes(deleteHome);
    context
        .command('homes')
        .description('List your homes.')
        .executes(listHomes);
    context
        .command('tpa')
        .description('Ask a player if you may teleport to them.')
        .executes(tpa);
    context
        .command('tpaccept')
        .description('Accept a pending teleport request.')
        .executes(acceptTeleport);
    context
        .command('tpdeny')
        .description('Deny a pending teleport request.')
        .executes(denyTeleport);
    context
        .command('msg')
        .description('Send a private message to a player.')
        .executes(messagePlayer);
    context
        .command('who')
        .description('List online players.')
        .executes(who);
    context
        .command('heal')
        .description('Restore your health.')
        .executes(heal);

    context.onPlayerLeave((player) {
      _teleportRequests.remove(player.uuid());
    });
  }

  /// `/spawn`: teleports the player to the spawn of their current world.
  void spawn(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final position = _context.world(player.world).spawn();
    player.teleport(position, 0, 0);
    player.sendMessage('Teleported to spawn at $position.');
    return;
  }

  /// `/setblock <x> <y> <z> <block>`: places a block's default state.
  void setBlock(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final parts = context.arguments;
    if (parts.length != 4) {
      player.sendMessage('Usage: /setblock <x> <y> <z> <block>.');
      return;
    }
    final x = int.tryParse(parts[0]);
    final y = int.tryParse(parts[1]);
    final z = int.tryParse(parts[2]);
    if (x == null || y == null || z == null) {
      player.sendMessage('Block coordinates must be whole numbers.');
      return;
    }
    final key = parts[3];
    final block = _context.block(key);
    if (block == null) {
      player.sendMessage('Unknown block "$key".');
      return;
    }
    final state = block.defaultState();
    if (!_context.world(player.world).setBlock(x, y, z, state, 3)) {
      player.sendMessage('Could not place $key at $x, $y, $z.');
      return;
    }
    player.sendMessage('Placed $key at $x, $y, $z.');
  }

  /// `/sethome [name]`: stores the player's current position as a home.
  void setHome(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /sethome [name] (letters, digits, - and _ only).',
      );
      return;
    }
    final position = player.position();
    _homes.set(player.uuid(), name, position);
    player.sendMessage('Home "$name" set at $position.');
    return;
  }

  /// `/home [name]`: teleports the player to a stored home.
  void home(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /home [name] (letters, digits, - and _ only).',
      );
      return;
    }
    final position = _homes.get(player.uuid(), name);
    if (position == null) {
      player.sendMessage(
        'Home "$name" not found. Create it with /sethome $name.',
      );
      return;
    }
    player.teleport(position, 0, 0);
    player.sendMessage('Welcome home, ${player.name()}.');
    return;
  }

  /// `/delhome [name]`: deletes a stored home.
  void deleteHome(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /delhome [name] (letters, digits, - and _ only).',
      );
      return;
    }
    if (!_homes.remove(player.uuid(), name)) {
      player.sendMessage('Home "$name" not found.');
      return;
    }
    player.sendMessage('Home "$name" deleted.');
    return;
  }

  /// `/homes`: lists the player's homes.
  void listHomes(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final names = _homes.namesOf(player.uuid());
    if (names.isEmpty) {
      player.sendMessage('You have no homes. Create one with /sethome [name].');
      return;
    }
    player.sendMessage('Homes (${names.length}): ${names.join(', ')}');
    return;
  }

  /// `/tpa <player>`: asks [player] to accept a teleport.
  void tpa(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final name = context.args.trim();
    if (name.isEmpty) {
      player.sendMessage('Usage: /tpa <player>.');
      return;
    }
    final target = _findPlayer(context, name);
    if (target == null) {
      player.sendMessage('Player "$name" is not online.');
      return;
    }
    if (target.uuid() == player.uuid()) {
      player.sendMessage('You cannot teleport to yourself.');
      return;
    }
    _teleportRequests[target.uuid()] = _TeleportRequest(player);
    target.sendMessage(
      '${player.name()} wants to teleport to you. Use /tpaccept or /tpdeny.',
    );
    player.sendMessage('Teleport request sent to ${target.name()}.');
    return;
  }

  /// `/tpaccept`: teleports the waiting requester to the player.
  void acceptTeleport(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final request = _teleportRequests.remove(player.uuid());
    if (request == null) {
      player.sendMessage('No pending teleport requests.');
      return;
    }
    final destination = player.position();
    request.requester.teleport(destination, 0, 0);
    request.requester.sendMessage(
      '${player.name()} accepted your teleport request.',
    );
    player.sendMessage('Teleported ${request.requester.name()} to you.');
    return;
  }

  /// `/tpdeny`: discards the pending request.
  void denyTeleport(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final request = _teleportRequests.remove(player.uuid());
    if (request == null) {
      player.sendMessage('No pending teleport requests.');
      return;
    }
    request.requester.sendMessage(
      '${player.name()} denied your teleport request.',
    );
    player.sendMessage(
      'Denied ${request.requester.name()}\'s teleport request.',
    );
    return;
  }

  /// `/msg <player> <message>`: sends a private message.
  void messagePlayer(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    final match = _messagePattern.firstMatch(context.args.trim());
    if (match == null) {
      player.sendMessage('Usage: /msg <player> <message>.');
      return;
    }
    final name = match.group(1)!;
    final text = match.group(2)!.trim();
    final target = _findPlayer(context, name);
    if (target == null) {
      player.sendMessage('Player "$name" is not online.');
      return;
    }
    target.sendMessage('[${player.name()} -> you] $text');
    player.sendMessage('[you -> ${target.name()}] $text');
    return;
  }

  /// `/who`: lists the players in the command sender's world.
  void who(CommandContext context) {
    final world = context.player?.world ?? _defaultWorld;
    final players = _context.onlinePlayers(world: world);
    if (players.isEmpty) {
      _tell(context, 'No players are online.');
      return;
    }
    final names = players.map((player) => player.name()).join(', ');
    _tell(context, 'Online (${players.length}): $names');
    return;
  }

  /// `/heal`: restores the player to full health.
  void heal(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return;
    }
    player.setHealth(maxHealth);
    player.sendMessage('You have been healed.');
    return;
  }

  Player? _requirePlayer(CommandContext context) {
    final player = context.player;
    if (player == null) {
      _context.log(
        '/${context.command} can only be used by a player.',
        level: LogLevel.warn,
      );
    }
    return player;
  }

  Player? _findPlayer(CommandContext context, String name) {
    final world = context.player?.world ?? _defaultWorld;
    return _context.findPlayer(name, world: world);
  }

  void _tell(CommandContext context, String message) {
    final player = context.player;
    if (player != null) {
      player.sendMessage(message);
      return;
    }
    _context.log(message);
  }
}

/// Installs the Essentials command set as a Steel plugin.
final class EssentialsPlugin implements SteelPlugin {
  EssentialsPlugin({required this.homeStore, this.defaultWorld = 0});

  /// Storage for player homes, loaded when the plugin is enabled.
  final HomeStore homeStore;

  /// World used when a command has no player context, such as console `/who`.
  final int defaultWorld;

  @override
  void onEnable(PluginContext context) {
    homeStore.load();
    Essentials(
      context: context,
      homes: homeStore,
      defaultWorld: defaultWorld,
    ).register();
  }
}

final class _TeleportRequest {
  _TeleportRequest(this.requester);

  final Player requester;
}

String? _parseHomeName(String args) {
  final trimmed = args.trim();
  final name = trimmed.isEmpty
      ? _defaultHomeName
      : trimmed.split(_whitespace).first.toLowerCase();
  return _homeNamePattern.hasMatch(name) ? name : null;
}
