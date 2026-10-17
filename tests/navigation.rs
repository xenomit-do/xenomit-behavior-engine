use serde_json::json;
use std::collections::BTreeSet;
use xenomit_behavior_engine::*;
use xenomit_game_api::{
    Bounds, EntityCategory, GameCommand, GameEntity, GameMapInfo, GameSnapshot, MapDescriptor,
    MapLink, NavigationState, Obstacle, ObstacleShape, Point as WorldPoint, PortalInfo, SafePlace,
    StageInfo, StarSystem,
};

fn point(x: f64, y: f64) -> Point {
    Point { x, y }
}
fn world(x: f64, y: f64) -> WorldPoint {
    WorldPoint { x, y }
}
fn obstacle(shape: ObstacleShape) -> Obstacle {
    Obstacle {
        id: 1,
        name: "NOA".into(),
        shape,
        kind: xenomit_game_api::ObstacleKind::NoAccess,
        inverted: false,
    }
}
fn rectangle(x: f64, y: f64, width: f64, height: f64) -> Obstacle {
    obstacle(ObstacleShape::Rectangle {
        bounds: Bounds {
            x,
            y,
            width,
            height,
        },
    })
}
fn assert_clear(navigation: &ObstacleNavigation<'_>, from: Point, path: &[Point]) {
    assert!(!path.is_empty());
    let mut previous = from;
    for next in path {
        assert!(
            navigation.segment_clear(previous, *next),
            "unsafe segment {previous:?} -> {next:?}"
        );
        previous = *next;
    }
}
#[test]
fn direct_route_is_one_waypoint() {
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &[]);
    assert_eq!(
        nav.find_path(point(100.0, 100.0), point(1800.0, 1800.0))
            .unwrap(),
        vec![point(1800.0, 1800.0)]
    );
}
#[test]
fn circle_rectangle_and_polygon_routes_do_not_cut_corners() {
    for blocked in [
        obstacle(ObstacleShape::Circle {
            center: world(1000.0, 1000.0),
            radius: 300.0,
        }),
        rectangle(700.0, 650.0, 600.0, 700.0),
        obstacle(ObstacleShape::Polygon {
            points: vec![
                world(700.0, 600.0),
                world(1300.0, 600.0),
                world(1300.0, 1400.0),
                world(1000.0, 1100.0),
                world(700.0, 1400.0),
            ],
        }),
    ] {
        let obstacles = [blocked];
        let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
        let from = point(100.0, 1000.0);
        let goal = point(1900.0, 1000.0);
        assert!(!nav.segment_clear(from, goal));
        let path = nav.find_path(from, goal).unwrap();
        assert!(path.len() >= 2);
        assert!(path.len() <= 5);
        assert_clear(&nav, from, &path);
        assert_eq!(*path.last().unwrap(), goal);
    }
}
#[test]
fn concave_inverted_zone_preserves_its_playable_interior() {
    let mut zone = obstacle(ObstacleShape::Polygon {
        points: vec![
            world(0.0, 0.0),
            world(2000.0, 0.0),
            world(2000.0, 2000.0),
            world(1200.0, 2000.0),
            world(1200.0, 800.0),
            world(800.0, 800.0),
            world(800.0, 2000.0),
            world(0.0, 2000.0),
        ],
    });
    zone.inverted = true;
    let obstacles = [zone];
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
    let from = point(500.0, 1500.0);
    let goal = point(1500.0, 1500.0);
    assert!(!nav.segment_clear(from, goal));
    assert_clear(&nav, from, &nav.find_path(from, goal).unwrap());
    assert!(!nav.can_move(point(1000.0, 1500.0)));
}
#[test]
fn blocked_goal_projects_to_safe_approach() {
    let obstacles = [rectangle(900.0, 900.0, 200.0, 200.0)];
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
    let from = point(100.0, 1000.0);
    let path = nav.find_path(from, point(1000.0, 1000.0)).unwrap();
    assert_clear(&nav, from, &path);
    assert!(nav.can_move(*path.last().unwrap()));
    assert!((path.last().unwrap().x - 1000.0).hypot(path.last().unwrap().y - 1000.0) <= 200.0);
}
#[test]
fn separating_wall_fails_without_emitting_unsafe_segment() {
    let obstacles = [rectangle(900.0, 0.0, 200.0, 2000.0)];
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
    assert!(
        nav.find_path(point(100.0, 1000.0), point(1900.0, 1000.0))
            .is_err()
    );
}
#[test]
fn malformed_geometry_and_inside_obstacle_fail_closed() {
    for bad in [
        rectangle(100.0, 100.0, -100.0, 50.0),
        obstacle(ObstacleShape::Polygon { points: vec![] }),
        obstacle(ObstacleShape::Circle {
            center: world(1000.0, 1000.0),
            radius: -10.0,
        }),
    ] {
        let obstacles = [bad];
        let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
        assert!(
            nav.find_path(point(100.0, 1000.0), point(1900.0, 1000.0))
                .is_err()
        );
    }
    let obstacles = [rectangle(900.0, 900.0, 200.0, 200.0)];
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
    assert!(
        nav.find_path(point(1000.0, 1000.0), point(1900.0, 1000.0))
            .is_err()
    );
}
#[test]
fn hero_can_safely_leave_clearance_margin() {
    let obstacles = [rectangle(900.0, 900.0, 200.0, 200.0)];
    let nav = ObstacleNavigation::new(2000.0, 2000.0, &obstacles);
    let path = nav
        .find_path(point(850.0, 1000.0), point(100.0, 1000.0))
        .unwrap();
    assert!(nav.can_move(path[0]));
    assert_eq!(*path.last().unwrap(), point(100.0, 1000.0));
}
fn system() -> StarSystem {
    StarSystem {
        maps: (1..=5)
            .map(|id| MapDescriptor {
                id,
                name: format!("1-{id}"),
                min_level: if id == 4 { 10 } else { 0 },
            })
            .collect(),
        links: vec![
            MapLink {
                from: 1,
                to: 2,
                portal_id: 12,
            },
            MapLink {
                from: 2,
                to: 3,
                portal_id: 23,
            },
            MapLink {
                from: 1,
                to: 3,
                portal_id: 13,
            },
            MapLink {
                from: 3,
                to: 4,
                portal_id: 34,
            },
            MapLink {
                from: 1,
                to: 5,
                portal_id: 15,
            },
            MapLink {
                from: 5,
                to: 4,
                portal_id: 54,
            },
        ],
    }
}
fn snapshot() -> GameSnapshot {
    GameSnapshot {
        observed_at_unix_ms: 0,
        stage: StageInfo::default(),
        map: Some(GameMapInfo {
            id: Some(1),
            name: Some("1-1".into()),
            width: Some(2000.0),
            height: Some(2000.0),
            player_position: Some(world(100.0, 1000.0)),
            safe_places: vec![SafePlace {
                kind: "portal".into(),
                name: "gate".into(),
                x: 1900.0,
                y: 1000.0,
                radius: 250.0,
            }],
            in_safe_place: Some(false),
        }),
        entities: vec![GameEntity {
            path: "map/1".into(),
            class_name: "Ship".into(),
            name: "hero".into(),
            category: EntityCategory::Player,
            position: world(100.0, 1000.0),
            bounds: Bounds::default(),
            id: Some(json!(1)),
            health: Some(100.0),
            max_health: Some(100.0),
            shield: Some(100.0),
            max_shield: Some(100.0),
            fields: Default::default(),
        }],
        visited: 1,
        truncated: false,
        navigation: NavigationState {
            map_id: Some(1),
            geometry_ready: true,
            star_system: system(),
            portals: vec![PortalInfo {
                id: 13,
                type_id: 0,
                position: world(1900.0, 1000.0),
                target_map: Some(3),
                jumping: false,
            }],
            ..Default::default()
        },
    }
}
#[test]
fn shortest_routes_are_directed_level_aware_and_can_exclude_failed_portals() {
    let system = system();
    assert_eq!(
        shortest_map_route(&system, 1, 3, None, &BTreeSet::new())
            .unwrap()
            .len(),
        1
    );
    assert!(shortest_map_route(&system, 3, 1, None, &BTreeSet::new()).is_none());
    assert!(shortest_map_route(&system, 1, 4, Some(9), &BTreeSet::new()).is_none());
    assert_eq!(
        shortest_map_route(&system, 1, 4, Some(10), &BTreeSet::new())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        shortest_map_route(&system, 1, 3, None, &BTreeSet::from([(1, 13)]))
            .unwrap()
            .len(),
        2
    );
}
#[test]
fn traveler_settles_approaches_jumps_retries_and_arrives() {
    let mut traveler = MapTraveler::default();
    let mut snapshot = snapshot();
    assert!(traveler.tick(&snapshot, 3, 0).destination.is_none());
    let travel = traveler.tick(&snapshot, 3, 2000);
    assert!(travel.disable_pet);
    assert!(travel.destination.is_some());
    snapshot.map.as_mut().unwrap().player_position = Some(world(1850.0, 1000.0));
    assert_eq!(
        traveler.tick(&snapshot, 3, 2200).command,
        Some(GameCommand::JumpPortal { portal_id: 13 })
    );
    assert!(traveler.tick(&snapshot, 3, 2300).command.is_none());
    assert!(traveler.tick(&snapshot, 3, 4700).command.is_some());
    snapshot.map.as_mut().unwrap().id = Some(3);
    snapshot.navigation.map_id = Some(3);
    assert_eq!(
        traveler.tick(&snapshot, 3, 5000).state,
        TravelState::Traveling
    );
    assert_eq!(
        traveler.tick(&snapshot, 3, 7000).state,
        TravelState::Arrived
    );
}
#[test]
fn traveler_waits_for_geometry_and_recovers_from_obstructed_portal() {
    let mut traveler = MapTraveler::default();
    let mut snapshot = snapshot();
    traveler.tick(&snapshot, 3, 0);
    snapshot.navigation.geometry_ready = false;
    assert!(traveler.tick(&snapshot, 3, 2000).destination.is_none());
    snapshot.navigation.geometry_ready = true;
    snapshot.navigation.obstacles = vec![rectangle(900.0, 0.0, 200.0, 2000.0)];
    assert!(traveler.tick(&snapshot, 3, 2100).destination.is_none());
    // The direct gate is excluded and the next route waits for the map-2 gate.
    assert!(traveler.tick(&snapshot, 3, 2200).status.contains("appear"));
}
#[test]
fn missing_and_failed_portals_exhaust_routes_instead_of_hunting_wrong_map() {
    let mut traveler = MapTraveler::default();
    let mut snapshot = snapshot();
    snapshot.navigation.portals.clear();
    traveler.tick(&snapshot, 3, 0);
    traveler.tick(&snapshot, 3, 16000);
    traveler.tick(&snapshot, 3, 17000);
    assert!(matches!(
        traveler.tick(&snapshot, 3, 18000).state,
        TravelState::Failed(_)
    ));
    assert!(matches!(
        traveler.tick(&snapshot, 3, 19000).state,
        TravelState::Failed(_)
    ));
    // A new explicit target resets the previous terminal failure.
    assert!(!matches!(
        traveler.tick(&snapshot, 1, 20000).state,
        TravelState::Failed(_)
    ));
}
#[test]
fn route_follower_replans_when_geometry_changes_and_when_knocked_off_arrival() {
    let mut snapshot = snapshot();
    let from = point(100.0, 1000.0);
    let goal = point(1900.0, 1000.0);
    let mut router = MovementRouter::default();
    assert_eq!(router.next(&snapshot, from, goal, 0).unwrap(), Some(goal));
    snapshot.navigation.obstacles = vec![rectangle(700.0, 650.0, 600.0, 700.0)];
    let next = router.next(&snapshot, from, goal, 150).unwrap().unwrap();
    assert_ne!(next, goal);
    let nav = ObstacleNavigation::from_snapshot(&snapshot, CoordinateTransform::default());
    assert!(nav.segment_clear(from, next));
    snapshot.navigation.obstacles.clear();
    assert_eq!(router.next(&snapshot, goal, goal, 300).unwrap(), None);
    assert_eq!(router.next(&snapshot, from, goal, 450).unwrap(), Some(goal));
}
#[test]
fn stale_map_geometry_never_emits_movement() {
    let mut snapshot = snapshot();
    snapshot.navigation.map_id = Some(2);
    let nav = ObstacleNavigation::from_snapshot(&snapshot, CoordinateTransform::default());
    assert!(!nav.can_move(point(100.0, 1000.0)));
    assert!(
        nav.find_path(point(100.0, 1000.0), point(1900.0, 1000.0))
            .is_err()
    );
}
#[test]
fn repair_hysteresis_preserves_state_until_health_and_shield_recover() {
    let config = SafetyConfig::default();
    let mut safety = SafetyManager::default();
    let mut snapshot = snapshot();
    snapshot.entities[0].health = Some(30.0);
    let outcome = safety.tick(&snapshot, 0, &config, CoordinateTransform::default());
    assert!(!outcome.context.safe_to_work);
    assert!(outcome.context.safety_destination.is_some());
    snapshot.entities[0].health = Some(70.0);
    assert!(
        safety
            .tick(&snapshot, 1000, &config, CoordinateTransform::default())
            .repairing
    );
    snapshot.entities[0].health = Some(100.0);
    snapshot.entities[0].shield = Some(50.0);
    assert!(
        safety
            .tick(&snapshot, 2000, &config, CoordinateTransform::default())
            .repairing
    );
    snapshot.entities[0].shield = Some(100.0);
    assert!(
        safety
            .tick(&snapshot, 3000, &config, CoordinateTransform::default())
            .context
            .safe_to_work
    );
}
#[test]
fn death_recovery_waits_for_offered_free_repair_and_counts_each_death_once() {
    let config = SafetyConfig::default();
    let mut safety = SafetyManager::default();
    let mut snapshot = snapshot();
    snapshot.navigation.hero.killed = true;
    snapshot.entities.clear();
    snapshot.map = None;
    assert!(
        safety
            .tick(&snapshot, 0, &config, CoordinateTransform::default())
            .commands
            .is_empty()
    );
    assert!(
        safety
            .tick(&snapshot, 5000, &config, CoordinateTransform::default())
            .commands
            .is_empty()
    );
    snapshot.navigation.hero.free_repair_type = Some(1);
    assert_eq!(
        safety
            .tick(&snapshot, 6000, &config, CoordinateTransform::default())
            .commands,
        vec![GameCommand::ReviveAt {
            repair_type: 1,
            location: xenomit_game_api::ReviveLocation::Base,
        }]
    );
    assert!(
        safety
            .tick(&snapshot, 6500, &config, CoordinateTransform::default())
            .commands
            .is_empty()
    );
    assert_eq!(safety.deaths(), 1);
    snapshot.navigation.hero.killed = false;
    safety.tick(&snapshot, 7000, &config, CoordinateTransform::default());
    snapshot.navigation.hero.killed = true;
    safety.tick(&snapshot, 8000, &config, CoordinateTransform::default());
    assert_eq!(safety.deaths(), 2);
}
#[test]
fn safety_limits_deaths_repair_time_and_configuration_values() {
    let mut config = SafetyConfig {
        max_deaths: 1,
        ..Default::default()
    };
    let mut snapshot = snapshot();
    snapshot.navigation.hero.killed = true;
    assert!(
        SafetyManager::default()
            .tick(&snapshot, 0, &config, CoordinateTransform::default())
            .failure
            .is_some()
    );
    config.max_deaths = 10;
    snapshot.navigation.hero.killed = false;
    snapshot.entities[0].health = Some(30.0);
    let mut safety = SafetyManager::default();
    safety.tick(&snapshot, 0, &config, CoordinateTransform::default());
    assert!(
        safety
            .tick(&snapshot, 300001, &config, CoordinateTransform::default())
            .failure
            .is_some()
    );
    config.min_health_percent = 96.0;
    assert!(config.validate().is_err());
}
#[test]
fn builtins_resolve_legacy_maps_and_resume_same_module_after_travel() {
    let token = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(ServiceRegistry::default(), token.clone());
    let mut runner = builtin_registry()
        .create(
            xenomit_game_api::ModuleConfig {
                module_id: "collect".into(),
                settings: json!({"working_map":"1-3","safety":{"enabled":false}}),
            },
            token,
        )
        .unwrap();
    let mut snapshot = snapshot();
    assert!(
        runner
            .tick(0, Some(&snapshot), true, &mut host)
            .plan
            .unwrap()
            .suspended
    );
    let travel = runner
        .tick(2000, Some(&snapshot), true, &mut host)
        .plan
        .unwrap();
    assert!(travel.controls.travel_to_map.is_some());
    assert!(
        travel
            .actions
            .iter()
            .any(|a| matches!(a, ActionIntent::MoveTo { .. }))
    );
    snapshot.map.as_mut().unwrap().id = Some(3);
    snapshot.navigation.map_id = Some(3);
    assert!(
        runner
            .tick(2200, Some(&snapshot), true, &mut host)
            .plan
            .unwrap()
            .suspended
    );
    let resumed = runner
        .tick(4200, Some(&snapshot), true, &mut host)
        .plan
        .unwrap();
    assert!(!resumed.suspended);
    assert_eq!(resumed.mode, BehaviorMode::Collector);
}
#[test]
fn builtin_death_commands_are_emitted_even_without_player_entity() {
    let token = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(ServiceRegistry::default(), token.clone());
    let mut runner = builtin_registry()
        .create(
            xenomit_game_api::ModuleConfig {
                module_id: "collect".into(),
                settings: json!({}),
            },
            token,
        )
        .unwrap();
    let mut snapshot = snapshot();
    snapshot.navigation.hero.killed = true;
    snapshot.navigation.hero.free_repair_type = Some(1);
    snapshot.entities.clear();
    runner.tick(0, Some(&snapshot), true, &mut host);
    let plan = runner
        .tick(5000, Some(&snapshot), true, &mut host)
        .plan
        .unwrap();
    assert!(plan.player.is_none());
    assert_eq!(
        plan.controls.commands,
        vec![GameCommand::ReviveAt {
            repair_type: 1,
            location: xenomit_game_api::ReviveLocation::Base,
        }]
    );
}

fn combat_snapshot() -> GameSnapshot {
    let mut frame = snapshot();
    let mut npc = frame.entities[0].clone();
    npc.id = Some(json!(2));
    npc.path = "map/2".into();
    npc.name = "Streuner".into();
    npc.category = EntityCategory::Npc;
    npc.position = world(600.0, 1000.0);
    npc.shield = Some(5000.0);
    npc.fields.insert("npcTypeId".into(), json!(71));
    frame.entities.push(npc);
    frame.navigation.hero.pet_active = true;
    frame.navigation.hero.owns_pet = true;
    frame.navigation.hero.pet_gears = vec![6];
    for id in [
        "ammunition_laser_lcb-10",
        "ammunition_laser_sab-50",
        "ammunition_laser_rsb-75",
    ] {
        frame.navigation.hero.items.insert(
            id.into(),
            xenomit_game_api::ItemState {
                quantity: Some(100.0),
                available: true,
                activatable: id.contains("rsb"),
                selected: id.contains("sab"),
                blocked: false,
            },
        );
    }
    frame
}

#[test]
fn combat_flags_apply_available_ammo_and_locator_with_individual_command_acknowledgement() {
    let token = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(ServiceRegistry::default(), token.clone());
    let mut runner = builtin_registry().create(xenomit_game_api::ModuleConfig { module_id:"npc_killer".into(),settings:json!({"safety":{"enabled":false},"npc_flags":["no_sab","use_rsb","pet_locator"],"laser_ammo":"ammunition_laser_sab-50"}) },token).unwrap();
    let frame = combat_snapshot();
    let plan = runner.tick(0, Some(&frame), true, &mut host).plan.unwrap();
    assert!(plan.selected_npc.is_some());
    assert!(plan.controls.npc_flags.contains(&NpcFlag::NoSab));
    assert!(
        plan.controls
            .commands
            .contains(&GameCommand::SelectPetGear {
                gear_type: 6,
                target: 71
            })
    );
    assert!(plan.controls.commands.contains(&GameCommand::UseItem {
        item_id: "ammunition_laser_lcb-10".into(),
        activate: false
    }));
    assert!(
        !plan
            .controls
            .commands
            .iter()
            .any(|c| matches!(c,GameCommand::UseItem { item_id,.. } if item_id.contains("sab")))
    );
    let rsb = GameCommand::UseItem {
        item_id: "ammunition_laser_rsb-75".into(),
        activate: true,
    };
    assert!(plan.controls.commands.contains(&rsb));
    // Earlier commands succeeded, but a later movement failed. Cooldowns and
    // acknowledged gear remain retained rather than replaying the whole plan.
    runner.command_succeeded(&rsb, 0);
    runner.command_succeeded(
        &GameCommand::SelectPetGear {
            gear_type: 6,
            target: 71,
        },
        0,
    );
    runner.plan_failed(&plan);
    let later = runner
        .tick(1000, Some(&frame), true, &mut host)
        .plan
        .unwrap();
    assert!(!later.controls.commands.contains(&rsb));
    assert!(
        runner
            .tick(5000, Some(&frame), true, &mut host)
            .plan
            .unwrap()
            .controls
            .commands
            .contains(&rsb)
    );
}

#[test]
fn sab_selection_recovers_to_regular_ammo_when_shields_recover() {
    let token = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(ServiceRegistry::default(), token.clone());
    let mut runner = builtin_registry()
        .create(
            xenomit_game_api::ModuleConfig {
                module_id: "npc_killer".into(),
                settings: json!({"safety":{"enabled":false},"sab_enabled":true}),
            },
            token,
        )
        .unwrap();
    let mut frame = combat_snapshot();
    frame.entities[0].shield = Some(20.0);
    frame
        .navigation
        .hero
        .items
        .get_mut("ammunition_laser_sab-50")
        .unwrap()
        .selected = false;
    let sab = GameCommand::UseItem {
        item_id: "ammunition_laser_sab-50".into(),
        activate: false,
    };
    assert!(
        runner
            .tick(0, Some(&frame), true, &mut host)
            .plan
            .unwrap()
            .controls
            .commands
            .contains(&sab)
    );
    runner.command_succeeded(&sab, 0);
    frame.entities[0].shield = Some(100.0);
    frame
        .navigation
        .hero
        .items
        .get_mut("ammunition_laser_sab-50")
        .unwrap()
        .selected = true;
    assert!(
        runner
            .tick(150, Some(&frame), true, &mut host)
            .plan
            .unwrap()
            .controls
            .commands
            .contains(&GameCommand::UseItem {
                item_id: "ammunition_laser_lcb-10".into(),
                activate: false
            })
    );
}

#[test]
fn builtin_returns_world_coordinates_with_non_identity_planner_transform() {
    let token = CancellationToken::default();
    let mut host = ThreadedOperationHost::new(ServiceRegistry::default(), token.clone());
    let mut runner = builtin_registry().create(xenomit_game_api::ModuleConfig { module_id:"npc_killer".into(),settings:json!({"safety":{"enabled":false},"behavior_config":{"mode":"npc_killer","coordinates":{"xx":2.0,"yy":2.0,"translate_x":17.0,"translate_y":23.0},"npc_killer":{"npcs":[{"selector":{"category":"npc"}}]}}}) },token).unwrap();
    let frame = combat_snapshot();
    let plan = runner.tick(0, Some(&frame), true, &mut host).plan.unwrap();
    assert_eq!(plan.player.unwrap().position, point(100.0, 1000.0));
    assert_eq!(plan.selected_npc.unwrap().position, point(600.0, 1000.0));
    for action in plan.actions {
        if let ActionIntent::AttackNpc { target } = action {
            assert_eq!(target.position, point(600.0, 1000.0));
        }
    }
}

#[test]
fn full_size_map_routes_around_polygon_without_unsafe_segments() {
    let obstacles = [obstacle(ObstacleShape::Polygon {
        points: vec![
            world(8000.0, 4500.0),
            world(13000.0, 4500.0),
            world(13000.0, 8500.0),
            world(8000.0, 8500.0),
        ],
    })];
    let nav = ObstacleNavigation::new(21000.0, 13100.0, &obstacles);
    let from = point(1000.0, 6500.0);
    let goal = point(20000.0, 6500.0);
    let path = nav.find_path(from, goal).unwrap();
    assert_clear(&nav, from, &path);
    assert_eq!(path.last(), Some(&goal));
}

#[test]
fn enemy_retreat_memory_expires_and_does_not_treat_friendly_entities_as_threats() {
    let mut frame = combat_snapshot();
    frame.entities[1]
        .fields
        .insert("isEnemy".into(), json!(true));
    frame.entities[1]
        .fields
        .insert("attackingHero".into(), json!(true));
    let mut safety = SafetyManager::default();
    let config = SafetyConfig {
        stop_running_no_sight: false,
        ..SafetyConfig::default()
    };
    assert!(
        !safety
            .tick(&frame, 0, &config, CoordinateTransform::default())
            .context
            .safe_to_work
    );
    frame.entities[1]
        .fields
        .insert("attackingHero".into(), json!(false));
    assert!(
        !safety
            .tick(&frame, 14000, &config, CoordinateTransform::default())
            .context
            .safe_to_work
    );
    assert!(
        safety
            .tick(&frame, 15000, &config, CoordinateTransform::default())
            .context
            .safe_to_work
    );
    frame.entities[1]
        .fields
        .insert("attackingHero".into(), json!(true));
    frame.entities[1]
        .fields
        .insert("isEnemy".into(), json!(false));
    assert!(
        safety
            .tick(&frame, 16000, &config, CoordinateTransform::default())
            .context
            .safe_to_work
    );
}
