use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use xenomit_behavior_engine::{
    CancellationToken, PluginCatalog, PluginManifest, PluginSelection, RuleContext, ServiceContext,
    ServiceRegistryBuilder, ThreadedOperationHost,
};
use xenomit_game_api::{
    EntityCategory, GameEntity, GameSnapshot, ModuleConfig, NavigationState, Point, StageInfo,
};

fn snapshot(map: Option<i32>, hero_fields: Value) -> GameSnapshot {
    GameSnapshot {
        navigation: NavigationState {
            map_id: map,
            ..Default::default()
        },
        observed_at_unix_ms: 1,
        stage: StageInfo::default(),
        map: None,
        entities: vec![GameEntity {
            path: "hero".into(),
            class_name: "Hero".into(),
            name: "hero".into(),
            category: EntityCategory::Player,
            position: Point::default(),
            bounds: Default::default(),
            id: Some(json!(1)),
            health: None,
            max_health: None,
            shield: None,
            max_shield: None,
            fields: hero_fields.as_object().unwrap().clone(),
        }],
        visited: 1,
        truncated: false,
    }
}

fn manifest(id: &str, modules: Value, behaviors: Value, deps: Value) -> PluginManifest {
    serde_json::from_value(json!({
        "format_version": 1, "id": id, "name": id,
        "required_plugins": deps, "modules": modules, "behaviors": behaviors
    }))
    .unwrap()
}

#[test]
fn builtin_groups_assign_ownership_and_disable_gate_bundle_without_affecting_base() {
    let catalog = PluginCatalog::builtin();
    let descriptors = catalog.descriptors();
    let owner = |id: &str| {
        descriptors
            .iter()
            .find(|p| {
                p.module_ids.iter().any(|m| m == id) || p.behavior_ids.iter().any(|m| m == id)
            })
            .map(|p| p.id.as_str())
    };
    assert_eq!(owner("collect"), Some("base"));
    assert_eq!(owner("best_ammo"), Some("combat"));
    assert_eq!(owner("galaxy"), Some("gg"));
    assert_eq!(owner("shared_simple_healing"), Some("combat"));
    let selection = PluginSelection::from([("gg".to_owned(), false)]);
    let modules = catalog.module_descriptors(&selection).unwrap();
    let ids: Vec<_> = modules.iter().map(|d| d.id.as_str()).collect();
    assert!(ids.contains(&"collect"));
    assert!(!ids.contains(&"galaxy"));
    let rules = catalog.behavior_settings_descriptor(&selection).unwrap();
    let keys: Vec<_> = rules.settings.iter().map(|s| s.key.as_str()).collect();
    assert!(keys.contains(&"anti_push.enabled"));
    assert!(keys.iter().any(|k| k.starts_with("shared_simple_healing.")));
}

#[test]
fn bundled_example_loads_as_a_disabled_namespaced_plugin() {
    let example: PluginManifest =
        serde_json::from_str(include_str!("../examples/plugins/example-tools.json")).unwrap();
    let mut catalog = PluginCatalog::builtin();
    catalog.register_manifest(example).unwrap();
    assert!(
        !catalog
            .module_enabled("example-tools:configuration", &PluginSelection::new())
            .unwrap()
    );
    let selection = PluginSelection::from([("example-tools".into(), true)]);
    assert!(
        catalog
            .module_enabled("example-tools:configuration", &selection)
            .unwrap()
    );
    assert!(
        catalog
            .behavior_settings_descriptor(&selection)
            .unwrap()
            .settings
            .iter()
            .any(|s| s.key == "example-tools:pet.enabled")
    );
}

#[test]
fn disabled_plugin_filters_its_modules_and_rules_and_rejects_disabled_dependency() {
    let mut catalog = PluginCatalog::builtin();
    catalog
        .register_manifest(manifest("dependent", json!([]), json!([]), json!(["gg"])))
        .unwrap();
    let selection =
        PluginSelection::from([("gg".to_owned(), false), ("dependent".to_owned(), true)]);
    let err = catalog.module_descriptors(&selection).unwrap_err();
    assert!(err.contains("requires enabled plugin gg"));
    let selection = PluginSelection::from([("gg".to_owned(), false)]);
    let (modules, _) = catalog.registries(&selection).unwrap();
    assert!(!modules.descriptors().iter().any(|d| d.id == "galaxy"));
    let selection = PluginSelection::from([("gg".to_owned(), false), ("combat".to_owned(), false)]);
    let (_, rules) = catalog.registries(&selection).unwrap();
    let rule_ids: Vec<_> = rules.descriptors().into_iter().map(|d| d.id).collect();
    assert!(rule_ids.iter().any(|id| id == "anti_push"));
    assert!(!rule_ids.iter().any(|id| id == "best_ammo"));
    assert!(!rule_ids.iter().any(|id| id == "shared_simple_healing"));
}

#[test]
fn manifest_registration_is_atomic_for_invalid_duplicate_and_dependency_cycles() {
    let mut catalog = PluginCatalog::builtin();
    let before = catalog.descriptors().len();
    let invalid_version: PluginManifest =
        serde_json::from_value(json!({"format_version":9,"id":"bad","name":"bad"})).unwrap();
    assert!(
        catalog
            .register_manifest(invalid_version)
            .unwrap_err()
            .contains("Unsupported plugin format")
    );
    assert_eq!(catalog.descriptors().len(), before);
    let invalid_name: PluginManifest =
        serde_json::from_value(json!({"format_version":1,"id":"bad","name":"  "})).unwrap();
    assert!(
        catalog
            .register_manifest(invalid_name)
            .unwrap_err()
            .contains("Plugin name is empty")
    );
    let dup = manifest("base", json!([]), json!([]), json!([]));
    assert!(
        catalog
            .register_manifest(dup)
            .unwrap_err()
            .contains("Duplicate plugin")
    );
    assert_eq!(catalog.descriptors().len(), before);
    let behavior = json!({
        "descriptor":{"id":"same","name":"Same","description":"duplicate contribution","settings":[]},
        "priority":1,"condition":{"type":"map","map_id":1},"cooldown_ms":100,
        "commands":[{"command":"set_pet","enabled":false}]
    });
    let duplicate_local = manifest(
        "duplicate_local",
        json!([]),
        json!([behavior.clone(), behavior]),
        json!([]),
    );
    assert!(
        catalog
            .register_manifest(duplicate_local)
            .unwrap_err()
            .contains("Invalid/duplicate behavior")
    );
    assert_eq!(catalog.descriptors().len(), before);
    catalog
        .register_manifest(manifest(
            "cycle_a",
            json!([]),
            json!([]),
            json!(["cycle_b"]),
        ))
        .unwrap();
    assert_eq!(catalog.descriptors().len(), before + 1);
    catalog
        .register_manifest(manifest(
            "cycle_b",
            json!([]),
            json!([]),
            json!(["cycle_a"]),
        ))
        .unwrap_err();
    assert_eq!(catalog.descriptors().len(), before + 1);
}

fn temp_dir(label: &str) -> PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!("xenomit-{label}-{n}"));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn directory_loading_reports_bad_files_but_resolves_forward_dependencies() {
    let dir = temp_dir("plugins");
    fs::write(
        dir.join("00-dependent.json"),
        r#"{"format_version":1,"id":"child","name":"child","required_plugins":["parent"]}"#,
    )
    .unwrap();
    fs::write(
        dir.join("10-parent.json"),
        r#"{"format_version":1,"id":"parent","name":"parent"}"#,
    )
    .unwrap();
    fs::write(
        dir.join("20-invalid.json"),
        r#"{"format_version":1,"id":"Bad ID","name":"broken"}"#,
    )
    .unwrap();
    fs::write(dir.join("30-not-json.txt"), "ignored").unwrap();
    let mut catalog = PluginCatalog::builtin();
    let errors = catalog.load_directory(&dir);
    assert_eq!(errors.len(), 1, "{errors:?}");
    let ids: Vec<_> = catalog.descriptors().iter().map(|d| d.id.clone()).collect();
    assert!(ids.contains(&"child".into()) && ids.contains(&"parent".into()));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn declarative_sequence_submits_service_once_waits_for_result_then_completes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(AtomicUsize::new(0));
    let calls_service = calls.clone();
    let mut services = ServiceRegistryBuilder::new();
    services
        .register("test.submit", move |_ctx: ServiceContext, input| {
            calls_service.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"accepted": input["value"]}))
        })
        .unwrap();
    let services = services.build();
    let manifest = manifest(
        "seq",
        json!([{
            "descriptor":{"id":"work","name":"Work","description":"Sequence integration test","settings":[{"key":"value","label":"Value","description":"","group":"General","kind":{"type":"integer","min":1,"max":9},"default":3}]},
            "implementation":{"type":"sequence","steps":[{"type":"service","service":"test.submit","input":{"value":{"$setting":"value"}},"timeout_ms":5000}]}
        }]),
        json!([]),
        json!([]),
    );
    let mut catalog = PluginCatalog::builtin();
    catalog.register_manifest(manifest).unwrap();
    let selection = PluginSelection::from([("seq".to_owned(), true)]);
    let (registry, _) = catalog.registries(&selection).unwrap();
    let mut runner = registry
        .create(
            ModuleConfig {
                module_id: "seq:work".into(),
                settings: json!({"value":7}),
            },
            CancellationToken::default(),
        )
        .unwrap();
    let cancel = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(services, cancel.clone());
    let mut tick = |now| {
        let out = runner.tick(now, None, false, &mut host);
        if matches!(
            out.state,
            xenomit_behavior_engine::modules::RunState::Completed
        ) {
            result.fetch_add(1, Ordering::SeqCst);
        }
        out
    };
    let first = tick(1);
    assert!(matches!(
        first.state,
        xenomit_behavior_engine::modules::RunState::Running
    ));
    for n in 2..100 {
        let out = tick(n);
        if matches!(
            out.state,
            xenomit_behavior_engine::modules::RunState::Completed
        ) {
            break;
        }
        thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(result.load(Ordering::SeqCst), 1);
}

#[test]
fn manifest_rule_obeys_enabled_stopped_condition_and_cooldown() {
    let behavior = json!([{
        "descriptor":{"id":"watch","name":"Watch","description":"Rule integration test","settings":[]}, "priority":1,
        "condition":{"type":"hero_field","key":"ready","equals":true}, "cooldown_ms":100,
        "commands":[{"command":"use_item","item_id":"test_item","activate":true}]
    }]);
    let mut catalog = PluginCatalog::builtin();
    catalog
        .register_manifest(manifest("rules", json!([]), behavior, json!([])))
        .unwrap();
    let selection = PluginSelection::from([
        ("base".to_owned(), false),
        ("combat".to_owned(), false),
        ("gg".to_owned(), false),
        ("event".to_owned(), false),
        ("economy".to_owned(), false),
        ("utility".to_owned(), false),
        ("rules".to_owned(), true),
    ]);
    let (_, registry) = catalog.registries(&selection).unwrap();
    let mut runner = registry
        .create(&json!({"rules:watch":{"enabled":true,"tick_stopped":false}}))
        .unwrap();
    let ready = snapshot(Some(1), json!({"isHero":true,"ready":true}));
    let no_ready = snapshot(Some(1), json!({"isHero":true,"ready":false}));
    assert!(
        runner
            .tick(&RuleContext {
                now_ms: 0,
                stopped: true,
                snapshot: Some(&ready),
                module_settings: &Value::Null
            })
            .commands
            .is_empty()
    );
    let mut settings = json!({"rules:watch":{"enabled":true,"tick_stopped":true}});
    runner.update(&settings).unwrap();
    assert!(
        runner
            .tick(&RuleContext {
                now_ms: 1,
                stopped: false,
                snapshot: Some(&no_ready),
                module_settings: &Value::Null
            })
            .commands
            .is_empty()
    );
    assert_eq!(
        runner
            .tick(&RuleContext {
                now_ms: 10,
                stopped: false,
                snapshot: Some(&ready),
                module_settings: &Value::Null
            })
            .commands
            .len(),
        1
    );
    assert!(
        runner
            .tick(&RuleContext {
                now_ms: 50,
                stopped: false,
                snapshot: Some(&ready),
                module_settings: &Value::Null
            })
            .commands
            .is_empty()
    );
    assert_eq!(
        runner
            .tick(&RuleContext {
                now_ms: 110,
                stopped: false,
                snapshot: Some(&ready),
                module_settings: &Value::Null
            })
            .commands
            .len(),
        1
    );
    settings["rules:watch"]["enabled"] = json!(false);
    runner.update(&settings).unwrap();
    assert!(
        runner
            .tick(&RuleContext {
                now_ms: 220,
                stopped: false,
                snapshot: Some(&ready),
                module_settings: &Value::Null
            })
            .commands
            .is_empty()
    );
}
