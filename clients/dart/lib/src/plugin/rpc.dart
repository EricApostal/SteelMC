import 'dart:convert';

import 'json.dart';
import 'model.dart';
import 'protocol.dart';
import 'transport.dart';

/// Thrown when the host connection closes while a reply is outstanding.
final class RpcClosedException implements Exception {
  RpcClosedException(this.message);

  /// Description of what was being awaited.
  final String message;

  @override
  String toString() => 'RpcClosedException: $message';
}

/// Thrown when a host call reports an error.
final class HostCallException implements Exception {
  HostCallException({required this.method, required this.error});

  /// Host method that failed.
  final String method;

  /// Error text reported by the host.
  final String error;

  @override
  String toString() => 'HostCallException($method): $error';
}

/// Newline-delimited JSON RPC over a [LineTransport].
///
/// The client is synchronous, matching the plugin runtime loop. [request] sends
/// a `host_call` and pumps inbound messages until the matching reply arrives;
/// anything unrelated is queued for the next [receive].
final class RpcClient {
  RpcClient(this._transport);

  final LineTransport _transport;
  final List<HostMessage> _deferred = <HostMessage>[];
  int _nextId = 1;

  /// Allocates an id for an outbound request.
  int allocateId() => _nextId++;

  /// Encodes [message] and writes it to the transport.
  void send(ClientMessage message) {
    _transport.writeLine(jsonEncode(encodeClientMessage(message)));
  }

  /// Returns the next inbound message, or `null` at end of input.
  ///
  /// Messages skipped while waiting for a host reply are returned first, so no
  /// command or event is lost when a handler makes a host call.
  HostMessage? receive() {
    if (_deferred.isNotEmpty) {
      return _deferred.removeAt(0);
    }
    final line = _transport.readLine();
    if (line == null) {
      return null;
    }
    final map = asJsonObject(jsonDecode(line), context: 'host message');
    return HostMessageMapper.fromMap(map.cast<String, dynamic>());
  }

  /// Sends a `host_call` and blocks until the host answers.
  ///
  /// Returns the decoded `result` of a `host_result`. Throws
  /// [HostCallException] for `host_error` and [RpcClosedException] if the
  /// transport closes first.
  Object? request(HostCallMessage message) {
    send(message);
    while (true) {
      final reply = receive();
      if (reply == null) {
        throw RpcClosedException(
            'connection closed while waiting for "${message.method}"');
      }
      switch (reply) {
        case HostResultMessage(:final id, :final result) when id == message.id:
          return result;
        case HostErrorMessage(:final id, :final error) when id == message.id:
          throw HostCallException(method: message.method, error: error);
        default:
          _deferred.add(reply);
      }
    }
  }

  /// Convenience wrapper around [request] for a method and its parameters.
  Object? call(
    String method, [
    Map<String, Object?> params = const <String, Object?>{},
  ]) =>
      request(HostCallMessage(
        id: allocateId(),
        method: method,
        params: params,
      ));

  /// Replies to the command or event with [id].
  void respond(int id, {bool cancel = false}) {
    send(ResponseMessage(id: id, cancel: cancel));
  }

  /// Writes a log line to the host.
  void log(String message, {String level = 'info'}) {
    send(LogMessage(level: level, message: message));
  }

  /// Reports that plugin registration finished.
  void ready() => send(const ReadyMessage());

  /// Announces a command to the host.
  void registerCommand(CommandRegistration command) {
    send(RegisterCommandMessage(
      name: command.name,
      description: command.description,
      permission: command.permission,
    ));
  }

  /// Subscribes to an event by its wire name.
  void registerEvent(String event) {
    send(RegisterEventMessage(event: event));
  }
}
