import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// One host call observed by [FakeSteelBinding].
final class HostCall {
  HostCall(this.className, this.methodName, this.hash, this.instance, this.args);

  final String className;
  final String methodName;
  final int hash;
  final SteelRef instance;
  final List<SteelArg> args;

  @override
  String toString() => '$className.$methodName(${args.map((a) => a.value)})';
}

/// In-memory [SteelBinding] with canned replies.
final class FakeSteelBinding implements SteelBinding {
  final List<HostCall> calls = <HostCall>[];
  final Map<int, String> names = <int, String>{1: 'Steve'};
  final Map<int, String> uuids = <int, String>{1: 'uuid-steve'};
  final Map<(int, int, int), int> blocks = <(int, int, int), int>{};
  final List<int> players = <int>[1];

  Vec3 position = const Vec3(0, 64, 0);
  Vec3 spawnPoint = const Vec3(0, 64, 0);
  double health = 20;
  int blockState = 100;
  int nextBlockState = 101;

  List<HostCall> callsTo(String method) => <HostCall>[
        for (final call in calls)
          if (call.methodName == method) call,
      ];

  @override
  SteelReturn ptrcall(
    String className,
    String methodName,
    int hash,
    SteelRef instance,
    List<SteelArg> args,
    SteelReturnKind returnKind,
  ) {
    calls.add(HostCall(className, methodName, hash, instance, args));
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
      case ('World', 'get_block'):
        final key = (
          args[0].value as int,
          args[1].value as int,
          args[2].value as int,
        );
        return SteelReturn(blocks[key] ?? 0);
      case ('World', 'set_block'):
        final key = (
          args[0].value as int,
          args[1].value as int,
          args[2].value as int,
        );
        blocks[key] = args[3].value as int;
        return SteelReturn(true);
      case ('World', 'spawn'):
        return SteelReturn(spawnPoint);
      case ('World', 'player_count'):
        return SteelReturn(players.length);
      case ('World', 'player_at'):
        final index = args[0].value as int;
        if (index >= players.length) {
          return SteelReturn(steelNoneRef());
        }
        return SteelReturn(
          steelEntityRef(instance.index, players[index]),
        );
      case ('Server', 'find_player'):
        final name = args[1].value as String;
        for (final entity in players) {
          if (names[entity] == name) {
            return SteelReturn(
              steelEntityRef(args[0].value as int, entity),
            );
          }
        }
        return SteelReturn(steelNoneRef());
      case ('Block', 'by_key'):
        final key = args[0].value as String;
        return SteelReturn(
          key == 'minecraft:stone' ? steelBlockRef(1) : steelNoneRef(),
        );
      case ('Block', 'default_state'):
        return SteelReturn(blockState);
      case ('BlockState', 'with_bool'):
      case ('BlockState', 'with_int'):
      case ('BlockState', 'with_enum'):
        return SteelReturn(nextBlockState);
      default:
        throw StateError('unexpected host call $className.$methodName');
    }
  }

  @override
  SteelRef construct(String className, [List<SteelArg> args = const <SteelArg>[]]) {
    calls.add(HostCall(className, '<construct>', 0, steelNoneRef(), args));
    return steelRef(11, 1, 1);
  }
}

/// In-memory [NativeShim].
final class FakeNativeShim implements NativeShim {
  final List<CommandRegistration> commands = <CommandRegistration>[];
  final List<int> commandIds = <int>[];
  final List<int> events = <int>[];
  final List<String> logs = <String>[];
  final List<RegisteredClass> classes = <RegisteredClass>[];

  @override
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  ) {
    commandIds.add(id);
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
    events.add(kind);
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
  }) {
    classes.add(
      RegisteredClass(
        name: name,
        inherits: inherits,
        methodNames: methodNames,
        methodHashes: methodHashes,
        argCounts: argCounts,
        argTypes: argTypes,
        returnTypes: returnTypes,
      ),
    );
    return 0;
  }

  @override
  int log(int level, String message) {
    logs.add('$level: $message');
    return 0;
  }
}

/// One class registration observed by [FakeNativeShim].
final class RegisteredClass {
  RegisteredClass({
    required this.name,
    required this.inherits,
    required this.methodNames,
    required this.methodHashes,
    required this.argCounts,
    required this.argTypes,
    required this.returnTypes,
  });

  final String name;
  final String inherits;
  final List<String> methodNames;
  final List<int> methodHashes;
  final List<int> argCounts;
  final List<int> argTypes;
  final List<int> returnTypes;
}

final class RegistrationPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    context
        .command('pos')
        .description('Show your position')
        .permission('test.pos')
        .executes((command) {
      final player = command.player;
      if (player == null) {
        return;
      }
      player.sendMessage('${player.name()} at ${player.position()}');
    });
    context.onPlayerJoin((player) {
      player.sendMessage('Welcome ${player.name()}');
    });
    context.onChat((event) {
      if (event.text == 'bad') {
        event.cancel();
      }
    });
  }
}

void main() {
  late FakeSteelBinding binding;
  late FakeNativeShim shim;
  late PluginContext context;

  setUp(() {
    binding = FakeSteelBinding();
    shim = FakeNativeShim();
    context = PluginContext(binding: binding, shim: shim);
  });

  test('registers commands and events through the native shim', () {
    RegistrationPlugin().onEnable(context);

    expect(shim.commands.single.name, 'pos');
    expect(shim.commands.single.description, 'Show your position');
    expect(shim.commands.single.permission, 'test.pos');
    expect(
      shim.events,
      containsAll(<int>[EventKind.playerJoin, EventKind.chat]),
    );
  });

  test('dispatches a command through the generated player API', () {
    RegistrationPlugin().onEnable(context);

    context.executeCommand(
      CommandContext(
        id: shim.commandIds.single,
        command: 'pos',
        args: '',
        player: context.player(0, 1),
      ),
    );

    final message = binding.callsTo('send_message').single;
    expect(message.args.single.value, 'Steve at Vec3(0.0, 64.0, 0.0)');
  });

  test('generated entity methods call through method binds', () {
    final player = context.player(0, 1);

    expect(player.name(), 'Steve');
    expect(player.uuid(), 'uuid-steve');
    expect(player.health(), 20);
    expect(player.setHealth(12), isTrue);
    expect(binding.health, 12);
    expect(player.teleport(const Vec3(1, 2, 3), 90, 15), isTrue);
    expect(binding.position, const Vec3(1, 2, 3));

    final teleport = binding.callsTo('teleport').single;
    expect(teleport.instance.kind, 12);
    expect(teleport.instance.index, 0);
    expect(teleport.instance.generation, 1);
    expect(teleport.hash, 3148085413);
    expect(teleport.args.map((arg) => arg.kind), <SteelArgKind>[
      SteelArgKind.vec3,
      SteelArgKind.f32,
      SteelArgKind.f32,
    ]);
  });

  test('world block access uses get_block and set_block', () {
    binding.blocks[(1, 64, 2)] = 42;
    final world = context.world(0);

    expect(world.getBlock(1, 64, 2).id, 42);
    expect(world.setBlock(1, 64, 2, BlockState(binding, 7), 3), isTrue);
    expect(binding.blocks[(1, 64, 2)], 7);

    final call = binding.callsTo('set_block').single;
    expect(call.args.map((arg) => arg.kind), <SteelArgKind>[
      SteelArgKind.i32,
      SteelArgKind.i32,
      SteelArgKind.i32,
      SteelArgKind.u32,
      SteelArgKind.u32,
    ]);
    expect(call.args.map((arg) => arg.value), <Object>[1, 64, 2, 7, 3]);
  });

  test('block and block state handles use generated static methods', () {
    final block = context.block('minecraft:stone');

    expect(block, isNotNull);
    expect(block!.id, 1);
    expect(block.defaultState().id, 100);

    final state = block.defaultState();
    expect(state.withBool('lit', true).id, 101);
    expect(state.withInt('level', 3).id, 101);
    expect(state.withEnum('axis', 'x').id, 101);

    final call = binding.callsTo('with_bool').single;
    expect(call.args[0].value, 'lit');
    expect(call.args[1].value, isTrue);
    expect(context.block('minecraft:missing'), isNull);
  });

  test('player queries use server and world binds', () {
    binding.spawnPoint = const Vec3(1, 2, 3);

    expect(context.spawn(), const Vec3(1, 2, 3));
    expect(context.findPlayer('Steve')?.uuid(), 'uuid-steve');
    expect(context.findPlayer('Alex'), isNull);
    expect(context.onlinePlayers(), hasLength(1));
    expect(context.onlinePlayers().single.name(), 'Steve');
  });

  test('events can be cancelled', () {
    RegistrationPlugin().onEnable(context);
    final event = ChatEvent(
      id: 1,
      world: 0,
      position: const Vec3(0, 64, 0),
      player: context.player(0, 1),
      text: 'bad',
    );

    context.dispatchChat(event);

    expect(event.isCancelled, isTrue);
  });

  test('block events and player lifecycle reach their handlers', () {
    final placed = <int>[];
    final joined = <String>[];
    context.onBlockPlace((event) => placed.add(event.blockState));
    context.onBlockBreak((event) => placed.add(-event.blockState));
    context.onPlayerJoin((player) => joined.add(player.name()));
    context.onPlayerLeave((player) => joined.remove(player.name()));

    context.dispatchBlockPlace(
      BlockEvent(
        id: 0,
        world: 0,
        position: const Vec3(0, 64, 0),
        player: context.player(0, 1),
        blockState: 5,
      ),
    );
    context.dispatchBlockBreak(
      BlockEvent(
        id: 0,
        world: 0,
        position: const Vec3(0, 64, 0),
        blockState: 5,
      ),
    );
    context.dispatchPlayerJoin(context.player(0, 1));
    expect(joined, <String>['Steve']);

    context.dispatchPlayerLeave(context.player(0, 1));

    expect(placed, <int>[5, -5]);
    expect(joined, isEmpty);
  });

  test('reference helpers encode the handle kinds the host expects', () {
    final world = steelWorldRef(4);
    expect(world.kind, SteelConstants.steelRefWorld);
    expect(world.index, 4);
    expect(world.generation, 0);

    final entity = steelEntityRef(4, 9);
    expect(entity.kind, SteelConstants.steelRefEntity);
    expect(entity.index, 4);
    expect(entity.generation, 9);

    expect(steelBlockRef(2).kind, SteelConstants.steelRefBlock);
    expect(steelBlockStateRef(3).kind, SteelConstants.steelRefBlockState);
    expect(steelItemRef(5).kind, SteelConstants.steelRefItem);
    expect(steelNoneRef().kind, SteelConstants.steelRefNone);
  });

  test('log forwards the level mapped to the host convention', () {
    context.log('hello', level: LogLevel.debug);
    context.log('careful', level: LogLevel.warn);

    expect(shim.logs, <String>['3: hello', '1: careful']);
  });
}