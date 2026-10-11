use serde_json::{Value, json};
use xenomit_behavior_engine::{
    BehaviorRegistry, RuleContext, builtin_behavior_registry,
    shared::{register_shared_behaviors, register_shared_modules},
};
use xenomit_game_api::{
    EntityCategory, GameEntity, GameSnapshot, ItemState, NavigationState, Point, StageInfo,
};

fn entity(category: EntityCategory, id: i64, x: f64, y: f64) -> GameEntity {
    GameEntity {
        path: format!("/{id}"),
        class_name: "Ship".into(),
        name: format!("ship-{id}"),
        category,
        position: Point { x, y },
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
    let mut hero = entity(EntityCategory::Player, 1, 0.0, 0.0);
    hero.fields.insert("isHero".into(), json!(true));
    let mut result = GameSnapshot {
        navigation: NavigationState::default(),
        observed_at_unix_ms: 0,
        stage: StageInfo::default(),
        map: None,
        entities: vec![hero],
        visited: 0,
        truncated: false,
    };
    for (id, x) in [(2, 100.0), (3, 130.0), (4, 160.0)] {
        let mut npc = entity(EntityCategory::Npc, id, x, 0.0);
        npc.fields.insert("isEnemy".into(), json!(true));
        result.entities.push(npc);
    }
    result
}

#[test]
fn shared_behaviors_register_with_distinct_ids_and_valid_defaults() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let ids: Vec<_> = registry.descriptors().into_iter().map(|d| d.id).collect();
    assert_eq!(
        ids,
        vec![
            "shared_simple_healing",
            "shared_crowd_avoidance",
            "shared_solaris_inc",
            "shared_repair_pet",
            "shared_fix_pet_stuck"
        ]
    );
    registry.create(&Value::Null).unwrap();
}

#[test]
fn crowd_avoidance_takes_over_and_moves_away_from_cluster() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let settings = json!({"shared_crowd_avoidance":{"enabled":true,"options":{"count":3,"radius":500.0,"npcs":true,"enemies":false,"allies":false,"run_mode":true,"preferred_zone_only":false}}});
    let mut runner = registry.create(&settings).unwrap();
    let s = snapshot();
    let out = runner.tick(&RuleContext {
        now_ms: 0,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    });
    let plan = out.takeover.unwrap();
    assert_eq!(plan.note.as_deref(), Some("Shared crowd avoidance"));
    assert!(plan.actions.iter().any(|a| matches!(a,xenomit_behavior_engine::ActionIntent::MoveTo{destination} if destination.x < 0.0)));
}

fn empty_registry() -> BehaviorRegistry {
    builtin_behavior_registry().filtered(|_| false)
}

#[test]
fn shared_module_registration_includes_spaceball_objective() {
    let mut builder = xenomit_behavior_engine::ModuleRegistryBuilder::new();
    register_shared_modules(&mut builder).unwrap();
    assert!(
        builder
            .build()
            .descriptors()
            .iter()
            .any(|d| d.id == "shared_spaceball")
    );
}

#[test]
fn simple_healing_uses_ready_supported_ship_ability() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let settings = json!({"shared_simple_healing":{"enabled":true,"options":{"hp_enabled":true,"hp_threshold":0.5,"shield_enabled":false,"shield_threshold":0.5,"pod_enabled":false,"pod_threshold":0.5}}});
    let mut runner = registry.create(&settings).unwrap();
    let mut s = snapshot();
    let hero = &mut s.entities[0];
    hero.class_name = "ship_aegis".into();
    hero.health = Some(20.0);
    hero.max_health = Some(100.0);
    s.navigation.hero.items.insert(
        "ability_aegis_hp-repair".into(),
        ItemState {
            quantity: Some(1.0),
            available: true,
            activatable: true,
            selected: false,
            blocked: false,
        },
    );
    let out = runner.tick(&RuleContext {
        now_ms: 1000,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    });
    assert!(out.commands.iter().any(|command| matches!(command,xenomit_game_api::GameCommand::UseItem{item_id,..} if item_id=="ability_aegis_hp-repair")));
}

#[test]
fn solaris_uses_observed_ability_and_gates_disabled_or_stopped_ticks() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let settings = json!({"shared_solaris_inc":{"enabled":true,"options":{"min_npcs":2,"max_distance":1000.0,"extra_flag_only":false,"use_pet_kamikaze":false,"stick_min_hp":0.0}}});
    let mut runner = registry.create(&settings).unwrap();
    let mut s = snapshot();
    s.entities[0].class_name = "ship_solaris".into();
    for e in &mut s.entities[1..] {
        e.category = EntityCategory::Npc;
    }
    s.navigation.hero.items.insert(
        "ability_solaris_inc".into(),
        ItemState {
            quantity: Some(1.0),
            available: true,
            activatable: true,
            selected: false,
            blocked: false,
        },
    );
    let out = runner.tick(&RuleContext {
        now_ms: 1000,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    });
    assert!(out.commands.iter().any(|c|matches!(c,xenomit_game_api::GameCommand::UseItem{item_id,..} if item_id=="ability_solaris_inc")));
    let out = runner.tick(&RuleContext {
        now_ms: 2000,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: true,
    });
    assert!(out.commands.is_empty());
}

#[test]
fn pet_repair_only_uses_gear_for_an_explicitly_owned_pet() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let settings = json!({"shared_repair_pet":{"enabled":true,"options":{"hp_threshold":0.8}}});
    let mut runner = registry.create(&settings).unwrap();
    let mut s = snapshot();
    s.navigation.hero.owns_pet = true;
    s.navigation.hero.pet_active = true;
    s.navigation.hero.pet_gears = vec![9];
    let mut other = entity(EntityCategory::Unknown, 88, 10.0, 0.0);
    other.class_name = "Pet".into();
    other.health = Some(1.0);
    other.max_health = Some(100.0);
    other.fields.insert("ownerId".into(), json!(77));
    s.entities.push(other);
    let ctx = RuleContext {
        now_ms: 1_000,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    };
    assert!(runner.tick(&ctx).commands.is_empty());
    let mut own = entity(EntityCategory::Unknown, 89, 10.0, 0.0);
    own.class_name = "Pet".into();
    own.health = Some(1.0);
    own.max_health = Some(100.0);
    own.fields.insert("ownerID".into(), json!(1));
    s.entities.push(own);
    let ctx = RuleContext {
        now_ms: 2_000,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    };
    assert!(runner.tick(&ctx).commands.iter().any(|c| matches!(
        c,
        xenomit_game_api::GameCommand::SelectPetGear { gear_type: 9, .. }
    )));
}

#[test]
fn stuck_pet_refresh_waits_until_combat_ends_and_requires_pet_enabled() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let settings = json!({"shared_fix_pet_stuck":{"enabled":true,"options":{"stuck_seconds":1}}});
    let mut runner = registry.create(&settings).unwrap();
    let mut s = snapshot();
    s.navigation.hero.owns_pet = true;
    s.navigation.hero.pet_active = false;
    s.entities[0].fields.insert("attacking".into(), json!(true));
    s.entities[0]
        .fields
        .insert("attackingTargetId".into(), json!(2));
    let disabled = RuleContext {
        now_ms: 0,
        snapshot: Some(&s),
        module_settings: &Value::Null,
        stopped: false,
    };
    assert!(runner.tick(&disabled).host_actions.is_empty());
    let module = json!({"pet_enabled":true});
    let ctx = RuleContext {
        now_ms: 0,
        snapshot: Some(&s),
        module_settings: &module,
        stopped: false,
    };
    runner.tick(&ctx);
    let ctx = RuleContext {
        now_ms: 1_000,
        snapshot: Some(&s),
        module_settings: &module,
        stopped: false,
    };
    assert!(runner.tick(&ctx).host_actions.is_empty());
    s.entities[0]
        .fields
        .insert("attacking".into(), json!(false));
    s.entities[0]
        .fields
        .insert("attackingTargetId".into(), json!(0));
    let ctx = RuleContext {
        now_ms: 1_500,
        snapshot: Some(&s),
        module_settings: &module,
        stopped: false,
    };
    assert!(
        runner
            .tick(&ctx)
            .host_actions
            .iter()
            .any(|a| matches!(a, xenomit_behavior_engine::HostAction::Refresh))
    );
}

#[test]
fn shared_setting_validation_rejects_empty_crowd_threshold() {
    let mut registry = empty_registry();
    register_shared_behaviors(&mut registry).unwrap();
    let invalid = json!({"shared_crowd_avoidance":{"enabled":true,"options":{"count":0,"radius":500.0,"npcs":true,"enemies":false,"allies":false,"run_mode":true,"preferred_zone_only":false}}});
    assert!(registry.create(&invalid).is_err());
}
