use serde_json::{Value, json};
use xenomit_behavior_engine::*;

fn tick(rules: &mut BehaviorRunner, now_ms: u64, stopped: bool) -> RuleOutput {
    rules.tick(&RuleContext {
        now_ms,
        snapshot: None,
        module_settings: &Value::Null,
        stopped,
    })
}
#[test]
fn hourly_default_becomes_pending_without_taking_over_or_resetting_deadline() {
    let registry = builtin_behavior_registry().filtered(|id| id == "refresh");
    let mut rules = registry.create(&json!({})).unwrap();
    assert!(tick(&mut rules, 0, false).host_actions.is_empty());
    assert!(tick(&mut rules, 3_599_999, false).host_actions.is_empty());
    let pending = tick(&mut rules, 3_600_000, false);
    assert_eq!(pending.host_actions, vec![HostAction::RequestRefresh]);
    assert!(pending.takeover.is_none());
    assert!(!pending.stop);
    assert!(pending.commands.is_empty());
    assert_eq!(
        tick(&mut rules, 7_200_000, false).host_actions,
        vec![HostAction::RequestRefresh]
    );
}
#[test]
fn interval_is_configurable_and_disabled_or_stopped_rules_do_not_refresh() {
    let registry = builtin_behavior_registry().filtered(|id| id == "refresh");
    let mut rules = registry
        .create(&json!({"refresh":{"interval_minutes":1}}))
        .unwrap();
    tick(&mut rules, 0, false);
    assert_eq!(
        tick(&mut rules, 60_000, false).host_actions,
        vec![HostAction::RequestRefresh]
    );
    assert!(tick(&mut rules, 120_000, true).host_actions.is_empty());
    rules
        .update(&json!({"refresh":{"enabled":false,"interval_minutes":1}}))
        .unwrap();
    assert!(tick(&mut rules, 180_000, false).host_actions.is_empty());
    rules
        .update(&json!({"refresh":{"enabled":true,"interval_minutes":1}}))
        .unwrap();
    assert!(tick(&mut rules, 180_000, false).host_actions.is_empty());
    assert_eq!(
        tick(&mut rules, 240_000, false).host_actions,
        vec![HostAction::RequestRefresh]
    );
}
#[test]
fn invalid_interval_is_rejected_and_base_owns_refresh() {
    let catalog = PluginCatalog::builtin();
    assert!(
        catalog
            .descriptors()
            .iter()
            .find(|p| p.id == "base")
            .unwrap()
            .behavior_ids
            .contains(&"refresh".to_owned())
    );
    let registry = builtin_behavior_registry().filtered(|id| id == "refresh");
    assert!(
        registry
            .create(&json!({"refresh":{"interval_minutes":0}}))
            .is_err()
    );
    assert!(
        registry
            .create(&json!({"refresh":{"interval_minutes":1.5}}))
            .is_err()
    );
    let (_, rules) = catalog
        .registries(&PluginSelection::from([("base".into(), false)]))
        .unwrap();
    assert!(!rules.descriptors().iter().any(|d| d.id == "refresh"));
}
