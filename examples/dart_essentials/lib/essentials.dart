import 'package:steel_extension/steel_plugin.dart';

import 'home_store.dart';

const String _defaultHomeName = 'home';
final RegExp _homeNamePattern = RegExp(r'^[a-z0-9_-]{1,32}$');
final RegExp _whitespace = RegExp(r'\s+');
final RegExp _messagePattern = RegExp(r'^(\S+)\s+([\s\S]+)$');

/// Essentials-style services: spawn, homes, teleport requests, messaging and
/// player utilities.
///
/// The class is a plain [PluginHost] consumer, so command handlers can be
/// exercised in unit tests with a fake host and no stdin:
///
/// ```dart
/// final essentials = Essentials(host: fakeHost, homes: store);
/// essentials.setHome(contextForPlayer('/sethome'));
/// ```
final class Essentials {
  Essentials({
    required PluginHost host,
    required HomeStore homes,
    int defaultWorld = 0,
  })  : _host = host,
        _homes = homes,
        _defaultWorld = defaultWorld;

  /// Maximum vanilla player health, used by `/heal`.
  static const double maxHealth = 20;

  final PluginHost _host;
  final HomeStore _homes;
  final int _defaultWorld;
  final Map<String, _TeleportRequest> _teleportRequests =
      <String, _TeleportRequest>{};

  /// Registers every Essentials command and the teleport-request cleanup.
  void register(PluginContext context) {
    context
        .command('spawn')
        .description('Teleport to the world spawn.')
        .executes(spawn);
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
      _teleportRequests.remove(player.uuid);
    });
  }

  /// `/spawn`: teleports the player to the spawn of their current world.
  Object? spawn(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final position = Vec3.fromResult(
      _host.call('world.spawn', <String, Object?>{'world': player.world}),
      context: 'world.spawn',
    );
    player.teleport(position);
    player.sendMessage('Teleported to spawn at $position.');
    return null;
  }

  /// `/sethome [name]`: stores the player's current position as a home.
  Object? setHome(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /sethome [name] (letters, digits, - and _ only).',
      );
      return null;
    }
    final position = player.position();
    _homes.set(player.uuid, name, position);
    player.sendMessage('Home "$name" set at $position.');
    return null;
  }

  /// `/home [name]`: teleports the player to a stored home.
  Object? home(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /home [name] (letters, digits, - and _ only).',
      );
      return null;
    }
    final position = _homes.get(player.uuid, name);
    if (position == null) {
      player.sendMessage(
        'Home "$name" not found. Create it with /sethome $name.',
      );
      return null;
    }
    player.teleport(position);
    player.sendMessage('Welcome home, ${player.name}.');
    return null;
  }

  /// `/delhome [name]`: deletes a stored home.
  Object? deleteHome(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final name = _parseHomeName(context.args);
    if (name == null) {
      player.sendMessage(
        'Usage: /delhome [name] (letters, digits, - and _ only).',
      );
      return null;
    }
    if (!_homes.remove(player.uuid, name)) {
      player.sendMessage('Home "$name" not found.');
      return null;
    }
    player.sendMessage('Home "$name" deleted.');
    return null;
  }

  /// `/homes`: lists the player's homes.
  Object? listHomes(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final names = _homes.namesOf(player.uuid);
    if (names.isEmpty) {
      player.sendMessage('You have no homes. Create one with /sethome [name].');
      return null;
    }
    player.sendMessage('Homes (${names.length}): ${names.join(', ')}');
    return null;
  }

  /// `/tpa <player>`: asks [player] to accept a teleport.
  Object? tpa(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final name = context.args.trim();
    if (name.isEmpty) {
      player.sendMessage('Usage: /tpa <player>.');
      return null;
    }
    final target = _findPlayer(context, name);
    if (target == null) {
      player.sendMessage('Player "$name" is not online.');
      return null;
    }
    if (target.uuid == player.uuid) {
      player.sendMessage('You cannot teleport to yourself.');
      return null;
    }
    _teleportRequests[target.uuid] = _TeleportRequest(player);
    target.sendMessage(
      '${player.name} wants to teleport to you. Use /tpaccept or /tpdeny.',
    );
    player.sendMessage('Teleport request sent to ${target.name}.');
    return null;
  }

  /// `/tpaccept`: teleports the waiting requester to the player.
  Object? acceptTeleport(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final request = _teleportRequests.remove(player.uuid);
    if (request == null) {
      player.sendMessage('No pending teleport requests.');
      return null;
    }
    final destination = player.position();
    request.requester.teleport(destination);
    request.requester.sendMessage(
      '${player.name} accepted your teleport request.',
    );
    player.sendMessage('Teleported ${request.requester.name} to you.');
    return null;
  }

  /// `/tpdeny`: discards the pending request.
  Object? denyTeleport(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final request = _teleportRequests.remove(player.uuid);
    if (request == null) {
      player.sendMessage('No pending teleport requests.');
      return null;
    }
    request.requester.sendMessage(
      '${player.name} denied your teleport request.',
    );
    player.sendMessage('Denied ${request.requester.name}\'s teleport request.');
    return null;
  }

  /// `/msg <player> <message>`: sends a private message.
  Object? messagePlayer(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    final match = _messagePattern.firstMatch(context.args.trim());
    if (match == null) {
      player.sendMessage('Usage: /msg <player> <message>.');
      return null;
    }
    final name = match.group(1)!;
    final text = match.group(2)!.trim();
    final target = _findPlayer(context, name);
    if (target == null) {
      player.sendMessage('Player "$name" is not online.');
      return null;
    }
    target.sendMessage('[${player.name} -> you] $text');
    player.sendMessage('[you -> ${target.name}] $text');
    return null;
  }

  /// `/who`: lists the players in the command sender's world.
  Object? who(CommandContext context) {
    final world = context.player?.world ?? _defaultWorld;
    final result =
        _host.call('world.players', <String, Object?>{'world': world});
    final entries =
        result is List ? List<Object?>.of(result) : const <Object?>[];
    final players = <Player>[
      for (final entry in entries)
        Player.fromResult(entry, _host, context: 'world.players entry'),
    ];
    if (players.isEmpty) {
      _tell(context, 'No players are online.');
      return null;
    }
    final names = players.map((player) => player.name).join(', ');
    _tell(context, 'Online (${players.length}): $names');
    return null;
  }

  /// `/heal`: restores the player to full health.
  Object? heal(CommandContext context) {
    final player = _requirePlayer(context);
    if (player == null) {
      return null;
    }
    player.setHealth(maxHealth);
    player.sendMessage('You have been healed.');
    return null;
  }

  Player? _requirePlayer(CommandContext context) {
    final player = context.player;
    if (player == null) {
      _host.log(
        '/${context.command} can only be used by a player.',
        level: LogLevel.warn,
      );
    }
    return player;
  }

  Player? _findPlayer(CommandContext context, String name) {
    final world = context.player?.world ?? _defaultWorld;
    final result = _host.call(
      'server.find_player',
      <String, Object?>{'world': world, 'name': name},
    );
    if (result == null) {
      return null;
    }
    return Player.fromResult(result, _host, context: 'server.find_player');
  }

  void _tell(CommandContext context, String message) {
    final player = context.player;
    if (player != null) {
      player.sendMessage(message);
      return;
    }
    _host.log(message);
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
      host: context,
      homes: homeStore,
      defaultWorld: defaultWorld,
    ).register(context);
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
