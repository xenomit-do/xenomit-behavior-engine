use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use xenomit_behavior_engine::*;
use xenomit_game_api::{
    EntityCategory, GameEntity, GameMapInfo, GameSnapshot, ItemState, MapDescriptor, MapLink,
    NavigationState, Point as WorldPoint, StageInfo, StarSystem,
};

fn entity(category: EntityCategory, id: i64, name: &str, x: f64, y: f64) -> GameEntity {
    GameEntity {
        path: format!("/entity/{id}"),
        class_name: name.into(),
        name: name.into(),
        category,
        position: WorldPoint { x, y },
        bounds: Default::default(),
        id: Some(json!(id)),
        health: Some(100.0),
        max_health: Some(100.0),
        shield: Some(100.0),
        max_shield: Some(100.0),
        fields: Default::default(),
    }
}
fn snapshot() -> GameSnapshot {
    let mut hero = entity(EntityCategory::Player, 1, "hero", 0.0, 0.0);
    hero.fields.insert("localTargetId".into(), json!(2));
    let mut target = entity(EntityCategory::Npc, 2, "NPC", 100.0, 0.0);
    target.fields.insert("isEnemy".into(), json!(true));
    GameSnapshot {
        navigation: NavigationState {
            map_id: Some(1),
            ..Default::default()
        },
        observed_at_unix_ms: 0,
        stage: StageInfo::default(),
        map: Some(GameMapInfo {
            id: Some(1),
            name: Some("1-1".into()),
            width: Some(10000.0),
            height: Some(10000.0),
            player_position: None,
            safe_places: vec![],
            in_safe_place: None,
        }),
        entities: vec![hero, target],
        visited: 2,
        truncated: false,
    }
}
fn settings(id: &str, options: Value) -> Value {
    json!({id: {"enabled": true, "options": options}})
}
fn tick(runner: &mut BehaviorRunner, now_ms: u64, s: &GameSnapshot) -> RuleOutput {
    runner.tick(&RuleContext {
        now_ms,
        snapshot: Some(s),
        module_settings: &Value::Null,
        stopped: false,
    })
}
fn item(
    s: &mut GameSnapshot,
    id: &str,
    available: bool,
    activatable: bool,
    blocked: bool,
    quantity: f64,
) {
    s.navigation.hero.items.insert(
        id.into(),
        ItemState {
            quantity: Some(quantity),
            available,
            activatable,
            selected: false,
            blocked,
        },
    );
}

#[test]
fn retained_priority_takeover_suppresses_competing_controls_and_owns_failure() {
    struct Rule {
        ticked: Arc<AtomicUsize>,
        failed: Arc<AtomicUsize>,
        takeover: bool,
    }
    impl GeneralBehavior for Rule {
        fn tick(&mut self, _: &RuleContext<'_>) -> RuleOutput {
            self.ticked.fetch_add(1, Ordering::SeqCst);
            if self.takeover {
                RuleOutput {
                    takeover: Some(BehaviorDecision {
                        mode: BehaviorMode::Collector,
                        player: None,
                        selected_npc: None,
                        selected_box: None,
                        actions: vec![],
                        note: None,
                        controls: ControlRequests::default(),
                        suspended: true,
                        can_refresh: false,
                    }),
                    ..Default::default()
                }
            } else {
                RuleOutput {
                    commands: vec![xenomit_game_api::GameCommand::UseItem {
                        item_id: "lower".into(),
                        activate: true,
                    }],
                    ..Default::default()
                }
            }
        }
        fn update(&mut self, _: &Value) -> Result<(), String> {
            Ok(())
        }
        fn plan_failed(&mut self) {
            self.failed.fetch_add(1, Ordering::SeqCst);
        }
    }
    let (high_ticks, low_ticks, high_failed, low_failed) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );
    let mut registry = BehaviorRegistry::default();
    for (priority, id, takeover, ticks, failed) in [
        (0, "first", true, high_ticks.clone(), high_failed.clone()),
        (1, "second", false, low_ticks.clone(), low_failed.clone()),
    ] {
        registry
            .register(priority, ModuleDescriptor::new(id, id), move |_| {
                Ok(Box::new(Rule {
                    ticked: ticks.clone(),
                    failed: failed.clone(),
                    takeover,
                }))
            })
            .unwrap();
    }
    let mut runner = registry.create(&json!({})).unwrap();
    let result = tick(&mut runner, 1, &snapshot());
    assert!(result.takeover.is_some());
    assert!(
        result.commands.is_empty(),
        "a lower priority rule must not issue a control alongside the takeover"
    );
    assert_eq!(high_ticks.load(Ordering::SeqCst), 1);
    assert_eq!(
        low_ticks.load(Ordering::SeqCst),
        0,
        "lower rules are skipped once an exclusive takeover owns the frame"
    );
    runner.plan_failed();
    assert_eq!(high_failed.load(Ordering::SeqCst), 1);
    assert_eq!(low_failed.load(Ordering::SeqCst), 0);
}

#[test]
fn unknown_condition_data_never_authorizes_custom_event() {
    let invalid = settings(
        "custom_events",
        json!({"events":[{"condition":{"future_sensor":true},"item":"mystery"}]}),
    );
    assert!(
        builtin_behavior_registry().create(&invalid).is_err(),
        "unknown condition data is rejected before it can authorize an action"
    );
}

#[test]
fn anti_push_counts_death_rising_edges_and_holds_pause_until_deadline() {
    let mut s = snapshot();
    s.entities[0]
        .fields
        .insert("lastDestroyerId".into(), json!(99));
    let mut runner = builtin_behavior_registry()
        .create(&settings(
            "anti_push",
            json!({"max_deaths":2,"death_pause_minutes":1,"draw_fire_pause_minutes":-1}),
        ))
        .unwrap();
    s.entities[0].health = Some(0.0);
    assert!(
        tick(&mut runner, 1000, &s).takeover.is_none(),
        "one death is below the configured threshold"
    );
    assert!(
        tick(&mut runner, 1100, &s).takeover.is_none(),
        "a sustained dead frame is one death, not repeated edges"
    );
    s.entities[0].health = Some(100.0);
    tick(&mut runner, 1200, &s);
    s.entities[0].health = Some(0.0);
    let pause = tick(&mut runner, 2000, &s);
    assert!(pause.takeover.is_some());
    assert!(
        !pause.stop,
        "positive pause duration should suspend rather than disconnect"
    );
    s.entities[0].health = Some(100.0);
    assert!(
        tick(&mut runner, 60_000, &s).takeover.is_some(),
        "pause persists before the deadline"
    );
    assert!(
        tick(&mut runner, 62_001, &s).takeover.is_none(),
        "rule resumes after the pause deadline"
    );
}

#[test]
fn module_snapshot_filters_captcha_boxes_but_keeps_ordinary_boxes() {
    let runner = builtin_behavior_registry().create(&Value::Null).unwrap();
    let mut s = snapshot();
    for (id, type_name) in [
        (10, "POISON_PUSAT_BOX_BLACK"),
        (11, "BONUS_BOX_RED"),
        (12, "BONUS_BOX_BLUE"),
    ] {
        let mut e = entity(EntityCategory::Unknown, id, "box", 20.0, 20.0);
        e.fields.insert("typeName".into(), json!(type_name));
        s.entities.push(e);
    }
    let filtered = runner.module_snapshot(&s);
    assert_eq!(
        filtered
            .entities
            .iter()
            .filter(|e| e.fields.contains_key("typeName"))
            .map(|e| e.id.clone().unwrap())
            .collect::<Vec<_>>(),
        vec![json!(12)]
    );
}

#[test]
fn numeric_draw_fire_effect_requests_anti_push_disconnect() {
    let mut state = snapshot();
    state.entities[1]
        .fields
        .insert("effects".into(), json!([36]));
    let mut runner = builtin_behavior_registry().create(&json!({})).unwrap();
    let result = tick(&mut runner, 1000, &state);
    assert!(result.stop);
    assert!(result.host_actions.contains(&HostAction::Disconnect));
}

#[test]
fn best_formation_uses_canonical_bat_item_when_in_range() {
    let mut state = snapshot();
    state.entities[0]
        .fields
        .insert("isAttacking".into(), json!(true));
    item(
        &mut state,
        "drone_formation_f-13-bt",
        true,
        true,
        false,
        1.0,
    );
    let mut runner = builtin_behavior_registry()
        .create(&settings("best_formation", json!({})))
        .unwrap();
    assert_eq!(
        tick(&mut runner, 1000, &state).commands,
        vec![xenomit_game_api::GameCommand::UseItem {
            item_id: "drone_formation_f-13-bt".into(),
            activate: true
        }]
    );
}

#[test]
fn booty_filter_only_removes_configured_box_types_with_zero_known_keys() {
    let mut s = snapshot();
    s.entities[0]
        .fields
        .insert("bootyKeys".into(), json!({"GREEN":0,"BLUE":2}));
    for (id, type_name) in [
        (10, "PIRATE_BOOTY_GOLD"),
        (11, "PIRATE_BOOTY_BLUE"),
        (12, "PIRATE_BOOTY_RED"),
    ] {
        let mut e = entity(EntityCategory::Unknown, id, "box", 0.0, 0.0);
        e.fields.insert("typeName".into(), json!(type_name));
        s.entities.push(e);
    }
    filter_booty_boxes(
        &mut s,
        &settings("booty_keys", json!({"keys":["GREEN","BLUE"]})),
    );
    let ids: Vec<_> = s
        .entities
        .iter()
        .filter_map(|e| e.id.as_ref().and_then(Value::as_i64))
        .collect();
    assert!(
        ids.contains(&11),
        "boxes with a positive key count remain eligible"
    );
    assert!(
        ids.contains(&12),
        "unconfigured booty types remain eligible"
    );
    assert!(!ids.contains(&10), "zero-count configured type is filtered");
}

#[test]
fn auto_cloak_waits_after_attack_and_requires_a_ready_item() {
    let mut s = snapshot();
    s.entities[0]
        .fields
        .insert("isAttacking".into(), json!(true));
    item(&mut s, "equipment_extra_cpu_cl04k", true, true, false, 1.0);
    let mut runner = builtin_behavior_registry()
        .create(&settings("auto_cloak", json!({"wait_ms":10000})))
        .unwrap();
    tick(&mut runner, 1000, &s);
    s.entities[0]
        .fields
        .insert("isAttacking".into(), json!(false));
    assert!(tick(&mut runner, 10_999, &s).commands.is_empty());
    assert_eq!(tick(&mut runner, 11_000, &s).commands.len(), 1);

    let mut unavailable = snapshot();
    item(
        &mut unavailable,
        "equipment_extra_cpu_cl04k",
        true,
        false,
        false,
        1.0,
    );
    let mut runner = builtin_behavior_registry()
        .create(&settings("auto_cloak", json!({"wait_ms":0})))
        .unwrap();
    assert!(
        tick(&mut runner, 1000, &unavailable).commands.is_empty(),
        "unactivatable item must not be used"
    );
}

#[test]
fn automatic_ammo_requires_available_unblocked_item_and_minimum_quantity() {
    let mut s = snapshot();
    s.entities[0]
        .fields
        .insert("isAttacking".into(), json!(true));
    item(&mut s, "ammunition_laser_ucb-100", true, true, true, 500.0);
    item(&mut s, "ammunition_laser_lcb-10", true, true, false, 5.0);
    let mut runner = builtin_behavior_registry()
        .create(&settings(
            "best_ammo",
            json!({"minimum":10,"default":"ammunition_laser_lcb-10"}),
        ))
        .unwrap();
    assert!(
        tick(&mut runner, 1000, &s).commands.is_empty(),
        "blocked candidate and below-minimum fallback are both unusable"
    );
    item(&mut s, "ammunition_laser_lcb-10", true, true, false, 10.0);
    assert_eq!(
        tick(&mut runner, 1600, &s).commands,
        vec![xenomit_game_api::GameCommand::UseItem {
            item_id: "ammunition_laser_lcb-10".into(),
            activate: true
        }]
    );
}

#[test]
fn already_selected_best_ammo_does_not_fall_back_to_weaker_ammo() {
    let mut s = snapshot();
    s.entities[0]
        .fields
        .insert("isAttacking".into(), json!(true));
    s.entities[1]
        .fields
        .insert("preferredAmmo".into(), json!("ammunition_laser_ucb-100"));
    s.navigation.hero.items.insert(
        "ammunition_laser_ucb-100".into(),
        ItemState {
            quantity: Some(500.0),
            available: true,
            activatable: true,
            selected: true,
            blocked: false,
        },
    );
    item(&mut s, "ammunition_laser_lcb-10", true, true, false, 500.0);
    let mut runner = builtin_behavior_registry()
        .create(&settings(
            "best_ammo",
            json!({"default":"ammunition_laser_lcb-10"}),
        ))
        .unwrap();
    assert!(
        tick(&mut runner, 1000, &s).commands.is_empty(),
        "the selected preferred ammo already satisfies the rule; do not downgrade to the default"
    );
}

#[test]
fn custom_registry_factory_receives_descriptor_validated_defaults() {
    let calls = Arc::new(AtomicUsize::new(0));
    let factory_calls = calls.clone();
    let mut registry = BehaviorRegistry::default();
    let descriptor = ModuleDescriptor::new("custom", "Custom")
        .setting(SettingDescriptor::new(
            "enabled",
            "Enabled",
            SettingKind::Boolean,
            json!(true),
        ))
        .setting(SettingDescriptor::new(
            "mode",
            "Mode",
            SettingKind::Text,
            json!("safe"),
        ));
    registry
        .register(0, descriptor, move |values| {
            factory_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(values["enabled"], json!(true));
            assert_eq!(values["mode"], json!("safe"));
            Ok(Box::new(NoopRule))
        })
        .unwrap();
    let _runner = registry.create(&Value::Null).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(
        registry
            .create(&json!({"custom":{"enabled":"yes"}}))
            .is_err(),
        "descriptor validation must reject malformed custom settings before construction"
    );
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "invalid settings must not reach the factory"
    );
    let mut runner = registry.create(&Value::Null).unwrap();
    runner
        .update(&json!({"custom":{"enabled":false,"mode":"fast"}}))
        .unwrap();
    assert!(runner.update(&json!({"custom":{"enabled":"no"}})).is_err());
}

#[test]
fn builtin_options_reject_wrong_types_and_negative_event_cooldowns() {
    let registry = builtin_behavior_registry();
    assert!(
        registry
            .create(&settings("auto_cloak", json!({"only_non_pvp":"yes"})))
            .is_err()
    );
    assert!(
        registry
            .create(&settings("auto_cloak", json!({"wait_ms":"later"})))
            .is_err()
    );
    assert!(
        registry
            .create(&settings(
                "custom_events",
                json!({"events":[{"condition":{"elapsed_ms":1},"item":"x","cooldown_ms":-1}]})
            ))
            .is_err()
    );

    let mut runner = registry.create(&Value::Null).unwrap();
    assert!(
        runner
            .update(&settings("auto_cloak", json!({"wait_ms":-5})))
            .is_err()
    );
}

struct NoopRule;
impl GeneralBehavior for NoopRule {
    fn tick(&mut self, _: &RuleContext<'_>) -> RuleOutput {
        RuleOutput::default()
    }
    fn update(&mut self, _: &Value) -> Result<(), String> {
        Ok(())
    }
}

#[test]
fn conditional_events_obey_cooldown_and_retry_when_item_is_not_ready() {
    let mut s = snapshot();
    item(&mut s, "custom", true, false, false, 1.0);
    let mut runner = builtin_behavior_registry()
        .create(&settings(
            "custom_events",
            json!({"events":[{"condition":{"elapsed_ms":1},"item":"custom","cooldown_ms":500}]}),
        ))
        .unwrap();
    assert!(tick(&mut runner, 10, &s).commands.is_empty());
    item(&mut s, "custom", true, true, false, 1.0);
    assert_eq!(tick(&mut runner, 20, &s).commands.len(), 1);
    assert!(tick(&mut runner, 519, &s).commands.is_empty());
    assert_eq!(tick(&mut runner, 520, &s).commands.len(), 1);
}

#[test]
fn profile_changer_fires_once_per_condition_edge() {
    let mut s = snapshot();
    let mut runner = builtin_behavior_registry()
        .create(&settings(
            "profile_changer",
            json!({"profile":"safe","conditions":[{"field":{"danger":true}}]}),
        ))
        .unwrap();
    s.entities[0].fields.insert("danger".into(), json!(true));
    assert_eq!(
        tick(&mut runner, 1000, &s).host_actions,
        vec![HostAction::Profile("safe".into())]
    );
    assert!(tick(&mut runner, 2000, &s).host_actions.is_empty());
    s.entities[0].fields.insert("danger".into(), json!(false));
    tick(&mut runner, 3000, &s);
    s.entities[0].fields.insert("danger".into(), json!(true));
    assert_eq!(
        tick(&mut runner, 4000, &s).host_actions,
        vec![HostAction::Profile("safe".into())]
    );
}

#[test]
fn travel_rule_takes_over_to_route_high_level_ship_via_4_4() {
    let mut s = snapshot();
    s.navigation.map_id = Some(10);
    s.navigation.geometry_ready = true;
    s.navigation.hero.level = Some(10);
    s.navigation.hero.faction = Some(1);
    s.navigation.star_system = StarSystem {
        maps: vec![
            MapDescriptor {
                id: 10,
                name: "1-6".into(),
                min_level: 0,
            },
            MapDescriptor {
                id: 20,
                name: "4-4".into(),
                min_level: 0,
            },
            MapDescriptor {
                id: 30,
                name: "2-6".into(),
                min_level: 0,
            },
        ],
        links: vec![
            MapLink {
                from: 10,
                to: 20,
                portal_id: 100,
            },
            MapLink {
                from: 20,
                to: 30,
                portal_id: 200,
            },
        ],
    };
    s.navigation.portals = vec![xenomit_game_api::PortalInfo {
        id: 100,
        type_id: 0,
        position: WorldPoint { x: 500.0, y: 500.0 },
        target_map: Some(20),
        jumping: false,
    }];
    s.map.as_mut().unwrap().id = Some(10);
    s.entities[0].position = WorldPoint { x: 0.0, y: 0.0 };
    let mut runner = builtin_behavior_registry()
        .create(&settings("fix_bl_travel", json!({})))
        .unwrap();
    // The first observation starts MapTraveler's settle timer for this map.
    runner.tick(&RuleContext {
        now_ms: 3000,
        snapshot: Some(&s),
        module_settings: &json!({"working_map":30}),
        stopped: false,
    });
    // MapTraveler waits for teleport/map state to settle before approaching its first portal.
    let result = runner.tick(&RuleContext {
        now_ms: 9000,
        snapshot: Some(&s),
        module_settings: &json!({"working_map":30}),
        stopped: false,
    });
    assert!(
        result.takeover.is_some(),
        "travel rule should suspend objective movement while routing"
    );
    assert!(
        !result.takeover.unwrap().actions.is_empty(),
        "known outgoing portal should provide a route waypoint"
    );
}
