use serde_json::json;
use xenomit_behavior_engine::*;
use xenomit_game_api::{
    Bounds, EntityCategory, GameEntity, GameMapInfo, GameSnapshot, Point as ApiPoint, StageInfo,
};

struct OpenMap;
impl Navigation for OpenMap {}

fn entity(id: i64, name: &str, category: EntityCategory, x: f64, y: f64) -> GameEntity {
    GameEntity {
        path: format!("stage.{id}"),
        id: Some(json!(id)),
        name: name.into(),
        class_name: name.into(),
        category,
        position: ApiPoint { x, y },
        bounds: Bounds::default(),
        health: None,
        max_health: None,
        shield: None,
        max_shield: None,
        fields: Default::default(),
    }
}
fn frame(entities: Vec<GameEntity>) -> GameSnapshot {
    let hero = entity(1, "hero", EntityCategory::Player, 0.0, 0.0);
    GameSnapshot {
        navigation: Default::default(),
        observed_at_unix_ms: 0,
        stage: StageInfo::default(),
        map: None,
        entities: std::iter::once(hero).chain(entities).collect(),
        visited: 0,
        truncated: false,
    }
}
fn npc(id: i64, x: f64, y: f64) -> GameEntity {
    entity(id, "npc", EntityCategory::Npc, x, y)
}
fn box_at(id: i64, x: f64, y: f64) -> GameEntity {
    entity(id, "box", EntityCategory::Unknown, x, y)
}
fn config(mode: BehaviorMode) -> BehaviorConfig {
    let mut c = BehaviorConfig {
        mode,
        ..Default::default()
    };
    c.collector.boxes = vec![BoxRule {
        selector: EntitySelector::name("box"),
        ..Default::default()
    }];
    c.npc_killer.npcs = vec![NpcRule {
        selector: EntitySelector::name("npc"),
        ..Default::default()
    }];
    c
}
fn context(f: &GameSnapshot) -> ModuleContext {
    ModuleContext::from_snapshot(f, &f.entities[0], CoordinateTransform::default())
}
fn movement(d: &BehaviorDecision) -> Vec<Point> {
    d.actions
        .iter()
        .filter_map(|a| match a {
            ActionIntent::MoveTo { destination } => Some(*destination),
            _ => None,
        })
        .collect()
}
fn pickup(d: &BehaviorDecision) -> Option<(EntityRef, u64)> {
    d.actions.iter().find_map(|a| match a {
        ActionIntent::CollectBox { target, wait_ms } => Some((target.clone(), *wait_ms)),
        _ => None,
    })
}
fn attack(d: &BehaviorDecision) -> bool {
    d.actions
        .iter()
        .any(|a| matches!(a, ActionIntent::AttackNpc { .. }))
}
fn id(target: &Option<EntityRef>) -> Option<i64> {
    target.as_ref().and_then(|e| e.id.as_ref()?.as_i64())
}

#[test]
fn combined_pickup_has_no_competing_orbit_and_attack_continues_while_waiting() {
    let f = frame(vec![
        npc(2, 560.0, 0.0),
        box_at(3, 100.0, 0.0),
        box_at(4, 20.0, 0.0),
    ]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::KillAndCollect)).unwrap();
    let first = engine.step(&f, 100);
    assert_eq!(id(&first.selected_box), Some(4));
    assert!(pickup(&first).is_some());
    assert!(movement(&first).is_empty());
    assert!(attack(&first));
    let second = engine.step(&f, 101);
    assert_eq!(id(&second.selected_box), Some(4));
    assert!(second.suspended);
    assert!(!second.can_refresh);
    assert!(attack(&second));
    assert!(pickup(&second).is_none());
    assert!(movement(&second).is_empty());
}

#[test]
fn combined_approach_has_one_box_destination() {
    let f = frame(vec![npc(2, 560.0, 0.0), box_at(3, 400.0, 0.0)]);
    let d = BehaviorEngine::new(config(BehaviorMode::KillAndCollect))
        .unwrap()
        .step(&f, 0);
    assert_eq!(movement(&d), vec![Point { x: 400.0, y: 0.0 }]);
    assert!(attack(&d));
}

#[test]
fn wait_includes_travel_padding_and_capped_retry_backoff() {
    let f = frame(vec![box_at(2, 100.0, 0.0)]);
    let mut c = context(&f);
    c.hero_speed = Some(400.0);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    let mut now = 100;
    for retry in 0..13 {
        let d = engine.step_with_context(&f, now, &c, &OpenMap);
        let wait = pickup(&d).unwrap().1;
        assert_eq!(wait, 500 + ((retry + 1) * 100).min(1000) + 250 + 30);
        assert!(
            engine
                .step_with_context(&f, now + wait, &c, &OpenMap)
                .suspended
        );
        now += wait + 1;
    }
}

#[test]
fn removed_or_invalid_box_cancels_wait_immediately() {
    for invalid in [false, true] {
        let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
        let mut f = frame(vec![box_at(2, 10.0, 0.0), box_at(3, 50.0, 0.0)]);
        engine.step(&f, 0);
        if invalid {
            f.entities[1].fields.insert("isValid".into(), json!(false));
        } else {
            f.entities.remove(1);
        }
        let d = engine.step(&f, 1);
        assert_eq!(id(&d.selected_box), Some(3));
        assert!(pickup(&d).is_some());
    }
}

#[test]
fn recent_attempt_collected_flag_does_not_cancel_an_active_pickup_wait() {
    let mut f = frame(vec![box_at(2, 10.0, 0.0), box_at(3, 50.0, 0.0)]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    engine.step(&f, 0);
    f.entities[1]
        .fields
        .insert("isCollected".into(), json!(true));
    let d = engine.step(&f, 1);
    assert!(d.suspended);
    assert_eq!(id(&d.selected_box), Some(2));
    assert!(pickup(&d).is_none());
    assert_eq!(id(&engine.step(&f, 1000).selected_box), Some(3));
}

#[test]
fn locked_full_health_obstacle_and_regenerating_shield_targets_are_blacklisted() {
    for (closest, hp, shield, moving, increased, ignored) in [
        (651.0, 100.0, 0.0, true, false, true),
        (651.0, 50.0, 0.0, true, false, false),
        (501.0, 50.0, 100.0, false, false, true),
        (501.0, 50.0, 50.0, false, true, true),
        (501.0, 50.0, 100.0, true, false, false),
    ] {
        let mut f = frame(vec![npc(2, 300.0, 0.0)]);
        f.entities[1].health = Some(hp);
        f.entities[1].max_health = Some(100.0);
        f.entities[1].shield = Some(shield);
        f.entities[1].max_shield = Some(100.0);
        let mut c = context(&f);
        let mut engine = BehaviorEngine::new(config(BehaviorMode::NpcKiller)).unwrap();
        engine.step_with_context(&f, 0, &c, &OpenMap);
        c.hero_locked_target = Some("id:2".into());
        c.hero_attacking = Some("id:2".into());
        let n = c.entities.get_mut("id:2").unwrap();
        n.closest_distance = Some(closest);
        n.moving = Some(moving);
        n.shield_increased = increased;
        assert_eq!(
            engine
                .step_with_context(&f, 1, &c, &OpenMap)
                .selected_npc
                .is_none(),
            ignored
        );
    }
}

#[test]
fn invoke_passive_target_without_casting_ability_uses_ten_minute_blacklist() {
    let mut cfg = config(BehaviorMode::NpcKiller);
    cfg.npc_killer.npcs[0].selector = EntitySelector {
        category: Some(EntityCategory::Npc),
        ..Default::default()
    };
    cfg.npc_killer.npcs[0].flags = vec![NpcFlag::Passive];
    let f = frame(vec![entity(2, "Invoke", EntityCategory::Npc, 300.0, 0.0)]);
    let mut c = context(&f);
    let mut engine = BehaviorEngine::new(cfg).unwrap();
    engine.step_with_context(&f, 0, &c, &OpenMap);
    c.hero_locked_target = Some("id:2".into());
    c.hero_local_target = Some("id:2".into());
    assert!(
        engine
            .step_with_context(&f, 1, &c, &OpenMap)
            .selected_npc
            .is_none()
    );
    c.casting_ability = true;
    assert!(
        engine
            .step_with_context(&f, 600_000, &c, &OpenMap)
            .selected_npc
            .is_none()
    );
    assert!(
        engine
            .step_with_context(&f, 600_001, &c, &OpenMap)
            .selected_npc
            .is_some()
    );
}

#[test]
fn map_change_clears_entity_waits_but_preserves_successful_cloak_cooldown() {
    let mut f = frame(vec![box_at(2, 10.0, 0.0)]);
    f.map = Some(GameMapInfo {
        id: Some(1),
        name: None,
        width: None,
        height: None,
        player_position: None,
        safe_places: vec![],
        in_safe_place: None,
    });
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.auto_cloak = true;
    let mut engine = BehaviorEngine::new(cfg).unwrap();
    engine.step(&f, 0);
    engine.cloak_used(0);
    f.map.as_mut().unwrap().id = Some(2);
    let d = engine.step(&f, 1);
    assert!(pickup(&d).is_some());
    assert!(!d.controls.cloak);
}

#[test]
fn map_bounds_are_checked_in_original_space_under_coordinate_transform() {
    let mut f = frame(vec![npc(2, 50.0, 50.0)]);
    f.entities[0].position = ApiPoint { x: 60.0, y: 50.0 };
    f.map = Some(GameMapInfo {
        id: Some(1),
        name: None,
        width: Some(100.0),
        height: Some(100.0),
        player_position: None,
        safe_places: vec![],
        in_safe_place: None,
    });
    let mut cfg = config(BehaviorMode::NpcKiller);
    cfg.coordinates = CoordinateTransform {
        xx: 2.0,
        yy: 2.0,
        translate_x: 1000.0,
        translate_y: 2000.0,
        ..Default::default()
    };
    cfg.npc_killer.npcs[0].combat_radius = 50.0;
    let d = BehaviorEngine::new(cfg).unwrap().step(&f, 0);
    for p in movement(&d) {
        assert!((1000.0..=1200.0).contains(&p.x));
        assert!((2000.0..=2200.0).contains(&p.y));
    }
    assert!(attack(&d));
}

#[test]
fn failed_pickup_is_immediately_retryable_without_added_backoff() {
    let f = frame(vec![box_at(2, 10.0, 0.0)]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    let (target, wait) = pickup(&engine.step(&f, 0)).unwrap();
    engine.collection_failed(&target);
    assert_eq!(pickup(&engine.step(&f, 1)).unwrap().1, wait);
}

#[test]
fn collector_keeps_current_box_until_alternative_saves_more_than_150_units() {
    let mut f = frame(vec![box_at(2, 400.0, 0.0)]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    engine.step(&f, 0);
    f.entities.push(box_at(3, 260.0, 0.0));
    assert_eq!(id(&engine.step(&f, 1).selected_box), Some(2));
    f.entities[2].position.x = 249.0;
    assert_eq!(id(&engine.step(&f, 2).selected_box), Some(3));
}

#[test]
fn box_priorities_rank_initial_target_before_distance() {
    let mut c = config(BehaviorMode::Collector);
    c.collector.boxes.insert(
        0,
        BoxRule {
            selector: EntitySelector {
                id: Some(json!(3)),
                ..Default::default()
            },
            priority: 1,
            ..Default::default()
        },
    );
    let d = BehaviorEngine::new(c)
        .unwrap()
        .step(&frame(vec![box_at(2, 10.0, 0.0), box_at(3, 400.0, 0.0)]), 0);
    assert_eq!(id(&d.selected_box), Some(3));
}

#[test]
fn resource_cargo_limit_does_not_block_bonus_boxes() {
    let f = frame(vec![box_at(2, 10.0, 0.0), box_at(3, 20.0, 0.0)]);
    let mut c = context(&f);
    c.cargo = Some(100.0);
    c.max_cargo = Some(100.0);
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.boxes.insert(
        0,
        BoxRule {
            selector: EntitySelector {
                id: Some(json!(2)),
                ..Default::default()
            },
            resource: true,
            priority: 0,
            ..Default::default()
        },
    );
    let d = BehaviorEngine::new(cfg)
        .unwrap()
        .step_with_context(&f, 0, &c, &OpenMap);
    assert_eq!(id(&d.selected_box), Some(3));
}

#[test]
fn contested_box_uses_destinations_and_pessimistic_travel_time() {
    let f = frame(vec![
        box_at(2, 100.0, 0.0),
        entity(3, "other", EntityCategory::Ship, 200.0, 0.0),
    ]);
    let mut c = context(&f);
    c.hero_speed = Some(400.0);
    let ship = c.entities.get_mut("id:3").unwrap();
    ship.speed = Some(400.0);
    ship.destination = Some(Point { x: 100.0, y: 0.0 });
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.ignore_contested_boxes = true;
    assert!(
        BehaviorEngine::new(cfg.clone())
            .unwrap()
            .step_with_context(&f, 0, &c, &OpenMap)
            .selected_box
            .is_none()
    );
    c.entities.get_mut("id:3").unwrap().destination = None;
    assert!(
        BehaviorEngine::new(cfg)
            .unwrap()
            .step_with_context(&f, 0, &c, &OpenMap)
            .selected_box
            .is_some()
    );
}

#[test]
fn ignore_boxes_flag_alone_enables_npc_distance_restriction() {
    let f = frame(vec![npc(2, 2000.0, 0.0), box_at(3, 100.0, 0.0)]);
    let mut cfg = config(BehaviorMode::KillAndCollect);
    assert!(pickup(&BehaviorEngine::new(cfg.clone()).unwrap().step(&f, 0)).is_some());
    cfg.npc_killer.npcs[0].flags.push(NpcFlag::IgnoreBoxes);
    let d = BehaviorEngine::new(cfg).unwrap().step(&f, 0);
    assert!(d.selected_box.is_none());
    assert!(attack(&d));
}

#[test]
fn low_target_hp_and_hero_collection_radius_control_detours() {
    for (hp, box_distance, collect) in [
        (24.0, 100.0, false),
        (25.0, 100.0, true),
        (100.0, 501.0, false),
    ] {
        let mut n = npc(2, 560.0, 0.0);
        n.health = Some(hp);
        n.max_health = Some(100.0);
        let f = frame(vec![n, box_at(3, box_distance, 0.0)]);
        let d = BehaviorEngine::new(config(BehaviorMode::KillAndCollect))
            .unwrap()
            .step(&f, 0);
        assert_eq!(d.selected_box.is_some(), collect);
        assert!(attack(&d));
    }
}

#[test]
fn circling_moves_tangentially_even_at_correct_radius() {
    let mut f = frame(vec![npc(2, 0.0, 0.0)]);
    f.entities[0].position.x = 560.0;
    let d = BehaviorEngine::new(config(BehaviorMode::NpcKiller))
        .unwrap()
        .step(&f, 0);
    let p = movement(&d)[0];
    assert!(p.y > 0.0);
    assert!((p.x.hypot(p.y) - 560.0).abs() < 1e-6);
    assert!(attack(&d));
}

#[test]
fn orbit_prefers_direction_without_obstacle() {
    struct LowerHalf;
    impl Navigation for LowerHalf {
        fn can_move(&self, p: Point) -> bool {
            p.y <= 0.0
        }
    }
    let mut f = frame(vec![npc(2, 0.0, 0.0)]);
    f.entities[0].position.x = 560.0;
    let d = BehaviorEngine::new(config(BehaviorMode::NpcKiller))
        .unwrap()
        .step_with_context(&f, 0, &context(&f), &LowerHalf);
    assert!(movement(&d)[0].y < 0.0);
}

#[test]
fn orbit_avoids_other_npc_danger_radius() {
    let hazard = entity(3, "hazard", EntityCategory::Npc, 471.0, 303.0);
    let mut f = frame(vec![npc(2, 0.0, 0.0), hazard]);
    f.entities[0].position.x = 560.0;
    let d = BehaviorEngine::new(config(BehaviorMode::NpcKiller))
        .unwrap()
        .step(&f, 0);
    assert_eq!(id(&d.selected_npc), Some(2));
    assert!(movement(&d)[0].y < 0.0);
}

#[test]
fn no_circle_reuses_valid_destination_but_large_radii_always_circle() {
    for (radius, expected_move) in [(560.0, false), (800.0, true)] {
        let mut cfg = config(BehaviorMode::NpcKiller);
        cfg.npc_killer.npcs[0].flags = vec![NpcFlag::NoCircle];
        cfg.npc_killer.npcs[0].combat_radius = radius;
        let mut f = frame(vec![npc(2, 0.0, 0.0)]);
        f.entities[0].position.x = radius;
        let mut c = context(&f);
        c.destination = Some(Point { x: 300.0, y: 0.0 });
        let d = BehaviorEngine::new(cfg)
            .unwrap()
            .step_with_context(&f, 0, &c, &OpenMap);
        assert_eq!(!movement(&d).is_empty(), expected_move);
        assert!(attack(&d));
    }
}

#[test]
fn moving_target_is_predicted_250_ms_ahead() {
    let mut cfg = config(BehaviorMode::NpcKiller);
    cfg.npc_killer.npcs[0].combat_radius = 0.0;
    let f = frame(vec![npc(2, 1000.0, 0.0)]);
    let mut c = context(&f);
    let n = c.entities.get_mut("id:2").unwrap();
    n.speed = Some(400.0);
    n.destination = Some(Point { x: 1500.0, y: 0.0 });
    let d = BehaviorEngine::new(cfg)
        .unwrap()
        .step_with_context(&f, 0, &c, &OpenMap);
    assert_eq!(movement(&d), vec![Point { x: 1100.0, y: 0.0 }]);
}

#[test]
fn attacked_npc_stays_blacklisted_for_20_seconds() {
    let mut f = frame(vec![npc(2, 300.0, 0.0)]);
    f.entities[1]
        .fields
        .insert("attackedByOthers".into(), json!(true));
    let mut engine = BehaviorEngine::new(config(BehaviorMode::NpcKiller)).unwrap();
    assert!(engine.step(&f, 0).selected_npc.is_none());
    f.entities[1].fields.clear();
    assert!(engine.step(&f, 19_999).selected_npc.is_none());
    assert!(engine.step(&f, 20_000).selected_npc.is_some());
}

#[test]
fn attack_second_requires_attacked_target_and_ignore_attacked_flag() {
    let mut cfg = config(BehaviorMode::NpcKiller);
    cfg.npc_killer.npcs[0].flags = vec![NpcFlag::AttackSecond, NpcFlag::IgnoreAttacked];
    let mut f = frame(vec![npc(2, 300.0, 0.0)]);
    assert!(
        BehaviorEngine::new(cfg.clone())
            .unwrap()
            .step(&f, 0)
            .selected_npc
            .is_none()
    );
    f.entities[1]
        .fields
        .insert("attackedByOthers".into(), json!(true));
    assert!(
        BehaviorEngine::new(cfg)
            .unwrap()
            .step(&f, 0)
            .selected_npc
            .is_some()
    );
}

#[test]
fn passive_and_leech_only_are_gated_by_observed_hero_state() {
    let f = frame(vec![npc(2, 300.0, 0.0)]);
    for flag in [NpcFlag::Passive, NpcFlag::LeechOnly] {
        let mut cfg = config(BehaviorMode::NpcKiller);
        cfg.npc_killer.npcs[0].flags = vec![flag];
        let mut c = context(&f);
        assert!(
            BehaviorEngine::new(cfg.clone())
                .unwrap()
                .step_with_context(&f, 0, &c, &OpenMap)
                .selected_npc
                .is_none()
        );
        c.energy_leech = true;
        c.entities.get_mut("id:2").unwrap().attacking_hero = true;
        assert!(
            BehaviorEngine::new(cfg)
                .unwrap()
                .step_with_context(&f, 0, &c, &OpenMap)
                .selected_npc
                .is_some()
        );
    }
}

#[test]
fn locked_unowned_target_is_blacklisted_unless_ownership_is_ignored() {
    for ignore in [false, true] {
        let mut cfg = config(BehaviorMode::NpcKiller);
        if ignore {
            cfg.npc_killer.npcs[0].flags.push(NpcFlag::IgnoreOwnership);
        }
        let f = frame(vec![npc(2, 300.0, 0.0)]);
        let mut c = context(&f);
        let mut engine = BehaviorEngine::new(cfg).unwrap();
        engine.step_with_context(&f, 0, &c, &OpenMap);
        c.hero_locked_target = Some("id:2".into());
        c.entities.get_mut("id:2").unwrap().owned = Some(false);
        assert_eq!(
            engine
                .step_with_context(&f, 1, &c, &OpenMap)
                .selected_npc
                .is_some(),
            ignore
        );
        c.entities.get_mut("id:2").unwrap().owned = Some(true);
        assert_eq!(
            engine
                .step_with_context(&f, 4999, &c, &OpenMap)
                .selected_npc
                .is_some(),
            ignore
        );
        assert!(
            engine
                .step_with_context(&f, 5001, &c, &OpenMap)
                .selected_npc
                .is_some()
        );
    }
}

#[test]
fn unlocked_unreachable_target_uses_short_blacklist() {
    let f = frame(vec![npc(2, 300.0, 0.0)]);
    let mut c = context(&f);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::NpcKiller)).unwrap();
    engine.step_with_context(&f, 0, &c, &OpenMap);
    c.entities.get_mut("id:2").unwrap().closest_distance = Some(601.0);
    assert!(
        engine
            .step_with_context(&f, 1, &c, &OpenMap)
            .selected_npc
            .is_none()
    );
    c.entities.get_mut("id:2").unwrap().closest_distance = Some(0.0);
    assert!(
        engine
            .step_with_context(&f, 1000, &c, &OpenMap)
            .selected_npc
            .is_none()
    );
    assert!(
        engine
            .step_with_context(&f, 1001, &c, &OpenMap)
            .selected_npc
            .is_some()
    );
}

#[test]
fn actively_attacked_target_survives_preferred_zone_change() {
    let mut cfg = config(BehaviorMode::NpcKiller);
    cfg.npc_killer.only_kill_preferred = true;
    let f = frame(vec![npc(2, 300.0, 0.0)]);
    let mut c = context(&f);
    let mut engine = BehaviorEngine::new(cfg).unwrap();
    engine.step_with_context(&f, 0, &c, &OpenMap);
    c.entities.get_mut("id:2").unwrap().preferred = Some(false);
    c.hero_attacking = Some("id:2".into());
    assert!(
        engine
            .step_with_context(&f, 1, &c, &OpenMap)
            .selected_npc
            .is_some()
    );
    c.hero_attacking = None;
    assert!(
        engine
            .step_with_context(&f, 2, &c, &OpenMap)
            .selected_npc
            .is_none()
    );
}

#[test]
fn current_target_dynamic_priority_beats_health_and_distance() {
    let mut f = frame(vec![npc(2, 300.0, 0.0)]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::NpcKiller)).unwrap();
    engine.step(&f, 0);
    let mut other = npc(3, 100.0, 0.0);
    other.health = Some(1.0);
    other.max_health = Some(100.0);
    f.entities.push(other);
    assert_eq!(id(&engine.step(&f, 1).selected_npc), Some(2));
}

#[test]
fn zero_max_hp_is_unknown_but_exhausted_readable_hp_is_dead() {
    let mut f = frame(vec![npc(2, 300.0, 0.0)]);
    f.entities[1].health = Some(0.0);
    f.entities[1].max_health = Some(0.0);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::NpcKiller)).unwrap();
    assert!(engine.step(&f, 0).selected_npc.is_some());
    f.entities[1].max_health = Some(100.0);
    assert!(engine.step(&f, 1).selected_npc.is_none());
}

#[test]
fn reachability_uses_entity_to_path_distance_instead_of_hero_distance() {
    struct Blocked;
    impl Navigation for Blocked {
        fn closest_distance(&self, _: &GameEntity, _: Point) -> f64 {
            500.0
        }
    }
    let f = frame(vec![npc(2, 3000.0, 0.0), box_at(3, 100.0, 0.0)]);
    let cfg = config(BehaviorMode::KillAndCollect);
    assert!(
        BehaviorEngine::new(cfg.clone())
            .unwrap()
            .step(&f, 0)
            .selected_npc
            .is_some()
    );
    let d = BehaviorEngine::new(cfg).unwrap().step_with_context(
        &f,
        0,
        &ModuleContext::default(),
        &Blocked,
    );
    assert!(d.selected_npc.is_none());
    assert!(d.selected_box.is_none());
}

#[test]
fn cloak_cooldown_starts_only_after_successful_item_use() {
    let f = frame(vec![]);
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.auto_cloak = true;
    let mut engine = BehaviorEngine::new(cfg).unwrap();
    assert!(engine.step(&f, 0).controls.cloak);
    assert!(engine.step(&f, 1).controls.cloak);
    engine.cloak_used(1);
    assert!(!engine.step(&f, 30_001).controls.cloak);
    assert!(engine.step(&f, 30_002).controls.cloak);
}

#[test]
fn danger_avoidance_cannot_be_overwritten_by_collection() {
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.stay_away_from_enemies = true;
    let f = frame(vec![
        entity(2, "enemy", EntityCategory::Ship, 300.0, 0.0),
        box_at(3, 100.0, 0.0),
    ]);
    let mut c = context(&f);
    c.entities.get_mut("id:2").unwrap().enemy = true;
    let d = BehaviorEngine::new(cfg)
        .unwrap()
        .step_with_context(&f, 0, &c, &OpenMap);
    assert_eq!(movement(&d).len(), 1);
    assert!(movement(&d)[0].x < 0.0);
    assert!(pickup(&d).is_none());
    assert_eq!(d.controls.ship_mode, Some(ShipMode::Run));
}

#[test]
fn blocked_escape_does_not_emit_unreachable_movement_or_resume_roaming() {
    struct Blocked;
    impl Navigation for Blocked {
        fn can_move(&self, _: Point) -> bool {
            false
        }
    }
    let mut cfg = config(BehaviorMode::Collector);
    cfg.collector.stay_away_from_enemies = true;
    let f = frame(vec![entity(2, "enemy", EntityCategory::Ship, 300.0, 0.0)]);
    let mut c = context(&f);
    c.entities.get_mut("id:2").unwrap().enemy = true;
    let d = BehaviorEngine::new(cfg)
        .unwrap()
        .step_with_context(&f, 0, &c, &Blocked);
    assert!(d.suspended);
    assert!(movement(&d).is_empty());
}

#[test]
fn safety_suspends_combat_but_collector_finishes_existing_pickup_first() {
    let f = frame(vec![npc(2, 300.0, 0.0), box_at(3, 10.0, 0.0)]);
    let mut c = context(&f);
    c.safe_to_work = false;
    c.safety_waiting = true;
    let d = BehaviorEngine::new(config(BehaviorMode::NpcKiller))
        .unwrap()
        .step_with_context(&f, 0, &c, &OpenMap);
    assert!(d.suspended);
    assert!(d.selected_npc.is_none());
    assert!(d.can_refresh);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    engine.step(&f, 0);
    let d = engine.step_with_context(&f, 1, &c, &OpenMap);
    assert_eq!(id(&d.selected_box), Some(3));
    assert!(!d.can_refresh);
}

#[test]
fn temporary_map_travel_suspends_and_resumes_original_mode() {
    let mut cfg = config(BehaviorMode::KillAndCollect);
    cfg.working_map = Some(2);
    let mut f = frame(vec![
        entity(8, "portal", EntityCategory::Portal, 100.0, 0.0),
        npc(3, 300.0, 0.0),
    ]);
    f.map = Some(GameMapInfo {
        id: Some(1),
        name: None,
        width: None,
        height: None,
        player_position: None,
        safe_places: vec![],
        in_safe_place: None,
    });
    let mut engine = BehaviorEngine::new(cfg).unwrap();
    let d = engine.step(&f, 0);
    assert!(d.suspended);
    assert_eq!(d.controls.travel_to_map, Some(2));
    assert!(!attack(&d));
    f.map.as_mut().unwrap().id = Some(2);
    let d = engine.step(&f, 1);
    assert!(!d.suspended);
    assert!(attack(&d));
    assert_eq!(d.mode, BehaviorMode::KillAndCollect);
}

#[test]
fn config_change_clears_wait_and_invalid_change_keeps_existing_state() {
    let f = frame(vec![box_at(2, 10.0, 0.0)]);
    let mut engine = BehaviorEngine::new(config(BehaviorMode::Collector)).unwrap();
    engine.step(&f, 0);
    let mut bad = config(BehaviorMode::Collector);
    bad.npc_killer.max_circle_iterations = 0;
    assert!(engine.replace_config(bad).is_err());
    assert!(engine.step(&f, 1).suspended);
    engine
        .replace_config(config(BehaviorMode::Collector))
        .unwrap();
    assert!(pickup(&engine.step(&f, 2)).is_some());
}

#[test]
fn legacy_json_loads_with_new_defaults_and_example_is_valid() {
    let config = BehaviorConfig::from_json(
        r#"{"mode":"npc_killer","npc_killer":{"npcs":[{"selector":{"category":"npc"}}]}}"#,
    )
    .unwrap();
    assert_eq!(config.npc_killer.max_circle_iterations, 5);
    assert!(BehaviorEngine::from_json(include_str!("../example-config.json")).is_ok());
}
