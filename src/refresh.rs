//! Periodic refresh requests never take over the objective or force a restart.
use crate::{BehaviorRegistry, GeneralBehavior, HostAction, RuleContext, RuleOutput};
use serde_json::{Value, json};
use xenomit_game_api::{ModuleDescriptor, SettingDescriptor, SettingKind};

pub(crate) fn register(registry: &mut BehaviorRegistry) {
    let descriptor = ModuleDescriptor::new("refresh", "Periodic client refresh")
        .description("Refresh at a safe module boundary. An overdue refresh waits for the current gate or operation to finish.")
        .setting(SettingDescriptor::new("enabled", "Enabled", SettingKind::Boolean, json!(true)))
        .setting(SettingDescriptor::new("interval_minutes", "Refresh interval (minutes)", SettingKind::Integer { min:1, max:10080 }, json!(60)));
    // Requests are observed even when a high-priority safety rule has a takeover.
    registry
        .register(-1, descriptor, |settings| {
            validate(settings)?;
            Ok(Box::new(PeriodicRefresh {
                settings: settings.clone(),
                epoch: None,
            }))
        })
        .expect("valid periodic refresh schema");
}
fn validate(settings: &Value) -> Result<(), String> {
    if !settings["interval_minutes"]
        .as_u64()
        .is_some_and(|m| (1..=10080).contains(&m))
    {
        return Err("Refresh interval must be 1..10080 whole minutes".into());
    }
    Ok(())
}
struct PeriodicRefresh {
    settings: Value,
    epoch: Option<u64>,
}
impl GeneralBehavior for PeriodicRefresh {
    fn update(&mut self, settings: &Value) -> Result<(), String> {
        validate(settings)?;
        if self.settings["enabled"] != settings["enabled"] {
            self.epoch = None;
        }
        self.settings = settings.clone();
        Ok(())
    }
    fn validate_settings(&self, settings: &Value) -> Result<(), String> {
        validate(settings)
    }
    fn export_state(&self, now_ms: u64) -> Option<Value> {
        Some(json!({
            "elapsed_ms": self.epoch.map(|epoch| now_ms.saturating_sub(epoch)),
        }))
    }
    fn restore_state(&mut self, state: &Value, now_ms: u64) -> Result<(), String> {
        self.epoch = state
            .get("elapsed_ms")
            .and_then(Value::as_u64)
            .map(|elapsed| now_ms.saturating_sub(elapsed));
        Ok(())
    }
    fn tick(&mut self, context: &RuleContext<'_>) -> RuleOutput {
        let mut output = RuleOutput::default();
        if self.settings["enabled"].as_bool() != Some(true) || context.stopped {
            return output;
        }
        let since = *self.epoch.get_or_insert(context.now_ms);
        let interval = self.settings["interval_minutes"].as_u64().unwrap_or(60) * 60000;
        if context.now_ms.saturating_sub(since) >= interval {
            output.host_actions.push(HostAction::RequestRefresh);
        }
        output
    }
}
