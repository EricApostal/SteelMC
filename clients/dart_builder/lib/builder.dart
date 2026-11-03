import 'package:build/build.dart';
import 'package:source_gen/source_gen.dart';

import 'src/steel_generator.dart';

/// Generates `<library>.steel.dart` registration metadata for `@SteelClass`
/// classes and their `@SteelMethod` instance methods.
Builder steelBuilder(BuilderOptions options) => PartBuilder(
      <Generator>[const SteelClassGenerator()],
      '.steel.dart',
      header: '// GENERATED CODE - DO NOT MODIFY BY HAND\n',
      options: options,
    );