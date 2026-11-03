import 'package:steel_essentials/greeter.dart';
import 'package:steel_extension/steel_plugin.dart';
import 'package:test/test.dart';

/// One class registration observed by [RecordingShim].
final class ClassRegistrationCall {
  ClassRegistrationCall({
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

/// In-memory [NativeShim] that records registration metadata.
final class RecordingShim implements NativeShim {
  final List<ClassRegistrationCall> classes = <ClassRegistrationCall>[];

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
      ClassRegistrationCall(
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
  int registerCommand(
    int id,
    String name,
    String description,
    String permission,
  ) =>
      0;

  @override
  int registerEvent(int kind) => 0;

  @override
  int log(int level, String message) => 0;
}

void main() {
  late RecordingShim shim;
  late ClassRegistrationCall registration;
  late int instance;

  setUpAll(() {
    shim = RecordingShim();
    final classId = registerExtensionClasses(steelClasses, shim).single;
    registration = shim.classes.single;
    instance = steelConstructExtension(classId);
  });

  test('the generator emits ClassDB metadata for Greeter', () {
    expect(registration.name, 'Greeter');
    expect(registration.inherits, 'Object');
    expect(
      registration.methodNames,
      <String>['greet', 'count', 'increment', 'half', 'toggle', 'reset'],
    );
    expect(registration.argCounts, <int>[1, 0, 1, 1, 1, 0]);
    expect(registration.argTypes, <int>[
      SteelValueType.string.index,
      SteelValueType.i32.index,
      SteelValueType.f64.index,
      SteelValueType.boolean.index,
    ]);
    expect(registration.returnTypes, <int>[
      SteelValueType.string.index,
      SteelValueType.i32.index,
      SteelValueType.i32.index,
      SteelValueType.f64.index,
      SteelValueType.boolean.index,
      SteelValueType.voidType.index,
    ]);
    expect(registration.methodHashes, <int>[
      steelMethodHash('Greeter::greet(String)->String'),
      steelMethodHash('Greeter::count()->int'),
      steelMethodHash('Greeter::increment(int)->int'),
      steelMethodHash('Greeter::half(double)->double'),
      steelMethodHash('Greeter::toggle(bool)->bool'),
      steelMethodHash('Greeter::reset()->void'),
    ]);
  });

  test('generated invoke callbacks dispatch through steelInvokeExtension', () {
    final greet = steelExtensionMethodId('Greeter', 'greet')!;
    expect(
      steelInvokeExtension(greet, instance, <Object?>['Steve']),
      'Hello, Steve!',
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'count')!,
        instance,
        const <Object?>[],
      ),
      1,
    );
    final increment = steelExtensionMethodId('Greeter', 'increment')!;
    expect(steelInvokeExtension(increment, instance, <Object?>[4]), 5);
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'half')!,
        instance,
        <Object?>[7.0],
      ),
      3.5,
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'toggle')!,
        instance,
        <Object?>[true],
      ),
      isFalse,
    );
    expect(
      steelInvokeExtension(
        steelExtensionMethodId('Greeter', 'reset')!,
        instance,
        const <Object?>[],
      ),
      isNull,
    );
  });

  test('every generated class has a working constructor', () {
    expect(steelClasses, isNotEmpty);
    for (final registration in steelClasses) {
      expect(registration.construct(), isA<Greeter>());
    }
  });
}