import 'dart:convert';

import 'json.dart';
import 'transport.dart';

/// Receives a `host_call` from the host while the Dart side is waiting for a
/// reply of its own.
typedef HostCallHandler = void Function(Map<String, Object?> call);

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
  RpcClient(this._transport, {this.onHostCall});

  final LineTransport _transport;

  /// Called for a nested `host_call` received while waiting for a reply.
  final HostCallHandler? onHostCall;

  final List<Map<String, Object?>> _deferred = <Map<String, Object?>>[];
  int _nextId = 1;

  /// Allocates an id for an outbound request.
  int allocateId() => _nextId++;

  /// Encodes [message] and writes it to the transport.
  void send(Map<String, Object?> message) {
    _transport.writeLine(jsonEncode(message));
  }

  /// Returns the next inbound message, or `null` at end of input.
  ///
  /// Messages skipped while waiting for a host reply are returned first, so no
  /// command or event is lost when a handler makes a host call.
  Map<String, Object?>? receive() {
    if (_deferred.isNotEmpty) {
      return _deferred.removeAt(0);
    }
    final line = _transport.readLine();
    if (line == null) {
      return null;
    }
    return asJsonObject(jsonDecode(line), context: 'host message');
  }

  /// Sends a `host_call` and blocks until the host answers.
  ///
  /// [message] must contain `method`; `params` defaults to an empty map and an
  /// `id` is allocated when omitted. While waiting, unrelated inbound messages
  /// are queued for [receive] and nested `host_call` messages are handed to
  /// [onHostCall].
  ///
  /// Returns the decoded `result` of a `host_result`. Throws
  /// [HostCallException] for `host_error` and [RpcClosedException] if the
  /// transport closes first.
  Object? request(Map<String, Object?> message) {
    final id = _int(message['id']) ?? allocateId();
    final method = message['method'];
    if (method is! String) {
      throw ArgumentError.value(message, 'message', 'missing "method"');
    }
    send(<String, Object?>{
      ...message,
      't': 'host_call',
      'id': id,
      'params': message['params'] ?? const <String, Object?>{},
    });

    while (true) {
      final reply = receive();
      if (reply == null) {
        throw RpcClosedException(
            'connection closed while waiting for "$method"');
      }
      final type = reply['t'];
      if (_int(reply['id']) == id) {
        if (type == 'host_result') {
          return reply['result'];
        }
        if (type == 'host_error') {
          throw HostCallException(
            method: method,
            error: reply['error']?.toString() ?? 'unknown host error',
          );
        }
      }
      if (type == 'host_call') {
        final handler = onHostCall;
        if (handler != null) {
          handler(reply);
          continue;
        }
      }
      _deferred.add(reply);
    }
  }

  /// Convenience wrapper around [request] for a method and its parameters.
  Object? call(
    String method, [
    Map<String, Object?> params = const <String, Object?>{},
  ]) =>
      request(<String, Object?>{'method': method, 'params': params});

  /// Replies to the command or event with [id].
  void respond(int id, {bool cancel = false, Object? result}) {
    send(<String, Object?>{
      't': 'response',
      'id': id,
      'cancel': cancel,
      'result': result,
    });
  }

  /// Writes a log line to the host.
  void log(String message, {String level = 'info'}) {
    send(<String, Object?>{'t': 'log', 'level': level, 'message': message});
  }

  /// Reports that plugin registration finished.
  void ready() => send(const <String, Object?>{'t': 'ready'});

  static int? _int(Object? value) => value is num ? value.toInt() : null;
}
