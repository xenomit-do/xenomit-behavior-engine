use serde_json::json;
use xenomit_behavior_engine::*;
use xenomit_game_api::{
    Bounds, EntityCategory, GalaxyGateInfo, GameCommand, GameEntity, GameMapInfo, GameSnapshot,
    MapDescriptor, Point as ApiPoint, StageInfo,
};

struct Host;
impl OperationHost for Host {
    fn start(&mut self, _: OperationRequest) -> Result<OperationHandle, String> {
        Ok(OperationHandle(1))
    }
    fn poll(&mut self, _: OperationHandle) -> OperationPoll {
        OperationPoll::Pending
    }
    fn cancel(&mut self, _: OperationHandle) {}
    fn release(&mut self, _: OperationHandle) {}
}

fn snapshot(map_id: i32, map_name: &str) -> GameSnapshot {
    GameSnapshot {
        navigation: xenomit_game_api::NavigationState {
            map_id: Some(map_id),
            hero: xenomit_game_api::HeroState {
                faction: Some(1),
                ..Default::default()
            },
            star_system: xenomit_game_api::StarSystem {
                maps: vec![
                    MapDescriptor {
                        id: 1,
                        name: "1-1".into(),
                        min_level: 1,
                    },
                    MapDescriptor {
                        id: 8,
                        name: "1-8".into(),
                        min_level: 1,
                    },
                    MapDescriptor {
                        id: 51,
                        name: "Alpha".into(),
                        min_level: 1,
                    },
                ],
                links: vec![],
            },
            galaxy_gates: vec![GalaxyGateInfo {
                id: 1,
                parts: 2,
                total_parts: 34,
                lives: 3,
                multiplier: 6,
                ..Default::default()
            }],
            ..Default::default()
        },
        observed_at_unix_ms: 0,
        stage: StageInfo::default(),
        map: Some(GameMapInfo {
            id: Some(map_id),
            name: Some(map_name.into()),
            width: Some(20000.0),
            height: Some(12000.0),
            player_position: Some(ApiPoint { x: 100.0, y: 100.0 }),
            safe_places: vec![],
            in_safe_place: Some(false),
        }),
        entities: vec![GameEntity {
            path: "hero".into(),
            class_name: "Ship".into(),
            name: "hero".into(),
            category: EntityCategory::Player,
            position: ApiPoint { x: 100.0, y: 100.0 },
            bounds: Bounds::default(),
            id: Some(json!(1)),
            health: Some(100.0),
            max_health: Some(100.0),
            shield: None,
            max_shield: None,
            fields: Default::default(),
        }],
        visited: 1,
        truncated: false,
    }
}

#[test]
fn galaxy_spins_on_the_home_map() {
    let registry = builtin_registry();
    let mut runner = registry
        .create(
            xenomit_game_api::ModuleConfig {
                module_id: "galaxy".into(),
                settings: json!({}),
            },
            CancellationToken::default(),
        )
        .unwrap();
    let frame = snapshot(1, "1-1");
    let mut host = Host;
    let output = runner.tick(1_000, Some(&frame), true, &mut host);
    assert!(matches!(output.state, RunState::Running));
    let plan = output.plan.expect("spin plan");
    assert!(plan.controls.commands.iter().any(|command| matches!(
        command,
        GameCommand::SpinGalaxyGate {
            gate_id: 1,
            spin_amount: 1,
            use_multiplier: true
        }
    )));
}

#[test]
fn skylab_restocks_a_collector_before_upgrading() {
    let rows = skylab_rows(
        r#"'prometiumCollector'.;"">level skylab_font_level">4</span> prometiumCollector_skylabPendingRobots>0< href="indexInternal.es?subaction=upgrade&construction=prometiumCollector""#,
    );
    let collector = rows
        .iter()
        .find(|row| row.name == "prometiumCollector")
        .unwrap();
    assert_eq!(collector.level, 4);
    assert_eq!(collector.pending_robots, 0);
    assert!(collector.upgrade_path.is_some());
    let action = next_skylab_action(&rows, &json!({"prometiumCollector": true}), false).unwrap();
    assert_eq!(
        action,
        SkylabAction::BuyRobots {
            construction: "prometiumCollector".into(),
            uri: false
        }
    );
}

#[test]
fn registry_lists_the_built_in_game_modes() {
    let ids: Vec<_> = builtin_registry()
        .descriptors()
        .into_iter()
        .map(|d| d.id)
        .collect();
    for id in [
        "galaxy",
        "gop",
        "low",
        "hades",
        "eternal_blacklight",
        "follow",
        "chaos_spawner",
        "skylab",
        "blacklight_farmer",
        "treacherous_domain",
    ] {
        assert!(ids.iter().any(|existing| existing == id), "{id}");
    }
}
