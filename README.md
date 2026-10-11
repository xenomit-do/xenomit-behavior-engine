# xenomit-behavior-engine

A configurable Rust behavior planner for the sibling `xenomit-game-api` crate.
It also provides an immutable module registry, schema discovery, retained
module instances, cancellable service operations, and completion-aware task
sequences. See [Writing modules](CUSTOM_MODULES.md) and the runnable
[gate/event example](examples/sequenced_event.rs) for custom workflows.
It implements collector, NPC-killer, and kill-and-collect modes. It has no UI,
client lifecycle, bridge connection, or reflected function-name assumptions.

The caller reads a `GameSnapshot`, passes it to `BehaviorEngine::step`, then
translates the returned `ActionIntent`s into calls for its `GameClient`. This
keeps behavior decisions separate from the caller's API bindings and error
handling.

## Modes and configuration

`BehaviorConfig.mode` accepts `collector`, `npc_killer`, or `kill_and_collect`.
Box and NPC rules are explicit allowlists: only entities matching an enabled
rule are selected. Selectors can match category, exact or partial name/class,
path, ID, and raw fields. If several rules match, the lowest numeric priority
wins (including disabled rules, allowing a more specific denial). Lower
priority values are preferred; NPC ties prefer lower health, then shorter
distance. Current NPCs receive Darkbot's health-dependent priority bonus when
locally targeted or within 600 units. The explicit configured bonus is additive.

Each NPC rule sets its desired `combat_radius` and optional `flags`. Combat
predicts movement 250ms ahead, corrects the radius, scores both orbit
directions against obstacles and other NPCs, and searches for a reachable
point. `no_circle` keeps a destination inside the allowed band unless the
radius exceeds 750 units. `position_tolerance` suppresses negligible moves.

Collector selection uses priority/distance and retains its current box until
an alternative saves more than 150 units. Within `pickup_radius` it requests
pickup, adding retry backoff, travel time and 30ms to the base `wait_ms`.
`WaitForCollection` preserves that box and blocks movement until the wait
expires or the box becomes invalid/disappears. `isCollected` can indicate a
recent attempt and does not cancel an active wait.

In combined mode, a box inside `collect_radius` controls movement while
attacking continues. Target health below 25% suppresses new detours. The
box-to-NPC limit only applies with `ignore_boxes`, capped at twice the NPC
radius. Collection and circling never request competing movement orders.

See [example-config.json](example-config.json). Replace its sample name
fragments with names or class names observed in snapshots. Native snapshots
report world coordinates. Configure `coordinates` when planning in another
coordinate system; combat radii and distances use those transformed units.
Before a world-coordinate executor, convert a raw planner decision with
`into_world_coordinates(config.coordinates)`. Built-in modules do this already.

## Use

```rust
use xenomit_behavior_engine::{ActionIntent, BehaviorEngine};
use xenomit_game_api::{CallTiming, FunctionCall, GameClient};
use serde_json::json;

fn run_step(
    engine: &mut BehaviorEngine,
    client: &dyn GameClient,
) -> Result<(), Box<dyn std::error::Error>> {
    let snapshot = client.snapshot(None)?;
    let decision = engine.step(&snapshot, snapshot.observed_at_unix_ms);

    for action in decision.actions {
        let call = match action {
            ActionIntent::MoveTo { destination } => FunctionCall {
                target: "<configured movement target>".into(),
                function: "<configured movement function>".into(),
                arguments: vec![json!(destination.x), json!(destination.y)],
                timing: CallTiming::InFrame,
            },
            ActionIntent::AttackNpc { target } => {
                let _target = target; // Map its ID or path using your API binding.
                continue;
            }
            ActionIntent::CollectBox { target, wait_ms } => {
                let _ = (target, wait_ms); // Map to your configured collection call.
                continue;
            }
            ActionIntent::WaitForCollection { .. } => continue,
        };
        client.call_function(&call)?;
    }
    Ok(())
}
```

Load JSON with `BehaviorConfig::from_json` and construct the planner with
`BehaviorEngine::new`, or use `BehaviorEngine::from_json`. The caller chooses
the actual reflected target, method, argument order, and retry policy after
inspecting the running client.

## Navigation and manager integration

`step` reads manager facts from snapshot fields and uses `ObstacleNavigation`.
It checks circles, rectangles, polygons and inverted no-access zones.
`MovementRouter` retains collision-checked A* waypoints and replans on map,
geometry, destination or progress changes. Missing/stale native geometry
pauses movement. Legacy snapshots without navigation retain bounds-only
behavior. `step_with_context` accepts custom observations and navigation.

The built-in registry provides collector, NPC killer, combined and palladium
modules with settings schemas, retained `SafetyManager` and `MapTraveler`.
Working-map selection uses a directed shortest portal route, level
restrictions, loading delays and bounded jump retries, then resumes the same
module. Safety includes enemy retreat, repair hysteresis, repair items,
offered free base revival and death limits. Item/configuration/pet policies
emit typed `GameCommand`s through `controls.commands`; the Ruffle native client
implements them. Honor `suspended` before any roaming fallback, report each
successful command with `command_succeeded`, and report failed plans so pickup
waits can roll back. Custom executors still own action bindings and lifecycle.

The registry also includes a Cubikon hunter that selects the faction X-6 map,
patrols four scan points, and prioritizes Cubikon targets. `npc_preset` loads
map-scoped NPC radii and priorities; `box_preset` selects the built-in ore,
bonus, booty, or event box groups. `item_restock` runs independently as an
objective and requires the host service `backpage.shop_purchase`. It buys only
configured rows, and skips purchases when inventory is unknown unless
`buy_when_unobserved` is enabled.

See [DARKBOT_PARITY.md](DARKBOT_PARITY.md) for source provenance, the full
observation contract, integration requirements and deliberate differences.
Paths are best-effort identities; use IDs when supplied by the bridge.
## Plugin bundles

`PluginCatalog` groups native modules and rules into swappable bundles and loads
third-party declarative contributions. See [PLUGINS.md](PLUGINS.md) for activation
and the versioned manifest format, [SHARED_PLUGIN.md](SHARED_PLUGIN.md) for the
SharedPlugin port, and [API_GAPS.md](API_GAPS.md) for the maintained API backlog.
