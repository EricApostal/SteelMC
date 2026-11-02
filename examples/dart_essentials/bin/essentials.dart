import 'package:steel_essentials/essentials.dart';
import 'package:steel_essentials/home_store.dart';
import 'package:steel_extension/steel_plugin.dart';

/// Entry point started by the Rust bridge.
///
/// Homes live in `homes.json` in the working directory the host was given in
/// the sidecar manifest (`.` for this example).
void main() => runSteelPlugin(
      EssentialsPlugin(homeStore: HomeStore(path: 'homes.json')),
    );
