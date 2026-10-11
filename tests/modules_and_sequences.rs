use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use xenomit_behavior_engine::*;
use xenomit_game_api::{ModuleConfig, set_setting_value, setting_value};

#[derive(Default)]
struct FakeHost {
    requests: Vec<OperationRequest>,
    polls: BTreeMap<OperationHandle, OperationPoll>,
    cancelled: Vec<OperationHandle>,
    released: Vec<OperationHandle>,
}
impl OperationHost for FakeHost {
    fn start(&mut self, request: OperationRequest) -> Result<OperationHandle, String> {
        self.requests.push(request);
        let handle = OperationHandle(self.requests.len() as u64);
        self.polls.insert(handle, OperationPoll::Pending);
        Ok(handle)
    }
    fn poll(&mut self, handle: OperationHandle) -> OperationPoll {
        self.polls[&handle].clone()
    }
    fn cancel(&mut self, handle: OperationHandle) {
        self.cancelled.push(handle);
    }
    fn release(&mut self, handle: OperationHandle) {
        self.released.push(handle);
    }
}
fn tick(
    sequence: &mut TaskSequence,
    host: &mut FakeHost,
    token: &CancellationToken,
    now_ms: u64,
    game_ready: bool,
) -> ModuleOutput {
    sequence.tick(&mut TickContext {
        now_ms,
        snapshot: None,
        game_ready,
        cancellation: token,
        operations: host,
    })
}
fn request() -> OperationRequest {
    OperationRequest::service("spin", json!({"count": 100}))
}
fn sequence(request: OperationRequest, retry: RetryPolicy) -> TaskSequence {
    TaskSequence::new()
        .operation("spin", request, Duration::from_secs(30), retry)
        .unwrap()
}

#[test]
fn pending_operation_is_issued_once_and_next_step_uses_its_result() {
    let mut sequence = sequence(request(), RetryPolicy::default())
        .operation_with(
            "event",
            Duration::from_secs(20),
            RetryPolicy::default(),
            |results| {
                Ok(OperationRequest::service(
                    "event",
                    json!({"gate": results["spin"]["gate"]}),
                ))
            },
        )
        .unwrap();
    let token = CancellationToken::default();
    let mut host = FakeHost::default();
    for time in [0, 1, 20, 29_999] {
        assert_eq!(
            tick(&mut sequence, &mut host, &token, time, false).state,
            RunState::Running
        );
    }
    assert_eq!(host.requests.len(), 1);
    host.polls.insert(
        OperationHandle(1),
        OperationPoll::Succeeded(json!({"gate": "alpha"})),
    );
    tick(&mut sequence, &mut host, &token, 30_000, false);
    assert_eq!(sequence.progress().completed_steps, 1);
    tick(&mut sequence, &mut host, &token, 30_001, false);
    assert_eq!(host.requests[1].input, json!({"gate": "alpha"}));
    host.polls
        .insert(OperationHandle(2), OperationPoll::Succeeded(json!(true)));
    assert_eq!(
        tick(&mut sequence, &mut host, &token, 30_002, false).state,
        RunState::Completed
    );
    tick(&mut sequence, &mut host, &token, 40_000, true);
    assert_eq!(host.requests.len(), 2);
    assert_eq!(host.released, vec![OperationHandle(1), OperationHandle(2)]);
}

#[test]
fn multi_hour_delay_and_condition_keep_progress_without_a_snapshot() {
    let mut sequence = TaskSequence::new()
        .delay("wait for event", Duration::from_secs(9 * 3600))
        .wait_until("game login", Duration::from_secs(60), |context, _| {
            context.game_ready
        });
    let token = CancellationToken::default();
    let mut host = FakeHost::default();
    tick(&mut sequence, &mut host, &token, 10, false);
    tick(&mut sequence, &mut host, &token, 9 * 3600 * 1000 + 9, false);
    assert_eq!(sequence.progress().completed_steps, 0);
    tick(
        &mut sequence,
        &mut host,
        &token,
        9 * 3600 * 1000 + 10,
        false,
    );
    assert_eq!(sequence.progress().completed_steps, 1);
    assert_eq!(
        tick(&mut sequence, &mut host, &token, 9 * 3600 * 1000 + 11, true).state,
        RunState::Completed
    );
    assert!(host.requests.is_empty());
}

#[test]
fn game_operations_wait_for_readiness_before_starting_the_timeout() {
    let mut sequence = sequence(request().requires_game(), RetryPolicy::default());
    let token = CancellationToken::default();
    let mut host = FakeHost::default();
    tick(&mut sequence, &mut host, &token, 0, false);
    tick(&mut sequence, &mut host, &token, 10_000_000, false);
    assert!(host.requests.is_empty());
    tick(&mut sequence, &mut host, &token, 10_000_001, true);
    assert_eq!(host.requests.len(), 1);
    assert_eq!(
        tick(&mut sequence, &mut host, &token, 10_000_002, false).state,
        RunState::Running
    );
}

#[test]
fn retries_back_off_and_reuse_identical_input_and_idempotency_key() {
    let calls = Arc::new(AtomicUsize::new(0));
    let built = calls.clone();
    let mut sequence = TaskSequence::new()
        .operation_with(
            "spin",
            Duration::from_secs(1),
            RetryPolicy {
                max_attempts: 2,
                backoff: Duration::from_millis(500),
            },
            move |_| {
                built.fetch_add(1, Ordering::Relaxed);
                Ok(request().idempotent())
            },
        )
        .unwrap();
    let token = CancellationToken::default();
    let mut host = FakeHost::default();
    tick(&mut sequence, &mut host, &token, 0, false);
    tick(&mut sequence, &mut host, &token, 1000, false);
    assert_eq!(host.cancelled, vec![OperationHandle(1)]);
    tick(&mut sequence, &mut host, &token, 1499, false);
    assert_eq!(host.requests.len(), 1);
    tick(&mut sequence, &mut host, &token, 1500, false);
    assert_eq!(host.requests.len(), 2);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert!(host.requests[0].idempotency_key.is_some());
    assert_eq!(
        host.requests[0].idempotency_key,
        host.requests[1].idempotency_key
    );
    assert_eq!(host.requests[0].input, host.requests[1].input);
    host.polls
        .insert(OperationHandle(2), OperationPoll::Succeeded(json!("done")));
    assert_eq!(
        tick(&mut sequence, &mut host, &token, 1501, false).state,
        RunState::Completed
    );
}

#[test]
fn non_idempotent_spins_cannot_retry_including_dynamic_requests() {
    let retry = RetryPolicy {
        max_attempts: 2,
        backoff: Duration::ZERO,
    };
    assert!(
        TaskSequence::new()
            .operation("spin", request(), Duration::from_secs(1), retry)
            .is_err()
    );
    let mut sequence = TaskSequence::new()
        .operation_with("spin", Duration::from_secs(1), retry, |_| Ok(request()))
        .unwrap();
    let mut host = FakeHost::default();
    assert!(matches!(
        tick(
            &mut sequence,
            &mut host,
            &CancellationToken::default(),
            0,
            false
        )
        .state,
        RunState::Failed(_)
    ));
    assert!(host.requests.is_empty());
}

#[test]
fn failure_is_terminal_and_never_reissues_work() {
    let mut sequence = sequence(request(), RetryPolicy::default());
    let mut host = FakeHost::default();
    let token = CancellationToken::default();
    tick(&mut sequence, &mut host, &token, 0, true);
    host.polls.insert(
        OperationHandle(1),
        OperationPoll::Failed("insufficient energy".into()),
    );
    let failed = tick(&mut sequence, &mut host, &token, 1, true);
    assert!(matches!(failed.state, RunState::Failed(_)));
    assert_eq!(
        tick(&mut sequence, &mut host, &token, 999_999, true).state,
        failed.state
    );
    assert_eq!(host.requests.len(), 1);
    assert_eq!(sequence.progress().completed_steps, 0);
}

#[test]
fn condition_timeout_is_terminal_and_cancellation_releases_inflight_work() {
    let mut wait = TaskSequence::new().wait_until("event", Duration::from_secs(10), |_, _| false);
    let mut host = FakeHost::default();
    let token = CancellationToken::default();
    tick(&mut wait, &mut host, &token, 0, false);
    assert!(matches!(
        tick(&mut wait, &mut host, &token, 10_000, false).state,
        RunState::Failed(_)
    ));
    let mut spin = sequence(request(), RetryPolicy::default());
    tick(&mut spin, &mut host, &token, 0, false);
    token.cancel();
    assert_eq!(
        tick(&mut spin, &mut host, &token, 1, false).state,
        RunState::Cancelled
    );
    assert_eq!(host.cancelled, vec![OperationHandle(1)]);
    assert_eq!(host.released, vec![OperationHandle(1)]);
}

#[test]
fn invalid_sequence_definitions_and_duplicate_result_names_are_rejected() {
    assert!(
        TaskSequence::new()
            .operation("spin", request(), Duration::ZERO, RetryPolicy::default())
            .is_err()
    );
    assert!(
        TaskSequence::new()
            .operation(
                "spin",
                request(),
                Duration::from_secs(1),
                RetryPolicy {
                    max_attempts: 0,
                    backoff: Duration::ZERO
                }
            )
            .is_err()
    );
    assert!(
        sequence(request(), RetryPolicy::default())
            .operation(
                "spin",
                request(),
                Duration::from_secs(1),
                RetryPolicy::default()
            )
            .is_err()
    );
}

fn descriptor() -> ModuleDescriptor {
    ModuleDescriptor {
        id: "event".into(),
        name: "Event mode".into(),
        description: "Custom event workflow".into(),
        required_services: vec!["spin".into()],
        settings: vec![SettingDescriptor::new(
            "gate.count",
            "Spin count",
            SettingKind::Integer { min: 1, max: 100 },
            json!(10),
        )],
    }
}
struct Counter {
    ticks: usize,
}
impl BehaviorModule for Counter {
    fn tick(&mut self, _: &mut TickContext<'_>) -> ModuleOutput {
        self.ticks += 1;
        ModuleOutput::running(self.ticks.to_string())
    }
    fn update_settings(&mut self, _: &Value) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn discovery_never_constructs_modules_and_updates_retain_instance_state() {
    let constructions = Arc::new(AtomicUsize::new(0));
    let count = constructions.clone();
    let mut builder = ModuleRegistryBuilder::new();
    builder
        .register(descriptor(), move |settings| {
            assert!(setting_value(settings, "gate.count").is_some());
            count.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(Counter { ticks: 0 }))
        })
        .unwrap();
    let registry = builder.build();
    let metadata = registry.descriptors();
    assert_eq!(registry.descriptors(), metadata);
    assert_eq!(constructions.load(Ordering::Relaxed), 0);
    let token = CancellationToken::default();
    let mut runner = registry
        .create(
            ModuleConfig {
                module_id: "event".into(),
                settings: Value::Null,
            },
            token,
        )
        .unwrap();
    let mut host = FakeHost::default();
    assert_eq!(runner.tick(0, None, false, &mut host).status, "1");
    runner
        .update(&ModuleConfig {
            module_id: "event".into(),
            settings: json!({"gate":{"count": 20}}),
        })
        .unwrap();
    assert_eq!(runner.tick(1, None, false, &mut host).status, "2");
    assert_eq!(constructions.load(Ordering::Relaxed), 1);
    assert!(
        runner
            .update(&ModuleConfig {
                module_id: "other".into(),
                settings: json!({})
            })
            .is_err()
    );
    assert!(
        runner
            .update(&ModuleConfig {
                module_id: "event".into(),
                settings: json!({"gate":{"count": -1}})
            })
            .is_err()
    );
    assert_eq!(runner.tick(2, None, false, &mut host).status, "3");
}

#[test]
fn registry_rejects_duplicate_ids_overlapping_paths_and_invalid_defaults() {
    let mut builder = ModuleRegistryBuilder::new();
    builder
        .register(descriptor(), |_| Ok(Box::new(TaskSequence::new())))
        .unwrap();
    assert!(
        builder
            .register(descriptor(), |_| Ok(Box::new(TaskSequence::new())))
            .is_err()
    );
    let mut overlap = descriptor();
    overlap.id = "overlap".into();
    overlap.settings.push(SettingDescriptor::new(
        "gate",
        "Gate",
        SettingKind::Json,
        json!({}),
    ));
    assert!(
        builder
            .register(overlap, |_| Ok(Box::new(TaskSequence::new())))
            .is_err()
    );
    let mut invalid = descriptor();
    invalid.id = "invalid".into();
    invalid.settings[0].default = json!(101);
    assert!(
        builder
            .register(invalid, |_| Ok(Box::new(TaskSequence::new())))
            .is_err()
    );
}

#[test]
fn completed_runner_is_terminal_and_cancels_its_operation_tree() {
    let mut builder = ModuleRegistryBuilder::new();
    builder
        .register(descriptor(), |_| Ok(Box::new(TaskSequence::new())))
        .unwrap();
    let token = CancellationToken::default();
    let child = token.child();
    let mut runner = builder
        .build()
        .create(
            ModuleConfig {
                module_id: "event".into(),
                settings: json!({}),
            },
            token,
        )
        .unwrap();
    let mut host = FakeHost::default();
    assert_eq!(
        runner.tick(0, None, false, &mut host).state,
        RunState::Completed
    );
    assert!(child.is_cancelled());
    assert_eq!(
        runner.tick(1, None, false, &mut host).state,
        RunState::Completed
    );
}

#[test]
fn schema_defaults_preserve_unknown_keys_and_dotted_paths() {
    let descriptor = descriptor();
    let mut settings = descriptor.prepare_settings(json!({"private": 7})).unwrap();
    assert_eq!(settings, json!({"private":7,"gate":{"count":10}}));
    set_setting_value(&mut settings, "gate.count", json!(100)).unwrap();
    descriptor.prepare_settings(settings.clone()).unwrap();
    let before = settings.clone();
    assert!(set_setting_value(&mut settings, "new..broken", json!(1)).is_err());
    assert_eq!(settings, before);
    assert!(
        descriptor
            .prepare_settings(json!({"gate": "wrong"}))
            .is_err()
    );
    assert!(descriptor.prepare_settings(json!([])).is_err());
}

#[test]
fn builtins_are_real_modules_and_all_defaults_construct() {
    let registry = builtin_registry();
    for descriptor in registry.descriptors() {
        assert_ne!(descriptor.id, "galaxy_gates");
        if descriptor.id == "palladium" {
            assert!(
                !descriptor
                    .settings
                    .iter()
                    .any(|s| s.key == "features.collect_boxes")
            );
        }
        registry
            .create(
                ModuleConfig {
                    module_id: descriptor.id,
                    settings: json!({}),
                },
                CancellationToken::default(),
            )
            .unwrap();
    }
}

fn await_result(host: &mut ThreadedOperationHost, handle: OperationHandle) -> OperationPoll {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let result = host.poll(handle);
        if result != OperationPoll::Pending {
            return result;
        }
        assert!(Instant::now() < deadline, "service did not finish");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn threaded_services_forward_inputs_keys_errors_and_catch_panics() {
    let mut services = ServiceRegistryBuilder::new();
    services
        .register("echo", |context, input| {
            Ok(json!({"input": input, "key": context.idempotency_key}))
        })
        .unwrap();
    services
        .register("fail", |_, _| Err("out of energy".into()))
        .unwrap();
    services
        .register("panic", |_, _| panic!("service failure"))
        .unwrap();
    let mut host = ThreadedOperationHost::new(services.build(), CancellationToken::default());
    assert!(host.start(request()).is_err());
    let mut request = OperationRequest::service("echo", json!(42));
    request.idempotency_key = Some("one spin".into());
    let handle = host.start(request).unwrap();
    let result = await_result(&mut host, handle);
    assert_eq!(
        result,
        OperationPoll::Succeeded(json!({"input":42,"key":"one spin"}))
    );
    assert_eq!(host.poll(handle), result);
    host.release(handle);
    assert!(matches!(host.poll(handle), OperationPoll::Failed(_)));
    let handle = host
        .start(OperationRequest::service("fail", Value::Null))
        .unwrap();
    assert_eq!(
        await_result(&mut host, handle),
        OperationPoll::Failed("out of energy".into())
    );
    let handle = host
        .start(OperationRequest::service("panic", Value::Null))
        .unwrap();
    assert_eq!(
        await_result(&mut host, handle),
        OperationPoll::Failed("service panicked".into())
    );
}

#[test]
fn root_cancellation_reaches_running_service_without_blocking_polling() {
    let (started, ready) = std::sync::mpsc::channel();
    let token = CancellationToken::default();
    let mut services = ServiceRegistryBuilder::new();
    services
        .register("long", move |context, _| {
            started.send(()).unwrap();
            while !context.cancellation.is_cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(json!("cancelled"))
        })
        .unwrap();
    let mut host = ThreadedOperationHost::new(services.build(), token.clone());
    let handle = host
        .start(OperationRequest::service("long", Value::Null))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(3)).unwrap();
    assert_eq!(host.poll(handle), OperationPoll::Pending);
    token.cancel();
    assert_eq!(
        await_result(&mut host, handle),
        OperationPoll::Succeeded(json!("cancelled"))
    );
    assert!(
        host.start(OperationRequest::service("long", Value::Null))
            .is_err()
    );
}

#[test]
fn cancelled_uncooperative_jobs_still_count_against_capacity() {
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let worker_gate = gate.clone();
    let mut services = ServiceRegistryBuilder::new();
    services
        .register("blocked", move |_, _| {
            let (lock, wake) = &*worker_gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = wake.wait(released).unwrap();
            }
            Ok(Value::Null)
        })
        .unwrap();
    let mut host = ThreadedOperationHost::new(services.build(), CancellationToken::default());
    for _ in 0..32 {
        let handle = host
            .start(OperationRequest::service("blocked", Value::Null))
            .unwrap();
        host.cancel(handle);
        host.release(handle);
    }
    let result = host.start(OperationRequest::service("blocked", Value::Null));
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    assert!(result.unwrap_err().contains("capacity"));
}
