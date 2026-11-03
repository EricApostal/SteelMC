import 'dart:io';

import 'package:steel_essentials/essentials.dart';
import 'package:steel_essentials/home_store.dart';
import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// One host method invocation recorded by [FakePluginHost].
final class HostCall {
  HostCall(this.method, this.params);

  final String method;
  final Map<String, Object?> params;

  @override
  String toString() => '$method($params)';
}

/// In-memory [PluginHost] with canned replies.
///
/// This is the whole point of the host abstraction: command logic runs without
/// stdin, a Dart VM process, or a Rust bridge.
final class FakePluginHost implements PluginHost {
  final List<HostCall> calls = <HostCall>[];
  final List<CommandRegistration> commands = <CommandRegistration>[];
  final List<int> events = <int>[];
  final List<String> logs = <String>[];
  final Map<String, Player> players = <String, Player>{};

  Vec3 position = const Vec3(0, 64, 0);
  Vec3 spawnPoint = const Vec3(0, 64, 0);
  double health = 20;

  @override
  Player? playerData(int world, int entity) {
    for (final player in players.values) {
      if (player.entity == entity) {
        return player;
      }
    }
    return null;
  }

  @override
  Vec3 entityPosition(int world, int entity) {
    calls.add(HostCall('player.position', <String, Object?>{
      'entity': entity,
      'world': world,
    }));
    return position;
  }

  @override
  void entityTeleport(
    int world,
    int entity,
    Vec3 position, {
    double yaw = 0,
    double pitch = 0,
  }) {
    calls.add(HostCall('player.teleport', <String, Object?>{
      'entity': entity,
      'world': world,
      'x': position.x,
      'y': position.y,
      'z': position.z,
      'yaw': yaw,
      'pitch': pitch,
    }));
    this.position = position;
  }

  @override
  void playerSendMessage(int world, int entity, String message) {
    calls.add(HostCall('player.send_message', <String, Object?>{
      'entity': entity,
      'world': world,
      'message': message,
    }));
  }

  @override
  double entityHealth(int world, int entity) {
    calls.add(HostCall('player.health', <String, Object?>{
      'entity': entity,
      'world': world,
    }));
    return health;
  }

  @override
  void entitySetHealth(int world, int entity, double health) {
    calls.add(HostCall('player.set_health', <String, Object?>{
      'entity': entity,
      'world': world,
      'health': health,
    }));
    this.health = health;
  }

  @override
  Vec3 worldSpawn(int world) {
    calls.add(HostCall('world.spawn', <String, Object?>{'world': world}));
    return spawnPoint;
  }

  @override
  List<Player> worldPlayers(int world) {
    calls.add(HostCall('world.players', <String, Object?>{'world': world}));
    return players.values.toList();
  }

  @override
  Player? findPlayerInWorld(int world, String name) {
    calls.add(HostCall('server.find_player', <String, Object?>{
      'world': world,
      'name': name,
    }));
    return players[name];
  }

  @override
  void log(String message, {LogLevel level = LogLevel.info}) {
    logs.add('${level.name}: $message');
  }

  @override
  void registerCommand(int id, CommandRegistration command) {
    commands.add(command);
  }

  @override
  void registerEvent(int kind) {
    if (!events.contains(kind)) {
      events.add(kind);
    }
  }

  List<HostCall> callsTo(String method) => <HostCall>[
        for (final call in calls)
          if (call.method == method) call,
      ];

  String? lastMessageTo(int entity) {
    for (final call in calls.reversed) {
      if (call.method == 'player.send_message' &&
          call.params['entity'] == entity) {
        final message = call.params['message'];
        if (message is String) {
          return message;
        }
      }
    }
    return null;
  }
}

void main() {
  late Directory tempDir;
  late FakePluginHost host;
  late HomeStore store;
  late Player steve;
  late Essentials essentials;

  setUp(() {
    tempDir = Directory.systemTemp.createTempSync('steel_essentials_');
    host = FakePluginHost();
    store = HomeStore(path: '${tempDir.path}/homes.json')..load();
    steve = Player(
      entity: 1,
      world: 0,
      name: 'Steve',
      uuid: 'uuid-steve',
      host: host,
    );
    essentials = Essentials(host: host, homes: store);
  });

  tearDown(() {
    if (tempDir.existsSync()) {
      tempDir.deleteSync(recursive: true);
    }
  });

  CommandContext command(String name, String args, {Player? player}) =>
      CommandContext(
        id: 1,
        command: name,
        args: args,
        player: player,
        host: host,
      );

  Player alex() {
    host.players['Alex'] = Player(
      entity: 2,
      world: 0,
      name: 'Alex',
      uuid: 'uuid-alex',
      host: host,
    );
    return Player(
      entity: 2,
      world: 0,
      name: 'Alex',
      uuid: 'uuid-alex',
      host: host,
    );
  }

  group('HomeStore', () {
    test('is empty when the file does not exist', () {
      expect(store.namesOf('uuid-steve'), isEmpty);
      expect(store.get('uuid-steve', 'home'), isNull);
    });

    test('persists and reloads homes', () {
      store.set('uuid-steve', 'base', const Vec3(1, 2, 3));

      final reloaded = HomeStore(path: store.path)..load();

      expect(reloaded.get('uuid-steve', 'base'), const Vec3(1, 2, 3));
    });

    test('sorts names and removes homes', () {
      store
        ..set('uuid-steve', 'zebra', const Vec3(0, 0, 0))
        ..set('uuid-steve', 'alpha', const Vec3(1, 1, 1));

      expect(store.namesOf('uuid-steve'), <String>['alpha', 'zebra']);
      expect(store.remove('uuid-steve', 'alpha'), isTrue);
      expect(store.remove('uuid-steve', 'alpha'), isFalse);

      final reloaded = HomeStore(path: store.path)..load();
      expect(reloaded.namesOf('uuid-steve'), <String>['zebra']);
    });

    test('round-trips multiple players', () {
      store
        ..set('uuid-steve', 'home', const Vec3(1, 2, 3))
        ..set('uuid-alex', 'home', const Vec3(-1, -2, -3));

      final reloaded = HomeStore(path: store.path)..load();

      expect(reloaded.get('uuid-steve', 'home'), const Vec3(1, 2, 3));
      expect(reloaded.get('uuid-alex', 'home'), const Vec3(-1, -2, -3));
    });
  });

  group('Essentials commands', () {
    test('/sethome stores the current position as the default home', () {
      host.position = const Vec3(5, 66, 5);

      essentials.setHome(command('sethome', '', player: steve));

      expect(store.get('uuid-steve', 'home'), const Vec3(5, 66, 5));
      expect(host.lastMessageTo(1), contains('Home "home" set'));
    });

    test('/sethome honors a name and normalizes its case', () {
      host.position = const Vec3(1, 2, 3);

      essentials.setHome(command('sethome', 'Base', player: steve));

      expect(store.get('uuid-steve', 'base'), const Vec3(1, 2, 3));
    });

    test('/sethome rejects an invalid name', () {
      essentials.setHome(command('sethome', '!!!', player: steve));

      expect(store.namesOf('uuid-steve'), isEmpty);
      expect(host.lastMessageTo(1), contains('Usage'));
    });

    test('/home teleports to the stored position', () {
      store.set('uuid-steve', 'base', const Vec3(10, 70, -4));

      essentials.home(command('home', 'base', player: steve));

      final teleport = host.callsTo('player.teleport').single;
      expect(teleport.params, <String, Object?>{
        'entity': 1,
        'world': 0,
        'x': 10.0,
        'y': 70.0,
        'z': -4.0,
        'yaw': 0.0,
        'pitch': 0.0,
      });
      expect(host.lastMessageTo(1), contains('Welcome home'));
    });

    test('/home reports an unknown home', () {
      essentials.home(command('home', 'missing', player: steve));

      expect(host.callsTo('player.teleport'), isEmpty);
      expect(host.lastMessageTo(1), contains('not found'));
    });

    test('/delhome removes an existing home', () {
      store.set('uuid-steve', 'base', const Vec3(1, 1, 1));

      essentials.deleteHome(command('delhome', 'base', player: steve));

      expect(store.get('uuid-steve', 'base'), isNull);
      expect(host.lastMessageTo(1), contains('deleted'));
    });

    test('/homes lists names in sorted order', () {
      store
        ..set('uuid-steve', 'zebra', const Vec3(0, 0, 0))
        ..set('uuid-steve', 'alpha', const Vec3(0, 0, 0));

      essentials.listHomes(command('homes', '', player: steve));

      expect(host.lastMessageTo(1), contains('alpha, zebra'));
    });

    test('/spawn teleports to the world spawn', () {
      host.spawnPoint = const Vec3(0, 64, 0);

      essentials.spawn(command('spawn', '', player: steve));

      final teleport = host.callsTo('player.teleport').single;
      expect(teleport.params['x'], 0.0);
      expect(teleport.params['y'], 64.0);
      expect(teleport.params['z'], 0.0);
    });

    test('/heal restores full health', () {
      essentials.heal(command('heal', '', player: steve));

      final setHealth = host.callsTo('player.set_health').single;
      expect(setHealth.params['health'], Essentials.maxHealth);
      expect(host.lastMessageTo(1), contains('healed'));
    });

    test('/tpa and /tpaccept teleport the requester', () {
      final alexPlayer = alex();

      essentials.tpa(command('tpa', 'Alex', player: steve));
      expect(host.lastMessageTo(2), contains('Steve'));

      host.position = const Vec3(4, 65, 4);
      essentials.acceptTeleport(command('tpaccept', '', player: alexPlayer));

      final teleport = host.callsTo('player.teleport').single;
      expect(teleport.params['entity'], 1);
      expect(teleport.params['x'], 4.0);
      expect(host.lastMessageTo(1), contains('accepted'));
    });

    test('/tpdeny discards the request', () {
      final alexPlayer = alex();

      essentials.tpa(command('tpa', 'Alex', player: steve));
      essentials.denyTeleport(command('tpdeny', '', player: alexPlayer));

      expect(host.callsTo('player.teleport'), isEmpty);
      expect(host.lastMessageTo(1), contains('denied'));
    });

    test('/msg delivers a private message', () {
      alex();

      essentials.messagePlayer(
        command('msg', 'Alex hello there', player: steve),
      );

      expect(host.lastMessageTo(2), contains('hello there'));
      expect(host.lastMessageTo(1), contains('hello there'));
    });

    test('/who lists online players', () {
      alex();
      host.players['Steve'] = steve;

      essentials.who(command('who', '', player: steve));

      final message = host.lastMessageTo(1);
      expect(message, contains('Alex'));
      expect(message, contains('Steve'));
    });

    test('registers every command and dispatches through PluginContext', () {
      final context = PluginContext(host);
      Essentials(host: context, homes: store).register(context);

      expect(
        host.commands.map((command) => command.name),
        containsAll(<String>[
          'spawn',
          'sethome',
          'home',
          'delhome',
          'homes',
          'tpa',
          'tpaccept',
          'tpdeny',
          'msg',
          'who',
          'heal',
        ]),
      );
      expect(host.events, contains(EventKind.playerLeave));

      context.executeCommand(command('heal', '', player: steve));

      expect(host.callsTo('player.set_health'), hasLength(1));
    });
  });
}
