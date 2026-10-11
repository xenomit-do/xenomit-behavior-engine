# Plugin format and activation

The desktop controller discovers built-in bundles and JSON files in
`%APPDATA%\XenomitBot\plugins` at startup. Restart the controller after adding
or editing a file. The Plugins page enables bundles independently. Activation
cannot change during a run. Settings for disabled bundles remain persisted.
Each run receives immutable filtered module and behavior registries, so a
disabled plugin contributes neither objective factories nor stopped rules.

| Bundle | Contributions |
|---|---|
| Base | NPC killer, collection, kill/collect; anti-push, safe stop, captcha and booty-key rules |
| Combat | Blacklight farming; defense, healing, crowd avoidance, item/ability selection |
| Galaxy gates | Galaxy, Hades, LoW, GoP, Eternal Blacklight; supported Shared gate contributions |
| Events | Treacherous Domain, chaos spawner; supported Shared event contributions |
| Economy | Palladium, Skylab; calendar and supported resource/account rules |
| Utilities | Follow, travel fixes, profiles, custom events and convenience rules |

The authoritative membership is `PluginCatalog::descriptors()`. A plugin can
be disabled even if another plugin's settings reference it; dependency errors
are reported before Start. No mandatory base bundle is silently re-enabled.
Account services belong to the host API and require a registered adapter. An
enabled bundle does not itself enable its optional behaviors.

## Third-party plugins

Format version 1 supports declarative objectives and conditional general rules.
These are executable engine contributions, not merely catalogue labels. Java
JARs from DarkBot are not compatible. Native Rust extensions can still use the
factory/service registration interfaces described in `CUSTOM_MODULES.md`.

Each file is one self-contained manifest. `id` and local contribution IDs use
lowercase letters, digits, underscores or hyphens (maximum 64 characters).
Contributions are namespaced as `plugin-id:local-id`, preventing collisions
with native modules. Unknown properties, unsupported versions, duplicate IDs,
invalid schemas, cyclic dependencies and malformed implementations are rejected.
File size is limited to 1 MiB, with up to 256 contributions and 1024 sequence
steps. Discovery reports invalid files and keeps valid plugins. A plugin with
unresolved dependencies is rejected after all files have been read. Third-party
bundles default to disabled unless `default_enabled` explicitly says otherwise.

Periodic client refresh waits for each module's interruption contract. Manifest
planner objectives may pause at the shared safe-idle point; manifest sequences
defer for their entire durable run and become ready only when terminal. This
policy is supplied by the engine and needs no manifest field.

The ready-to-copy [example manifest](examples/plugins/example-tools.json) can
be installed in the discovery directory. Example (save as `example-tools.json`):

```json
{
  "format_version": 1,
  "id": "example-tools",
  "name": "Example tools",
  "description": "A timed configuration change and an optional PET rule",
  "default_enabled": false,
  "required_plugins": [],
  "modules": [{
    "descriptor": {
      "id": "configuration",
      "name": "Delayed configuration",
      "description": "Wait, select configuration 2, then finish",
      "settings": [],
      "required_services": []
    },
    "implementation": {
      "type": "sequence",
      "steps": [
        {"type": "delay", "duration_ms": 1000},
        {"type": "command", "command": {"command": "select_configuration", "configuration": 2}, "timeout_ms": 5000}
      ]
    }
  }],
  "behaviors": [{
    "descriptor": {"id": "pet", "name": "Enable PET on map 1", "description": "Requires an observed map", "settings": [], "required_services": []},
    "priority": 100,
    "condition": {"type": "map", "map_id": 1},
    "cooldown_ms": 10000,
    "commands": [{"command": "set_pet", "enabled": true}]
  }]
}
```

Behavior descriptors automatically gain `enabled=false` and
`tick_stopped=false` controls; do not redeclare them. Their settings live under
the namespaced rule ID in global `behaviors`. Rules check enabled/paused state,
snapshot conditions and a monotonic dispatch cooldown. Dispatch cooldowns also
limit failed attempts; they do not imply a successful command or retry a
transaction. Higher priority takeovers pause supplementary plugin rules through
the ordinary behavior runner.

Conditions are `map`, `hero_field` (`key` dotted path and non-null `equals`),
`all` and `any` (nonempty `conditions` arrays). Unknown observations fail closed.
Commands use the serialized `GameCommand` contract in `xenomit-game-api`.

A `planner` module implementation takes a serialized `BehaviorConfig` in
`config`, retains its planner state between ticks, and emits ordinary intents.
It uses world coordinates through the controller's normal executor. Safety and
travel policies should be provided by the planner configuration or separate
enabled rules; JSON planners do not inherit a built-in module's extra state.

Sequence steps are:

- `delay`: positive `duration_ms`, cooperative and cancellable.
- `command`: typed `command` and positive `timeout_ms`; waits for its result.
- `service`: `service`, `input`, optional `requires_game`, and positive
  `timeout_ms`; requires the host to implement that named service.

The engine derives required service names from sequence steps and validates
them before Start. Service input recursively substitutes `{"$setting":"path"}`
with the module's prepared settings. Step completion and errors follow
`TaskSequence`; no automatic retries occur, particularly for purchases/spins.
Settings changes during a declarative module run require stopping first.
Host services cannot be invented or injected by JSON manifests.

## Feature coverage

SharedPlugin coverage is tracked in `SHARED_PLUGIN.md`. The complete upstream
inventory and missing observations/commands are in `API_GAPS.md` and the
machine-readable `api-gaps.json`. Catalogue membership means an implemented
contribution; missing upstream features are recorded as gaps instead of empty
selectable modules.
