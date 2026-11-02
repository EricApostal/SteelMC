import 'dart:convert';

import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// [LineTransport] that replays scripted inbound messages and records what the
/// plugin writes. No process or stdin is involved.
final class ScriptedTransport implements LineTransport {
  ScriptedTransport([Iterable<String> inbound = const <String>[]])
      : _inbound = List<String>.of(inbound);

  final List<String> _inbound;
  final List<Map<String, Object?>> written = <Map<String, Object?>>[];
  int _readIndex = 0;

  @override
  String? readLine() =>
      _readIndex < _inbound.length ? _inbound[_readIndex++] : null;

  @override
  void writeLine(String line) {
    final decoded = jsonDecode(line);
    written.add(Map<String, Object?>.from(decoded as Map));
  }

  /// Messages written by the plugin with type [type], in order.
  List<Map<String, Object?>> ofType(String type) => <Map<String, Object?>>[
        for (final message in written)
          if (message['t'] == type) message,
      ];
}

final class RegistrationPlugin implements SteelPlugin {
  @override
  void onEnable(PluginContext context) {
    context
        .command('pos')
        .description('Show your position')
        .permission('test.pos')
        .executes((command) => null);
    context.onPlayerJoin((player) {});
  }
}

final class CommandPlugin implements SteelPlugin {
  Vec3? seen;

  @override
  void onEnable(PluginContext context) {
    context.command('pos').executes((command) {
      seen = command.player!.position();
      return command.player!.name;
    });
  }
}

final class ChatPlugin implements SteelPlugin {
  String? lastText;

  @override
  void onEnable(PluginContext context) {
    context.onChat((event) {
      lastText = event.text;
      if (event.text == 'bad') {
        event.cancel();
      }
    });
    context.onBlockPlace((event) {});
  }
}

const String _playerJson = '{"entity":42,"world":0,"name":"Steve",'
    '"uuid":"uuid-steve"}';

void main() {
  group('registration', () {
    test('registers commands and events before announcing ready', () {
      final transport = ScriptedTransport(<String>[
        '{"t":"init","level":1}',
        '{"t":"shutdown"}',
      ]);

      runSteelPlugin(RegistrationPlugin(), transport: transport);

      expect(transport.written, <Map<String, Object?>>[
        <String, Object?>{
          't': 'register_command',
          'name': 'pos',
          'description': 'Show your position',
          'permission': 'test.pos',
        },
        <String, Object?>{'t': 'register_event', 'event': 'player_join'},
        <String, Object?>{'t': 'ready'},
      ]);
    });

    test('does not enable the plugin before init level 1', () {
      final transport = ScriptedTransport(<String>[
        '{"t":"init","level":0}',
        '{"t":"shutdown"}',
      ]);

      runSteelPlugin(RegistrationPlugin(), transport: transport);

      expect(transport.written, isEmpty);
    });
  });

  group('commands', () {
    test('invokes the handler, calls the host and answers', () {
      final plugin = CommandPlugin();
      final transport = ScriptedTransport(<String>[
        '{"t":"init","level":1}',
        '{"t":"command","id":7,"player":$_playerJson,'
            '"command":"pos","args":""}',
        '{"t":"host_result","id":1,"ok":true,'
            '"result":{"x":10,"y":64,"z":-3}}',
        '{"t":"shutdown"}',
      ]);

      runSteelPlugin(plugin, transport: transport);

      expect(plugin.seen, const Vec3(10, 64, -3));

      final hostCalls = transport.ofType('host_call');
      expect(hostCalls, hasLength(1));
      expect(hostCalls.single, <String, Object?>{
        't': 'host_call',
        'id': 1,
        'method': 'player.position',
        'params': <String, Object?>{'entity': 42, 'world': 0},
      });

      expect(transport.ofType('response'), <Map<String, Object?>>[
        <String, Object?>{
          't': 'response',
          'id': 7,
          'cancel': false,
          'result': 'Steve',
        },
      ]);
    });
  });

  group('events', () {
    test('cancels chat but not block placement', () {
      final plugin = ChatPlugin();
      final transport = ScriptedTransport(<String>[
        '{"t":"init","level":1}',
        '{"t":"event","id":8,"event":"chat","player":$_playerJson,'
            '"world":0,"x":1,"y":2,"z":3,"text":"bad"}',
        '{"t":"event","id":9,"event":"block_place","player":$_playerJson,'
            '"world":0,"x":4,"y":5,"z":6,"block_state":5}',
        '{"t":"shutdown"}',
      ]);

      runSteelPlugin(plugin, transport: transport);

      expect(plugin.lastText, 'bad');
      expect(
        transport.ofType('register_event'),
        <Map<String, Object?>>[
          <String, Object?>{'t': 'register_event', 'event': 'chat'},
          <String, Object?>{'t': 'register_event', 'event': 'block_place'},
        ],
      );
      expect(transport.ofType('response'), <Map<String, Object?>>[
        <String, Object?>{
          't': 'response',
          'id': 8,
          'cancel': true,
          'result': null,
        },
        <String, Object?>{
          't': 'response',
          'id': 9,
          'cancel': false,
          'result': null,
        },
      ]);
    });
  });

  group('RpcClient', () {
    test('returns the result of the matching host_result', () {
      final transport = ScriptedTransport(<String>[
        '{"t":"host_result","id":1,"ok":true,"result":"Steve"}',
      ]);
      final rpc = RpcClient(transport);

      final result = rpc.request(<String, Object?>{
        'method': 'player.name',
        'params': <String, Object?>{'entity': 1, 'world': 0},
      });

      expect(result, 'Steve');
      expect(transport.written.single, <String, Object?>{
        't': 'host_call',
        'id': 1,
        'method': 'player.name',
        'params': <String, Object?>{'entity': 1, 'world': 0},
      });
    });

    test('handles a nested host_call while waiting for its result', () {
      final transport = ScriptedTransport(<String>[
        '{"t":"host_call","id":99,"method":"log",'
            '"params":{"message":"nested"}}',
        '{"t":"host_result","id":1,"ok":true,"result":42}',
      ]);
      final nested = <Map<String, Object?>>[];
      final rpc = RpcClient(transport, onHostCall: nested.add);

      final result = rpc.request(<String, Object?>{
        'method': 'player.health',
        'params': <String, Object?>{'entity': 1, 'world': 0},
      });

      expect(result, 42);
      expect(nested, hasLength(1));
      expect(nested.single['method'], 'log');
    });

    test('throws HostCallException for host_error', () {
      final transport = ScriptedTransport(<String>[
        '{"t":"host_error","id":1,"error":"no such player"}',
      ]);
      final rpc = RpcClient(transport);

      expect(
        () => rpc.request(<String, Object?>{'method': 'player.teleport'}),
        throwsA(
          isA<HostCallException>().having(
            (error) => error.error,
            'error',
            'no such player',
          ),
        ),
      );
    });
  });
}
