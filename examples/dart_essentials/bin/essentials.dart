import 'dart:io';

import 'package:steel_essentials/essentials.dart';
import 'package:steel_essentials/greeter.dart';
import 'package:steel_essentials/home_store.dart';
import 'package:steel_extension/steel_plugin.dart';

/// Entry point invoked by the embedded Dart runtime.
///
/// Homes live in `homes.json` in the working directory the host was given in
/// the sidecar manifest (`.` for this example). The generated [steelClasses]
/// registers `Greeter` with the host ClassDB.
void main() => runSteelPlugin(
      EssentialsPlugin(
        homeStore: HomeStore(
          path: Platform.environment['STEEL_ESSENTIALS_HOMES'] ?? 'homes.json',
        ),
      ),
      classes: steelClasses,
    );