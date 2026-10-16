# Writing modules

Start with [the runnable gate/event example](examples/sequenced_event.rs):

```powershell
cargo run --example sequenced_event
```

It registers an event wait, a gate spin that consumes the event result, a
cooldown, and gate entry. Its services are simulations. Implement the service
callbacks against your own Backpage or game adapters to perform real actions.

## Register once, keep state for a whole run

Extend `builtin_registry_builder()` with `register(descriptor, factory)` and
consume it with `build()`. The resulting catalog is immutable. Factories run
only when starting a module; listing descriptors does not instantiate modules.
Each start creates one `ModuleRunner` and one retained `BehaviorModule`.
Settings updates call that same instance's `update_settings`, so they never
reset a sequence. The default implementation rejects changes until stopped;
override it when your state machine can safely apply edits. A runner rejects
module ID changes: stop, select another module, then start a new run.

Descriptors declare settings using booleans, bounded integers/numbers, text,
choices, boolean dictionaries, or JSON. Dotted keys describe nested objects.
Defaults and types are validated before the factory runs; duplicate IDs and
overlapping paths fail registration. Groups and descriptions drive the UI.
`BooleanMap { discover: Some(EntityCategory::Npc) }` adds observed NPC species
as enabled entries without re-enabling entries the user disabled.

In `xenomit-bot-gui`, construct `PlannerRuntime::with_registry(modules.build(),
services.build())` where the default runtime is currently installed in
`src/bot/mod.rs`. Modules are Rust registrations made before engine attachment,
not hot-loaded plugins. No UI match arms or new settings widgets are needed.
Discovery runs off the UI thread during startup and can overlap game loading.
The desktop builds a page and its input entities once per descriptor, then
reuses them when switching modules. Configuration lives under
`modules.<module_id>` in `modules.json`; legacy settings supply migration defaults.

## Sequence operations instead of sleeping in a tick

`TaskSequence` implements `BehaviorModule` and can be returned directly from a
factory. `operation` submits a request once and polls it on subsequent ticks.
Only successful completion advances the sequence. `operation_with` builds a
request from previous named results once and retains it across retries.
Operation result names must be unique. `delay` and `wait_until` cooperate with
the scheduler even for multi-hour waits. `progress()` and `results()` let a
wrapper expose richer state or compose a sequence with another module.

For loops, branching, changing event modes, or recovery, implement
`BehaviorModule::tick(&mut self, &mut TickContext) -> ModuleOutput` with ordinary
Rust fields for your phase and active sequence. Delegate to its `tick`, inspect
`RunState`, then transition phases. Return `Running`, `Completed`, or `Failed`
explicitly. Completed, failed, and cancelled sequences/runners are terminal and
do not restart work on subsequent ticks. A phase transition may create a new
sequence deliberately. A custom combat module can return an existing
`BehaviorDecision` in `ModuleOutput.plan`; service-only modules return no plan
and the bot does not issue idle roaming orders for them.

Modules can implement `BehaviorModule::interruption_status` to declare when a
host-requested interruption is safe. The runner forwards the reason and an
immutable `InterruptionContext` containing the current snapshot and readiness;
terminal modules report `Ready`. The default is `Deferred`, so custom modules
must explicitly opt in. Keep durable workflows deferred until their terminal
state, and defer while combat, collection, travel, recovery, or gate work is
active. Returning `Ready` permits the host to stop ticking this run at that
point; it does not itself cancel operations or change the module state.

For example, a module with no durable state can delegate to the shared idle
check:

```rust
fn interruption_status(
    &self,
    context: &InterruptionContext<'_>,
    _reason: InterruptionReason,
) -> InterruptionStatus {
    xenomit_behavior_engine::interruption::safe_idle_status(context)
}
```

The shared helper is available to custom Rust modules. Third-party manifest
planners use the engine's idle policy, while manifest sequences defer until all
steps complete. A sequence should not opt into interruption merely because its
current step is a delay: later steps may depend on durable earlier results.

`TickContext.now_ms` is monotonic time since start, independent of snapshot
timestamps. `snapshot` may be absent. Nongame operations can run while the
client is connecting; requests marked `requires_game()` wait for readiness
before starting their timeout. `wait_until` conditions receive context and
previous results. Ticks and conditions must return promptly; put blocking I/O
in a service, never sleep or perform network requests inside a module tick.

## Host services, cancellation, and retries

Use `ServiceRegistryBuilder::register(name, |context, input| ...)` for operations
outside the planner. Declare required names with `requires_service` so the bot
fails a start immediately if an adapter is unavailable. The host supplies
`game.call` through its `GameClient`; construct those requests with
`OperationRequest::game_call(FunctionCall)`, preserving in-frame timing when
needed. Service callbacks receive JSON input, cancellation, and an optional
idempotency key, then return JSON or an error. They can use their own async
runtime if needed.

`ThreadedOperationHost` runs callbacks off the scheduler, catches panics,
caches results, and bounds concurrent work. Cancelled jobs still count against
capacity until they actually exit. Stop, timeout, runner completion, and host
drop propagate cooperative cancellation. Callbacks must check the token during
long waits and use finite I/O timeouts; Rust cannot forcibly terminate an
uncooperative callback. `cancel` requests cancellation, `release` discards the
handle; completion should be polled before releasing when its result matters.

Automatic retries require an explicitly idempotent request. Every retry keeps
the same input and idempotency key. A spin or purchase should use the default
single attempt unless its service/server deduplicates that key. A timeout
cannot prove a remote purchase did not succeed: query authoritative state
before deciding to issue another spend. Retrying a timed-out service can overlap
its previous callback if that callback ignores cancellation, so adapter-side
deduplication is essential.

State persists in memory through ticks, waits, setting edits, and missing
snapshots. Stop or process restart starts a new run. Persist a journal and
reconcile remote state in your factory/state machine if your module needs
recovery across application restarts; the engine does not promise durable
exactly-once execution of remote actions.

## Native game commands and movement

`OperationRequest::game_command(GameCommand)` invokes the host's `game.command`
service, waits for bridge readiness, and supports the same cancellation and
completion contract as other operations. Ruffle native commands include
portal jumps, explicit pet activation, equipped pet gear/locator selection,
ship configuration, equipped item selection/activation and offered free base
revival. A successful command means the client submitted the request; wait on
an authoritative snapshot with `wait_until` before treating a portal jump or
other server action as completed. Commands are non-idempotent by default.

`SettingKind::Map` creates a searchable map selector in the desktop settings.
Its value is null (current map) or a map ID; legacy names resolve only against
the actual game catalog. Map choices update as the catalog loads while keeping
the existing control and settings page alive. Registrations are immutable;
choosing another module requires stopping the current run.

For a custom movement plan, use `ObstacleNavigation` to query walkability and
`MovementRouter` for retained routes. `MapTraveler::tick` provides temporary
portal travel with shortest directed routes and bounded retries; keep the
back module in your own state and resume only after `TravelState::Arrived`.
`SafetyManager` can supply the planner context and recovery commands.
The desktop runs movement through a world-coordinate router for every module.
Raw `BehaviorEngine` decisions use its configured coordinate transform;
convert once with `decision.into_world_coordinates(engine.config().coordinates)`
before returning a custom plan. Built-in modules already convert their plans.

The runtime invokes `command_succeeded(command, now_ms)` individually as
commands succeed, even when a later action fails. Use it for item cooldowns
and acknowledged command state. `plan_failed` must not undo successful remote
operations; it can release a pickup wait or prepare a new route. Custom module
ticks, conditions and service callbacks still need explicit finite limits and
cooperative cancellation.
