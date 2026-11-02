import 'dart:convert';
import 'dart:io';

/// A bidirectional stream of newline-delimited UTF-8 messages.
///
/// The plugin runtime is deliberately free of `dart:ffi`; it speaks JSON to the
/// Rust bridge through an implementation of this interface. Tests provide a
/// scripted transport so no subprocess is needed.
abstract interface class LineTransport {
  /// Reads one message, or returns `null` when the input is closed.
  String? readLine();

  /// Writes one message followed by a newline.
  void writeLine(String line);
}

/// [LineTransport] backed by the current process's standard input and output.
///
/// This is the transport used by the plugin runtime when none is supplied.
final class StdioTransport implements LineTransport {
  StdioTransport({Stdin? input, IOSink? output})
      : _input = input ?? stdin,
        _output = output ?? stdout;

  final Stdin _input;
  final IOSink _output;

  @override
  String? readLine() => _input.readLineSync(encoding: utf8);

  @override
  void writeLine(String line) {
    // `Stdout` writes through a synchronous, fd-backed `StreamConsumer`, so
    // `writeln` reaches the host before the runtime blocks on the next
    // `readLineSync`. Calling `flush()` here would bind the `IOSink` until its
    // future completes, making the next synchronous `writeln` throw
    // "StreamSink is bound to a stream".
    _output.writeln(line);
  }
}
