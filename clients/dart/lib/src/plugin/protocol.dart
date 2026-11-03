import 'package:dart_mappable/dart_mappable.dart';

part 'protocol.mapper.dart';

/// Snapshot of a player as sent by the host.
@MappableClass()
final class PlayerData with PlayerDataMappable {
  const PlayerData({
    required this.entity,
    required this.world,
    required this.name,
    required this.uuid,
  });

  final int entity;
  final int world;
  final String name;
  final String uuid;
}

/// Host -> plugin messages.
@MappableClass(discriminatorKey: 't')
sealed class HostMessage with HostMessageMappable {
  const HostMessage();
}

@MappableClass(discriminatorValue: 'init')
final class InitMessage extends HostMessage with InitMessageMappable {
  const InitMessage({required this.level});
  final int level;
}

@MappableClass(discriminatorValue: 'command')
final class CommandMessage extends HostMessage with CommandMessageMappable {
  const CommandMessage({
    required this.id,
    this.player,
    required this.command,
    required this.args,
  });

  final int id;
  final PlayerData? player;
  final String command;
  final String args;
}

@MappableClass(discriminatorValue: 'event')
final class EventMessage extends HostMessage with EventMessageMappable {
  const EventMessage({
    required this.id,
    required this.event,
    this.player,
    required this.world,
    required this.x,
    required this.y,
    required this.z,
    @MappableField(key: 'block_state') required this.blockState,
    required this.text,
  });

  final int id;
  final String event;
  final PlayerData? player;
  final int world;
  final double x;
  final double y;
  final double z;
  final int blockState;
  final String text;
}

@MappableClass(discriminatorValue: 'host_result')
final class HostResultMessage extends HostMessage with HostResultMessageMappable {
  const HostResultMessage({
    required this.id,
    required this.ok,
    this.result,
  });

  final int id;
  final bool ok;
  final Object? result;
}

@MappableClass(discriminatorValue: 'host_error')
final class HostErrorMessage extends HostMessage with HostErrorMessageMappable {
  const HostErrorMessage({required this.id, required this.error});
  final int id;
  final String error;
}

@MappableClass(discriminatorValue: 'shutdown')
final class ShutdownMessage extends HostMessage with ShutdownMessageMappable {
  const ShutdownMessage();
}

/// Plugin -> host messages.
@MappableClass(discriminatorKey: 't')
sealed class ClientMessage with ClientMessageMappable {
  const ClientMessage();
}

@MappableClass(discriminatorValue: 'register_command')
final class RegisterCommandMessage extends ClientMessage
    with RegisterCommandMessageMappable {
  const RegisterCommandMessage({
    required this.name,
    required this.description,
    this.permission,
  });

  final String name;
  final String description;
  final String? permission;
}

@MappableClass(discriminatorValue: 'register_event')
final class RegisterEventMessage extends ClientMessage
    with RegisterEventMessageMappable {
  const RegisterEventMessage({required this.event});
  final String event;
}

@MappableClass(discriminatorValue: 'host_call')
final class HostCallMessage extends ClientMessage with HostCallMessageMappable {
  const HostCallMessage({
    required this.id,
    required this.method,
    this.params = const <String, Object?>{},
  });

  final int id;
  final String method;
  final Map<String, Object?> params;
}

@MappableClass(discriminatorValue: 'response')
final class ResponseMessage extends ClientMessage with ResponseMessageMappable {
  const ResponseMessage({required this.id, this.cancel = false});
  final int id;
  final bool cancel;
}

@MappableClass(discriminatorValue: 'log')
final class LogMessage extends ClientMessage with LogMessageMappable {
  const LogMessage({required this.level, required this.message});
  final String level;
  final String message;
}

@MappableClass(discriminatorValue: 'ready')
final class ReadyMessage extends ClientMessage with ReadyMessageMappable {
  const ReadyMessage();
}

/// Encodes a client message with its `t` discriminator.
Map<String, Object?> encodeClientMessage(ClientMessage message) =>
    switch (message) {
      final RegisterCommandMessage m => m.toMap(),
      final RegisterEventMessage m => m.toMap(),
      final HostCallMessage m => m.toMap(),
      final ResponseMessage m => m.toMap(),
      final LogMessage m => m.toMap(),
      final ReadyMessage m => m.toMap(),
    };
