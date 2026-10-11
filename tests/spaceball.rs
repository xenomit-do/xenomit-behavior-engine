use serde_json::json;
use xenomit_behavior_engine::*;
use xenomit_game_api::{
    EntityCategory, GameEntity, GameMapInfo, GameSnapshot, ModuleConfig, NavigationState,
    Point as ApiPoint, StageInfo,
};

struct NoOperations;
impl OperationHost for NoOperations {
    fn start(&mut self, _: OperationRequest) -> Result<OperationHandle, String> {
        panic!("SpaceBall should emit typed intents")
    }
    fn poll(&mut self, _: OperationHandle) -> OperationPoll {
        panic!("No operations")
    }
    fn cancel(&mut self, _: OperationHandle) {}
    fn release(&mut self, _: OperationHandle) {}
}
fn entity(id: i32, name: &str, category: EntityCategory, x: f64) -> GameEntity {
    GameEntity {
        path: format!("entity/{id}"),
        class_name: name.into(),
        name: name.into(),
        category,
        position: ApiPoint { x, y: 1000.0 },
        bounds: Default::default(),
        id: Some(json!(id)),
        health: Some(100.0),
        max_health: Some(100.0),
        shield: Some(100.0),
        max_shield: Some(100.0),
        fields: Default::default(),
    }
}
fn frame() -> GameSnapshot {
    GameSnapshot {
        navigation: NavigationState {
            map_id: Some(16),
            geometry_ready: true,
            ..Default::default()
        },
        observed_at_unix_ms: 1,
        stage: StageInfo::default(),
        map: Some(GameMapInfo {
            id: Some(16),
            name: Some("4-4".into()),
            width: Some(20000.0),
            height: Some(15000.0),
            player_position: Some(ApiPoint {
                x: 1000.0,
                y: 1000.0,
            }),
            safe_places: vec![],
            in_safe_place: Some(false),
        }),
        entities: vec![
            entity(1, "hero", EntityCategory::Player, 1000.0),
            entity(2, "SpaceBall", EntityCategory::Npc, 2000.0),
            entity(3, "Streuner", EntityCategory::Npc, 1100.0),
        ],
        visited: 3,
        truncated: false,
    }
}
fn runner(settings: serde_json::Value) -> ModuleRunner {
    PluginCatalog::builtin()
        .registries(&Default::default())
        .unwrap()
        .0
        .create(
            ModuleConfig {
                module_id: "shared_spaceball".into(),
                settings,
            },
            CancellationToken::default(),
        )
        .unwrap()
}
#[test]
fn attacks_only_ball_and_follow_never_attacks_other_npcs() {
    let s = frame();
    let mut attack = runner(json!({}));
    let plan = attack
        .tick(0, Some(&s), true, &mut NoOperations)
        .plan
        .unwrap();
    assert_eq!(plan.selected_npc.unwrap().name, "SpaceBall");
    let mut follow = runner(json!({"mode":"follow"}));
    let plan = follow
        .tick(0, Some(&s), true, &mut NoOperations)
        .plan
        .unwrap();
    assert!(plan.selected_npc.is_none());
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, ActionIntent::MoveTo { .. }))
    );
    assert!(
        !plan
            .actions
            .iter()
            .any(|a| matches!(a, ActionIntent::AttackNpc { .. }))
    );
}
#[test]
fn loses_ball_collects_only_allowed_cargo_then_resumes_ball_after_delay() {
    let mut r = runner(json!({"target_delay_seconds":1}));
    let mut s = frame();
    r.tick(0, Some(&s), true, &mut NoOperations);
    s.entities.retain(|e| e.name != "SpaceBall");
    s.entities
        .push(entity(4, "FROM_SHIP", EntityCategory::Unknown, 1100.0));
    s.entities
        .push(entity(5, "BONUS_BOX", EntityCategory::Unknown, 1001.0));
    let plan = r.tick(10, Some(&s), true, &mut NoOperations).plan.unwrap();
    assert_eq!(plan.selected_box.unwrap().name, "FROM_SHIP");
    s.entities.retain(|e| e.category != EntityCategory::Unknown);
    s.entities
        .push(entity(2, "SpaceBall", EntityCategory::Npc, 2000.0));
    let plan = r.tick(20, Some(&s), true, &mut NoOperations).plan.unwrap();
    assert!(plan.selected_npc.is_none());
    let plan = r
        .tick(2010, Some(&s), true, &mut NoOperations)
        .plan
        .unwrap();
    assert_eq!(plan.selected_npc.unwrap().name, "SpaceBall");
}
#[test]
fn unknown_map_waits_and_disabled_event_bundle_has_no_ball_factory() {
    let mut r = runner(json!({}));
    let mut s = frame();
    s.map = None;
    s.navigation.map_id = None;
    let plan = r.tick(0, Some(&s), true, &mut NoOperations).plan.unwrap();
    assert!(plan.suspended);
    assert!(plan.actions.is_empty());
    let selection = PluginSelection::from([("event".into(), false)]);
    assert!(
        !PluginCatalog::builtin()
            .module_enabled("shared_spaceball", &selection)
            .unwrap()
    );
}
