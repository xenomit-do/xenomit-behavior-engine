//! Background-compatible item restocking policy executed through a host service.
use crate::{
    BehaviorModule, InterruptionContext, InterruptionReason, InterruptionStatus, ModuleOutput,
    ModuleRegistryBuilder, OperationHandle, OperationPoll, OperationRequest, TickContext,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xenomit_game_api::{ModuleDescriptor, SettingDescriptor, SettingKind};

const SERVICE: &str = "backpage.shop_purchase";

pub fn register(builder: &mut ModuleRegistryBuilder) {
    let descriptor = ModuleDescriptor {
        id: "item_restock".into(),
        name: "Item restock".into(),
        description: "Restock configured shop items when observed inventory reaches its threshold.".into(),
        settings: vec![
            SettingDescriptor::new(
                "tick_ms",
                "Tick interval (ms)",
                SettingKind::Integer { min: 250, max: 60_000 },
                json!(1000),
            )
            .group("Scheduling"),
            SettingDescriptor::new(
                "items",
                "Restock rules",
                SettingKind::Json,
                json!([]),
            )
            .group("Items")
            .description("Each enabled row needs item_id, category, threshold, amount, and interval_minutes. A threshold of -1 purchases the configured amount every interval."),
            SettingDescriptor::new(
                "buy_when_unobserved",
                "Buy when inventory is unknown",
                SettingKind::Boolean,
                json!(false),
            )
            .group("Safety"),
        ],
        required_services: vec![SERVICE.into()],
    };
    builder
        .register(descriptor, |settings| {
            Ok(Box::new(ItemRestockModule::new(settings)?))
        })
        .expect("item restock descriptor is valid");
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct RestockRule {
    item_id: String,
    category: String,
    enabled: bool,
    threshold: f64,
    amount: u64,
    interval_minutes: u64,
}

impl Default for RestockRule {
    fn default() -> Self {
        Self {
            item_id: String::new(),
            category: String::new(),
            enabled: true,
            threshold: 0.0,
            amount: 0,
            interval_minutes: 15,
        }
    }
}

struct PendingPurchase {
    handle: OperationHandle,
    item_id: String,
    interval_ms: u64,
}

struct ItemRestockModule {
    rules: Vec<RestockRule>,
    buy_when_unobserved: bool,
    pending: Option<PendingPurchase>,
    next_check: BTreeMap<String, u64>,
}

impl ItemRestockModule {
    fn new(settings: &Value) -> Result<Self, String> {
        Ok(Self {
            rules: parse_rules(settings)?,
            buy_when_unobserved: settings
                .get("buy_when_unobserved")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            pending: None,
            next_check: BTreeMap::new(),
        })
    }
}

impl BehaviorModule for ItemRestockModule {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        if self.pending.is_some() {
            return InterruptionStatus::Deferred("Waiting for the current shop purchase".into());
        }
        crate::interruption::safe_idle_status(context)
    }
    /// An in-flight purchase is an account operation: it pauses the drain
    /// countdown and never survives a checkpoint.
    fn non_interruptible(&self) -> bool {
        self.pending.is_some()
    }
    fn export_state(&self, now_ms: u64, _best_effort: bool) -> Option<crate::ModuleState> {
        let mut issues = Vec::new();
        if self.pending.is_some() {
            issues.push("in-flight purchase discarded; the rule re-checks on restore".into());
        }
        let data = serde_json::json!({
            "next_check_in_ms": self
                .next_check
                .iter()
                .map(|(item, at)| (item.clone(), at.saturating_sub(now_ms)))
                .collect::<BTreeMap<String, u64>>(),
        });
        let mut state = crate::ModuleState::clean(String::new(), data);
        state.issues = issues;
        Some(state)
    }
    fn restore_state(
        &mut self,
        state: &crate::ModuleState,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        state.check_schema()?;
        let next_check: BTreeMap<String, u64> = state
            .data
            .get("next_check_in_ms")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|error| {
                crate::RestoreError::Corrupt(format!("restock checkpoint: {error}"))
            })?
            .unwrap_or_default();
        self.next_check = next_check
            .into_iter()
            .map(|(item, remaining)| (item, now_ms.saturating_add(remaining)))
            .collect();
        self.pending = None;
        Ok(())
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        if let Some(pending) = &self.pending {
            match context.operations.poll(pending.handle) {
                OperationPoll::Pending => {
                    return ModuleOutput::running(format!("Purchasing {}", pending.item_id));
                }
                OperationPoll::Succeeded(result) => {
                    let pending = self.pending.take().unwrap();
                    context.operations.release(pending.handle);
                    self.next_check.insert(
                        pending.item_id.clone(),
                        context.now_ms.saturating_add(pending.interval_ms),
                    );
                    let detail = result
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("purchase complete");
                    return ModuleOutput::running(format!("{}: {detail}", pending.item_id));
                }
                OperationPoll::Failed(error) => {
                    let pending = self.pending.take().unwrap();
                    context.operations.release(pending.handle);
                    self.next_check.insert(
                        pending.item_id.clone(),
                        context.now_ms.saturating_add(30_000),
                    );
                    return ModuleOutput::running(format!(
                        "{}: {error}; retrying later",
                        pending.item_id
                    ));
                }
            }
        }

        let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
            return ModuleOutput::running("Waiting for the game bridge");
        };
        if self.rules.is_empty() {
            return ModuleOutput::running("No restock rules configured");
        }
        for rule in self.rules.iter().filter(|rule| rule.enabled) {
            if context.now_ms < self.next_check.get(&rule.item_id).copied().unwrap_or(0) {
                continue;
            }
            let quantity = snapshot
                .navigation
                .hero
                .items
                .get(&rule.item_id)
                .and_then(|item| item.quantity)
                .filter(|quantity| quantity.is_finite() && *quantity >= 0.0);
            let below_threshold = if rule.threshold < 0.0 {
                true
            } else if let Some(quantity) = quantity {
                quantity <= rule.threshold
            } else {
                self.buy_when_unobserved
            };
            let interval_ms = rule.interval_minutes.saturating_mul(60_000);
            if !below_threshold {
                self.next_check.insert(
                    rule.item_id.clone(),
                    context.now_ms.saturating_add(interval_ms),
                );
                continue;
            }
            let request = OperationRequest::service(
                SERVICE,
                json!({
                    "category": rule.category,
                    "item_id": rule.item_id,
                    "amount": rule.amount,
                }),
            );
            return match context.operations.start(request) {
                Ok(handle) => {
                    self.pending = Some(PendingPurchase {
                        handle,
                        item_id: rule.item_id.clone(),
                        interval_ms,
                    });
                    ModuleOutput::running(format!("Purchasing {}", rule.item_id))
                }
                Err(error) => {
                    self.next_check
                        .insert(rule.item_id.clone(), context.now_ms.saturating_add(30_000));
                    ModuleOutput::running(format!("Shop service unavailable: {error}"))
                }
            };
        }
        ModuleOutput::running("Restock thresholds are satisfied")
    }

    fn update_settings(&mut self, settings: &Value) -> Result<(), String> {
        self.rules = parse_rules(settings)?;
        self.buy_when_unobserved = settings
            .get("buy_when_unobserved")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.next_check.clear();
        Ok(())
    }
}

fn parse_rules(settings: &Value) -> Result<Vec<RestockRule>, String> {
    let rules: Vec<RestockRule> =
        serde_json::from_value(settings.get("items").cloned().unwrap_or_else(|| json!([])))
            .map_err(|error| format!("Invalid restock rules: {error}"))?;
    for rule in &rules {
        if rule.item_id.trim().is_empty() || rule.category.trim().is_empty() {
            return Err("Restock item_id and category must not be empty".into());
        }
        if rule.amount == 0 || rule.amount > 10_000_000 {
            return Err("Restock amount must be between 1 and 10,000,000".into());
        }
        if !rule.threshold.is_finite() || rule.threshold < -1.0 {
            return Err("Restock threshold must be -1 or greater".into());
        }
        if rule.interval_minutes == 0 || rule.interval_minutes > 1440 {
            return Err("Restock interval must be between 1 and 1440 minutes".into());
        }
    }
    Ok(rules)
}
