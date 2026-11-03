import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// In-memory [PluginHost] used to exercise the public plugin API without the
/// embedded Dart VM.
final class FakePluginHost implements PluginHost {
  final List<CommandRegistration> commands = <CommandRegistration>[];
  final List<int> commandIds = <int>[];
  final List<int> events = <int>[];
  final List<String> messages = <String>[];
  final List<(int, int, String)> logs = <(int, int, String)>[];
  final Map<int, Player> players = <int, Player>{};

  Vec3 position = const Vec3(0, 64, 0);
  Vec3 spawn = const Vec3(0, 64, 0);
  double health = 20;

  @override
  Player? playerData(int world, int entity) => players[entity];

  @override
  Vec3 entityPosition(int world, int entity) => position;

  @override
  void entityTeleport(
    int world,
    int entity,
    Vec3 position, {
    double yaw = 0,
    double pitch = 0,
  }) {
    this.position = position;
  }

  @override
  void playerSendMessage(int world, int entity, String message) {
    messages.add(message);
  }

  @override
  double entityHealth(int world, int entity) => health;

  @override
  void entitySetHealth(int world, int entity, double health) {
    this.health = health;
  }

  @override
  Vec3 worldSpawn(int world) => spawn;

  @override
  List<Player> worldPlayers(int world) => players.values.toList();

  @override
  Player? findPlayerInWorld(int world, String name) {
    for (final player in players.values) {
      if (player.name == name) {
        return player;
      }
    }
    return null;
  }

  @override
  void log(String message, {LogLevel level = LogLevel.info}) {
    logs.add((level.index, 0, message));
  }

  @override
  void registerCommand(int id, CommandRegistration command) {
    commandIds.add(id);
    commands.add(command);
  }

  @override
  void registerEvent(int kind) {
    events.add(kind);
  }
}

final class RegistrationPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    context
        .command('pos')
        .description('Show your position')
        .permission('test.pos')
        .executes((command) {
      command.player?.sendMessage('${command.player?.position()}');
    });
    context.onPlayerJoin((player) {});
    context.onChat((event) {
      if (event.text == 'bad') {
        event.cancel();
      }
    });
  }
}

void main() {
  late FakePluginHost host;
  late PluginContext context;

  setUp(() {
    host = FakePluginHost();
    host.players[1] = Player(
      entity: 1,
      world: 0,
      name: 'Steve',
      uuid: 'uuid-steve',
      host: host,
    );
    context = PluginContext(host);
  });

  test('registers commands and events', () {
    RegistrationPlugin().onEnable(context);

    expect(host.commands.single.name, 'pos');
    expect(host.commands.single.description, 'Show your position');
    expect(host.commands.single.permission, 'test.pos');
    expect(
      host.events,
      containsAll(<int>[
        EventKind.playerJoin,
        EventKind.chat,
      ]),
    );
  });

  test('dispatches a command through the typed player API', () {
    RegistrationPlugin().onEnable(context);
    final player = host.players[1]!;

    context.executeCommand(
      CommandContext(
        id: host.commandIds.single,
        command: 'pos',
        args: '',
        player: player,
        host: host,
      ),
    );

    expect(host.messages, <String>['Vec3(0.0, 64.0, 0.0)']);
  });

  test('events can be cancelled', () {
    RegistrationPlugin().onEnable(context);
    final event = ChatEvent(
      id: 1,
      world: 0,
      position: const Vec3(0, 64, 0),
      player: host.players[1],
      text: 'bad',
    );

    context.dispatchChat(event);

    expect(event.isCancelled, isTrue);
  });

  test('typed world and server helpers use the host', () {
    host.spawn = const Vec3(1, 2, 3);
    expect(context.spawn(), const Vec3(1, 2, 3));
    expect(context.findPlayer('Steve')?.uuid, 'uuid-steve');
    expect(context.onlinePlayers(), hasLength(1));
  });
}
