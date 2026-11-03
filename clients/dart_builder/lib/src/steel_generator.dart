import 'package:analyzer/dart/constant/value.dart';
import 'package:analyzer/dart/element/element.dart';
import 'package:analyzer/dart/element/type.dart';
import 'package:build/build.dart';
import 'package:source_gen/source_gen.dart';
import 'package:steel_extension/annotations.dart'
    show SteelValueType, steelMethodHash;

/// Generates `steelClasses` for `@SteelClass` annotated classes.
///
/// Only positional parameters of `int`, `double`, `bool` and `String` are
/// supported, and methods must return one of those types or `void`. Anything
/// else is reported as an [InvalidGenerationSourceError] with the offending
/// element.
class SteelClassGenerator extends Generator {
  const SteelClassGenerator();

  @override
  String? generate(LibraryReader library, BuildStep buildStep) {
    final classes = <_ClassSpec>[];
    for (final element in library.classes) {
      final annotation = _findAnnotation(element, 'SteelClass');
      if (annotation == null) {
        continue;
      }
      classes.add(_readClass(element, annotation));
    }
    if (classes.isEmpty) {
      return null;
    }
    final buffer = StringBuffer()
      ..writeln('/// Extension classes declared in this library.')
      ..writeln(
        'List<SteelClassRegistration> get steelClasses => '
        '<SteelClassRegistration>[',
      );
    for (final spec in classes) {
      _writeClass(buffer, spec);
    }
    buffer.writeln('];');
    return buffer.toString();
  }

  _ClassSpec _readClass(ClassElement element, DartObject annotation) {
    final dartName = element.name;
    if (dartName == null || dartName.startsWith('_')) {
      throw InvalidGenerationSourceError(
        '@SteelClass classes must be public so generated code can name them',
        element: element,
      );
    }
    if (element.isAbstract) {
      throw InvalidGenerationSourceError(
        '@SteelClass class $dartName must be concrete: the generator instantiates it',
        element: element,
      );
    }
    final className = annotation.getField('name')?.toStringValue();
    if (className == null || className.isEmpty) {
      throw InvalidGenerationSourceError(
        '@SteelClass on $dartName needs a non-empty host class name',
        element: element,
      );
    }
    final inherits =
        annotation.getField('inherits')?.toStringValue() ?? 'Object';
    ConstructorElement? constructor;
    for (final candidate in element.constructors) {
      if (candidate.isDefaultConstructor) {
        constructor = candidate;
        break;
      }
    }
    if (constructor == null) {
      throw InvalidGenerationSourceError(
        '@SteelClass class $dartName needs an unnamed constructor',
        element: element,
      );
    }
    if (constructor.formalParameters.isNotEmpty) {
      throw InvalidGenerationSourceError(
        '@SteelClass class $dartName needs an unnamed constructor without '
        'parameters; the host constructs it with no arguments',
        element: constructor,
      );
    }
    final methods = <_MethodSpec>[];
    for (final method in element.methods) {
      final methodAnnotation = _findAnnotation(method, 'SteelMethod');
      if (methodAnnotation == null) {
        continue;
      }
      methods.add(
        _readMethod(element, method, methodAnnotation, className),
      );
    }
    return _ClassSpec(
      dartName: dartName,
      className: className,
      inherits: inherits,
      methods: methods,
    );
  }

  _MethodSpec _readMethod(
    ClassElement owner,
    MethodElement method,
    DartObject annotation,
    String hostClassName,
  ) {
    final dartName = method.name;
    if (dartName == null) {
      throw InvalidGenerationSourceError(
        '@SteelMethod needs a named method',
        element: method,
      );
    }
    if (method.isStatic) {
      throw InvalidGenerationSourceError(
        '@SteelMethod is only supported on instance methods; '
        '$dartName is static',
        element: method,
      );
    }
    final className = annotation.getField('name')?.toStringValue();
    final methodName =
        className == null || className.isEmpty ? dartName : className;
    final argTypes = <SteelValueType>[];
    for (final parameter in method.formalParameters) {
      final parameterName = parameter.name ?? '?';
      if (parameter.isNamed) {
        throw InvalidGenerationSourceError(
          'parameter $parameterName of $dartName is named; '
          'Steel extension methods only support positional parameters',
          element: parameter,
        );
      }
      if (parameter.isOptionalPositional) {
        throw InvalidGenerationSourceError(
          'parameter $parameterName of $dartName is optional; '
          'Steel extension methods only support required positional parameters',
          element: parameter,
        );
      }
      argTypes.add(
        _valueType(
          parameter.type,
          owner: owner,
          member: dartName,
          name: parameterName,
        ),
      );
    }
    final returnType = _valueType(
      method.returnType,
      owner: owner,
      member: dartName,
      name: 'return value',
      allowVoid: true,
    );
    final signature =
        '$methodName(${argTypes.map(_dartTypeName).join(',')})'
        '->${_dartTypeName(returnType)}';
    return _MethodSpec(
      dartName: dartName,
      name: methodName,
      argTypes: argTypes,
      returnType: returnType,
      hash: steelMethodHash('$hostClassName::$signature'),
    );
  }

  SteelValueType _valueType(
    DartType type, {
    required ClassElement owner,
    required String member,
    required String name,
    bool allowVoid = false,
  }) {
    if (type.isDartCoreInt) {
      return SteelValueType.i32;
    }
    if (type.isDartCoreDouble) {
      return SteelValueType.f64;
    }
    if (type.isDartCoreBool) {
      return SteelValueType.boolean;
    }
    if (type.isDartCoreString) {
      return SteelValueType.string;
    }
    if (allowVoid && type is VoidType) {
      return SteelValueType.voidType;
    }
    throw InvalidGenerationSourceError(
      'unsupported type ${type.getDisplayString()} for $name of '
      '${owner.name}.$member; Steel extension methods support int, double, '
      'bool and String${allowVoid ? ', and void as a return type' : ''}',
      element: owner,
    );
  }

  void _writeClass(StringBuffer buffer, _ClassSpec spec) {
    buffer
      ..writeln('  SteelClassRegistration(')
      ..writeln("    name: ${_quote(spec.className)},")
      ..writeln("    inherits: ${_quote(spec.inherits)},")
      ..writeln('    construct: () => ${spec.dartName}(),');
    if (spec.methods.isEmpty) {
      buffer.writeln('    methods: const <SteelMethodRegistration>[],');
    } else {
      buffer.writeln('    methods: <SteelMethodRegistration>[');
      for (final method in spec.methods) {
        _writeMethod(buffer, spec, method);
      }
      buffer.writeln('    ],');
    }
    buffer.writeln('  ),');
  }

  void _writeMethod(StringBuffer buffer, _ClassSpec owner, _MethodSpec spec) {
    final argTypes = spec.argTypes
        .map((type) => 'SteelValueType.${type.name}')
        .join(', ');
    buffer
      ..writeln('      SteelMethodRegistration(')
      ..writeln("        name: ${_quote(spec.name)},")
      ..writeln('        hash: ${spec.hash},')
      ..writeln('        argTypes: const <SteelValueType>[$argTypes],')
      ..writeln('        returnType: SteelValueType.${spec.returnType.name},')
      ..writeln('        invoke: (Object instance, List<Object?> args) {');
    final arguments = <String>[
      for (var index = 0; index < spec.argTypes.length; index++)
        'args[$index] as ${_dartTypeName(spec.argTypes[index])}',
    ].join(', ');
    final call = '(instance as ${owner.dartName}).${spec.dartName}($arguments)';
    if (spec.returnType == SteelValueType.voidType) {
      buffer
        ..writeln('          $call;')
        ..writeln('          return null;');
    } else {
      buffer.writeln('          return $call;');
    }
    buffer
      ..writeln('        },')
      ..writeln('      ),');
  }
}

final class _ClassSpec {
  const _ClassSpec({
    required this.dartName,
    required this.className,
    required this.inherits,
    required this.methods,
  });

  final String dartName;
  final String className;
  final String inherits;
  final List<_MethodSpec> methods;
}

final class _MethodSpec {
  const _MethodSpec({
    required this.dartName,
    required this.name,
    required this.argTypes,
    required this.returnType,
    required this.hash,
  });

  final String dartName;
  final String name;
  final List<SteelValueType> argTypes;
  final SteelValueType returnType;
  final int hash;
}

DartObject? _findAnnotation(Element element, String name) {
  for (final annotation in element.metadata.annotations) {
    final annotationElement = annotation.element;
    if (annotationElement is ConstructorElement &&
        annotationElement.enclosingElement.name == name) {
      final value = annotation.computeConstantValue();
      if (value == null) {
        throw InvalidGenerationSourceError(
          'could not evaluate @$name on ${element.name}',
          element: element,
        );
      }
      return value;
    }
  }
  return null;
}

String _dartTypeName(SteelValueType type) => switch (type) {
      SteelValueType.i32 => 'int',
      SteelValueType.u32 => 'int',
      SteelValueType.f32 => 'double',
      SteelValueType.f64 => 'double',
      SteelValueType.boolean => 'bool',
      SteelValueType.string => 'String',
      SteelValueType.ref => 'List<int>',
      SteelValueType.voidType => 'void',
    };

String _quote(String value) {
  final escaped = value
      .replaceAll(r'\', r'\\')
      .replaceAll("'", r"\'")
      .replaceAll(r'$', r'\$');
  return "'$escaped'";
}