//! Runnable module integration using simulated services, without account access.
//! Replace the three service callbacks with your Backpage/game adapters.
use serde_json::json;
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use xenomit_behavior_engine::*;
use xenomit_game_api::ModuleConfig;

fn main() -> Result<(), String> {
    let descriptor = ModuleDescriptor::new("gate_event", "Galaxy gate event")
        .description("Wait for an event, spin a gate, then enter after cooldown.")
        .setting(
            SettingDescriptor::new(
                "gate.spins",
                "Spin count",
                SettingKind::Integer { min: 1, max: 1000 },
                json!(100),
            )
            .group("Galaxy gate"),
        )
        .setting(
            SettingDescriptor::new(
                "event.name",
                "Event",
                SettingKind::Text,
                json!("anniversary"),
            )
            .group("Event"),
        )
        .setting(
            SettingDescriptor::new(
                "event.cooldown_seconds",
                "Cooldown (seconds)",
                SettingKind::Integer {
                    min: 0,
                    max: 172_800,
                },
                json!(0),
            )
            .group("Event"),
        )
        .requires_service("event.wait")
        .requires_service("galaxy_gate.spin")
        .requires_service("galaxy_gate.enter");

    // Registration is the only place that knows this module exists. No core
    // enum, scheduler match, or desktop settings widget needs modification.
    let mut modules = builtin_registry_builder();
    modules.register(descriptor, |settings| {
        let event = settings["event"]["name"].clone();
        let spins = settings["gate"]["spins"].clone();
        let cooldown = settings["event"]["cooldown_seconds"].as_u64().unwrap();
        let sequence = TaskSequence::new()
            .operation(
                "await_event",
                OperationRequest::service("event.wait", json!({"name": event})).idempotent(),
                Duration::from_secs(24 * 3600),
                RetryPolicy {
                    max_attempts: 3,
                    backoff: Duration::from_secs(5),
                },
            )?
            .operation_with(
                "spin",
                Duration::from_secs(30),
                RetryPolicy::default(),
                move |results| {
                    // A spend is issued once, and no retry is allowed by default.
                    // Only mark it idempotent if the adapter/server deduplicates keys.
                    Ok(OperationRequest::service(
                        "galaxy_gate.spin",
                        json!({"gate": results["await_event"]["gate"], "count": spins}),
                    ))
                },
            )?
            .delay("cooldown", Duration::from_secs(cooldown))
            .operation_with(
                "enter",
                Duration::from_secs(30),
                RetryPolicy::default(),
                |results| {
                    if results["spin"]["complete"] != json!(true) {
                        return Err("Gate is not complete; choose another spin strategy".into());
                    }
                    Ok(OperationRequest::service(
                        "galaxy_gate.enter",
                        json!({"gate": results["await_event"]["gate"]}),
                    )
                    .requires_game())
                },
            )?;
        Ok(Box::new(sequence))
    })?;
    let registry = modules.build(); // Immutable catalog for this host's lifetime.

    let spins = Arc::new(AtomicUsize::new(0));
    let spent = spins.clone();
    let mut services = ServiceRegistryBuilder::new();
    services.register("event.wait", |context, _| {
        // A real adapter may wait here, checking cancellation and HTTP timeouts.
        if context.cancellation.is_cancelled() {
            return Err("Cancelled".into());
        }
        Ok(json!({"gate": "alpha"}))
    })?;
    services.register("galaxy_gate.spin", move |_, input| {
        spent.fetch_add(input["count"].as_u64().unwrap() as usize, Ordering::Relaxed);
        Ok(json!({"complete": true}))
    })?;
    services.register("galaxy_gate.enter", |_, _| Ok(json!({"entered": true})))?;
    let cancellation = CancellationToken::default();
    let mut operations = ThreadedOperationHost::new(services.build(), cancellation.clone());
    let mut runner = registry.create(
        ModuleConfig {
            module_id: "gate_event".into(),
            settings: json!({}),
        },
        cancellation,
    )?;
    let started = Instant::now();
    loop {
        // Real hosts pass their latest snapshot and readiness. This example
        // simulates a ready game; event/spin services can run before readiness.
        let output = runner.tick(
            started.elapsed().as_millis() as u64,
            None,
            true,
            &mut operations,
        );
        match output.state {
            RunState::Completed => {
                println!(
                    "{} ({} spins)",
                    output.status,
                    spins.load(Ordering::Relaxed)
                );
                return Ok(());
            }
            RunState::Failed(error) => return Err(error),
            RunState::Cancelled => return Err("Cancelled".into()),
            RunState::Running => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
