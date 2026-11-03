import 'dart:io';

import 'package:steel_essentials/essentials.dart';
import 'package:steel_essentials/home_store.dart';
import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// One host method invocation recorded by [FakeSteelBinding].
final class HostCall {
  HostCall(this.className, this.methodName, this.instance, this.args);

  final String className;
  final String methodName;
  final SteelRef instance;
  final List<SteelArg> args;

  @override
  String toString() => '$className.$methodName(${args.map((a) => a.value)})';
}

/// In-memory [SteelBinding] with canned replies.
///
/// This is the whole point of the binding abstraction: command logic runs
/// without stdin, a Dart VM process, or a Rust bridge.
final class FakeSteelBinding implements SteelBinding {
  final List<HostCall> calls = <HostCall>[];
  final Map<int, String> names = <int, String>{};
  final Map<int, String> uuids = <int, String>{};
  final List<int> playerEntities = <int>[1];

  Vec3 position = const Vec3(0, 64, 0);
  Vec3 spawnPoint = const Vec3(0, 64, 0);
  double health = 20;

  List<HostCall> callsTo(String method) => <HostCall>[
        for (final call in calls)
          if (call.methodName == method) call,
      ];

  String? lastMessageTo(int entity) {
    for (final call in calls.reversed) {
      if (call.methodName == 'send_message' &&
          call.instance.generation == entity) {
        return call.args.single.value as String;
      }
    }
    return null;
  }

  @override
  SteelRef construct(String className, [List<SteelArg> args = const <SteelArg>[]]) {
    calls.add(HostCall(className, '<construct>', steelNoneRef(), args));
    return steelRef(11, 1, 1);
  }

  @override
  SteelReturn ptrcall(
    String className,
    String methodName,
    int hash,
    SteelRef instance,
    List<SteelArg> args,
    SteelReturnKind returnKind,
  ) {
    calls.add(HostCall(className, methodName, instance, args));
    switch ((className, methodName)) {
      case ('Player', 'name'):
        return SteelReturn(names[instance.generation] ?? '');
      case ('Player', 'uuid'):
        return SteelReturn(uuids[instance.generation] ?? '');
      case ('Player', 'send_message'):
        return SteelReturn(true);
      case ('Entity', 'position'):
        return SteelReturn(position);
      case ('Entity', 'teleport'):
        position = args[0].value as Vec3;
        return SteelReturn(true);
      case ('Entity', 'health'):
        return SteelReturn(health);
      case ('Entity', 'set_health'):
        health = args[0].value as double;
        return SteelReturn(true);
      case ('World', 'spawn'):
        return SteelReturn(spawnPoint);
      case ('World', 'player_count'):
        return SteelReturn(playerEntities.length);
      case ('World', 'player_at'):
        final index = args[0].value as int;
        if (index >= playerEntities.length) {
          return SteelReturn(steelNoneRef());
        }
        return SteelReturn(
          steelEntityRef(instance.index, playerEntities[index]),
        );
      case ('Block', 'by_key'):
        final key = args[0].value as String;
        return SteelReturn(
          key == 'minecraft:stone' ? steelBlockRef(1) : steelNoneRef(),
        );
      case ('Block', 'default_state'):
        return SteelReturn(42);
      case ('World', 'set_block'):
        return SteelReturn(true);
      case ('Server', 'find_player'):
        final name = args[1].value as String;
        for (final entity in playerEntities) {
          if (names[entity] == name) {
            return SteelReturn(
              steelEntityRef(args[0].value as int, entity),
            );
          }
        }
        return SteelReturn(steelNoneRef());
      default:
        throw StateError('unexpected host call $className.$methodName');
    }
  }
}

/// In-memory [NativeShim].
final class FakeNativeShim implements NativeShim {
  final List<CommandRegistration> commands = <CommandRegistration>[];
  final List<int> events = <int>[];
  final List<String> logs = <String>[];

  @override
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  ) {
    commands.add(
      CommandRegistration(
        name: name,
        description: description,
        permission: permission,
      ),
    );
    return 0;
  }

  @override
  int registerEvent(int kind) {
    if (!events.contains(kind)) {
      events.add(kind);
    }
    return 0;
  }

  @override
  int registerClass({
    required String name,
    required String inherits,
    required List<String> methodNames,
    required List<int> methodHashes,
    required List<int> argCounts,
    required List<int> argTypes,
    required List<int> returnTypes,
  }) =>
      0;

  @override
  int log(int level, String message) {
    logs.add('$level: $message');
    return 0;
  }
}

void main() {
  late Directory tempDir;
  late FakeSteelBinding binding;
  late FakeNativeShim shim;
  late PluginContext context;
  late HomeStore store;
  late Player steve;
  late Essentials essentials;

  setUp(() {
    tempDir = Directory.systemTemp.createTempSync('steel_essentials_');
    binding = FakeSteelBinding()
      ..names[1] = 'Steve'
      ..uuids[1] = 'uuid-steve';
    shim = FakeNativeShim();
    context = PluginContext(binding: binding, shim: shim);
    store = HomeStore(path: '${tempDir.path}/homes.json')..load();
    steve = context.player(0, 1);
    essentials = Essentials(context: context, homes: store);
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
      );

  Player alex() {
    binding.names[2] = 'Alex';
    binding.uuids[2] = 'uuid-alex';
    if (!binding.playerEntities.contains(2)) {
      binding.playerEntities.add(2);
    }
    return context.player(0, 2);
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
    test('/setblock resolves a block and places its default state', () {
      essentials.setBlock(
        command('setblock', '1 64 2 minecraft:stone', player: steve),
      );

      final setBlock = binding.callsTo('set_block').single;
      expect(setBlock.instance.kind, SteelConstants.steelRefWorld);
      expect(setBlock.args.map((arg) => arg.value), <Object>[
        1,
        64,
        2,
        42,
        3,
      ]);
      expect(binding.lastMessageTo(1), contains('Placed'));
    });

    test('/setblock reports an unknown block', () {
      essentials.setBlock(
        command('setblock', '1 64 2 minecraft:missing', player: steve),
      );

      expect(binding.callsTo('set_block'), isEmpty);
      expect(binding.lastMessageTo(1), contains('Unknown block'));
    });

    test('/sethome stores the current position as the default home', () {
      binding.position = const Vec3(5, 66, 5);

      essentials.setHome(command('sethome', '', player: steve));

      expect(store.get('uuid-steve', 'home'), const Vec3(5, 66, 5));
      expect(binding.lastMessageTo(1), contains('Home "home" set'));
    });

    test('/sethome honors a name and normalizes its case', () {
      binding.position = const Vec3(1, 2, 3);

      essentials.setHome(command('sethome', 'Base', player: steve));

      expect(store.get('uuid-steve', 'base'), const Vec3(1, 2, 3));
    });

    test('/sethome rejects an invalid name', () {
      essentials.setHome(command('sethome', '!!!', player: steve));

      expect(store.namesOf('uuid-steve'), isEmpty);
      expect(binding.lastMessageTo(1), contains('Usage'));
    });

    test('/home teleports to the stored position', () {
      store.set('uuid-steve', 'base', const Vec3(10, 70, -4));

      essentials.home(command('home', 'base', player: steve));

      final teleport = binding.callsTo('teleport').single;
      expect(teleport.instance.generation, 1);
      expect(teleport.args[0].value, const Vec3(10, 70, -4));
      expect(teleport.args[1].value, 0.0);
      expect(teleport.args[2].value, 0.0);
      expect(binding.lastMessageTo(1), contains('Welcome home'));
    });

    test('/home reports an unknown home', () {
      essentials.home(command('home', 'missing', player: steve));

      expect(binding.callsTo('teleport'), isEmpty);
      expect(binding.lastMessageTo(1), contains('not found'));
    });

    test('/delhome removes an existing home', () {
      store.set('uuid-steve', 'base', const Vec3(1, 1, 1));

      essentials.deleteHome(command('delhome', 'base', player: steve));

      expect(store.get('uuid-steve', 'base'), isNull);
      expect(binding.lastMessageTo(1), contains('deleted'));
    });

    test('/homes lists names in sorted order', () {
      store
        ..set('uuid-steve', 'zebra', const Vec3(0, 0, 0))
        ..set('uuid-steve', 'alpha', const Vec3(0, 0, 0));

      essentials.listHomes(command('homes', '', player: steve));

      expect(binding.lastMessageTo(1), contains('alpha, zebra'));
    });

    test('/spawn teleports to the world spawn', () {
      binding.spawnPoint = const Vec3(0, 64, 0);

      essentials.spawn(command('spawn', '', player: steve));

      final teleport = binding.callsTo('teleport').single;
      expect(teleport.args[0].value, const Vec3(0, 64, 0));
    });

    test('/heal restores full health', () {
      essentials.heal(command('heal', '', player: steve));

      final setHealth = binding.callsTo('set_health').single;
      expect(setHealth.args.single.value, Essentials.maxHealth);
      expect(binding.lastMessageTo(1), contains('healed'));
    });

    test('/tpa and /tpaccept teleport the requester', () {
      final alexPlayer = alex();

      essentials.tpa(command('tpa', 'Alex', player: steve));
      expect(binding.lastMessageTo(2), contains('Steve'));

      binding.position = const Vec3(4, 65, 4);
      essentials.acceptTeleport(command('tpaccept', '', player: alexPlayer));

      final teleport = binding.callsTo('teleport').single;
      expect(teleport.instance.generation, 1);
      expect(teleport.args[0].value, const Vec3(4, 65, 4));
      expect(binding.lastMessageTo(1), contains('accepted'));
    });

    test('/tpdeny discards the request', () {
      final alexPlayer = alex();

      essentials.tpa(command('tpa', 'Alex', player: steve));
      essentials.denyTeleport(command('tpdeny', '', player: alexPlayer));

      expect(binding.callsTo('teleport'), isEmpty);
      expect(binding.lastMessageTo(1), contains('denied'));
    });

    test('/msg delivers a private message', () {
      alex();

      essentials.messagePlayer(
        command('msg', 'Alex hello there', player: steve),
      );

      expect(binding.lastMessageTo(2), contains('hello there'));
      expect(binding.lastMessageTo(1), contains('hello there'));
    });

    test('/who lists online players', () {
      alex();

      essentials.who(command('who', '', player: steve));

      final message = binding.lastMessageTo(1);
      expect(message, contains('Alex'));
      expect(message, contains('Steve'));
    });

    test('registers every command and dispatches through PluginContext', () {
      Essentials(context: context, homes: store).register();

      expect(
        shim.commands.map((command) => command.name),
        containsAll(<String>[
          'spawn',
          'setblock',
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
      expect(shim.events, contains(EventKind.playerLeave));

      context.executeCommand(command('heal', '', player: steve));

      expect(binding.callsTo('set_health'), hasLength(1));
    });
  });
}