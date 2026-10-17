//! General rules run alongside (and may temporarily suspend) the objective module.
//! All decisions use world coordinates. Unknown manager facts never imply permission.
use crate::{
    ActionIntent, BehaviorDecision, BehaviorMode, ControlRequests, EntityRef, MapTraveler, Point,
    ShipMode, TravelState,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
};
use xenomit_game_api::{
    EntityCategory, GameCommand, GameEntity, GameSnapshot, ModuleDescriptor, SettingDescriptor,
    SettingKind,
};

#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    Refresh,
    /// Reload after an objective completed; never replay its completed workflow.
    RefreshIdle,
    /// Cooperative refresh: the runtime must wait for module interruption permission.
    RequestRefresh,
    Disconnect,
    Close,
    Profile(String),
    AcceptUrgentQuest,
    ClaimCalendar,
    SelectTarget(i32),
    TagPlayer {
        id: i32,
        tag: String,
    },
}

#[derive(Default)]
pub struct RuleOutput {
    /// Highest priority takeover wins; module ticks must be skipped while set.
    pub takeover: Option<BehaviorDecision>,
    pub commands: Vec<GameCommand>,
    pub host_actions: Vec<HostAction>,
    pub status: Vec<String>,
    pub stop: bool,
}
pub struct RuleContext<'a> {
    pub now_ms: u64,
    pub snapshot: Option<&'a GameSnapshot>,
    pub module_settings: &'a Value,
    pub stopped: bool,
}
pub trait GeneralBehavior: Send {
    fn tick(&mut self, context: &RuleContext<'_>) -> RuleOutput;
    fn update(&mut self, settings: &Value) -> Result<(), String>;
    /// Validate settings for this implementation before retained state is mutated.
    fn validate_settings(&self, _settings: &Value) -> Result<(), String> {
        Ok(())
    }
    /// Export retained timers/counters relative to `now_ms`, or `None` when
    /// the rule has no state worth restoring.
    fn export_state(&self, _now_ms: u64) -> Option<Value> {
        None
    }
    /// Restore a blob from `export_state`. Implementations rebase durations
    /// onto `now_ms`; unknown fields are ignored, corruption returns an error.
    fn restore_state(&mut self, _state: &Value, _now_ms: u64) -> Result<(), String> {
        Err("rule does not support checkpoint restore".into())
    }
    fn command_succeeded(&mut self, _command: &GameCommand, _now_ms: u64) {}
    fn plan_failed(&mut self) {}
}
type Factory = Arc<dyn Fn(&Value) -> Result<Box<dyn GeneralBehavior>, String> + Send + Sync>;
#[derive(Clone, Default)]
pub struct BehaviorRegistry {
    entries: Vec<(i32, ModuleDescriptor, Factory)>,
}
impl BehaviorRegistry {
    pub fn filtered(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self {
            entries: self
                .entries
                .iter()
                .filter(|(_, d, _)| keep(&d.id))
                .cloned()
                .collect(),
        }
    }
    pub fn register<F>(
        &mut self,
        priority: i32,
        descriptor: ModuleDescriptor,
        factory: F,
    ) -> Result<(), String>
    where
        F: Fn(&Value) -> Result<Box<dyn GeneralBehavior>, String> + Send + Sync + 'static,
    {
        if descriptor.id.is_empty() || self.entries.iter().any(|(_, d, _)| d.id == descriptor.id) {
            return Err(format!("Invalid/duplicate behavior {}", descriptor.id));
        }
        descriptor.prepare_settings(json!({}))?;
        self.entries.push((priority, descriptor, Arc::new(factory)));
        self.entries.sort_by_key(|(priority, _, _)| *priority);
        Ok(())
    }
    pub fn descriptors(&self) -> Vec<ModuleDescriptor> {
        self.entries.iter().map(|(_, d, _)| d.clone()).collect()
    }
    pub fn create(&self, settings: &Value) -> Result<BehaviorRunner, String> {
        let mut rules = Vec::new();
        for (_, descriptor, factory) in &self.entries {
            let values = descriptor.prepare_settings(
                settings
                    .get(&descriptor.id)
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            )?;
            let rule = factory(&values).map_err(|e| format!("{}: {e}", descriptor.name))?;
            rules.push((descriptor.clone(), rule));
        }
        Ok(BehaviorRunner { rules, owner: None })
    }
}
pub struct BehaviorRunner {
    rules: Vec<(ModuleDescriptor, Box<dyn GeneralBehavior>)>,
    owner: Option<usize>,
}
impl BehaviorRunner {
    pub fn update(&mut self, settings: &Value) -> Result<(), String> {
        // Validate all settings before mutating any retained rule instance.
        let prepared = self
            .rules
            .iter()
            .map(|(d, rule)| {
                let v =
                    d.prepare_settings(settings.get(&d.id).cloned().unwrap_or_else(|| json!({})))?;
                rule.validate_settings(&v)?;
                Ok(v)
            })
            .collect::<Result<Vec<_>, String>>()?;
        for ((_, rule), values) in self.rules.iter_mut().zip(prepared) {
            rule.update(&values)?;
        }
        Ok(())
    }
    pub fn tick(&mut self, context: &RuleContext<'_>) -> RuleOutput {
        self.owner = None;
        let mut result = RuleOutput::default();
        for (index, (descriptor, rule)) in self.rules.iter_mut().enumerate() {
            if result.stop {
                break;
            }
            if let Some(plan) = &result.takeover {
                // Combat selectors may supplement defense, but never interfere with waits or travel.
                if plan.selected_npc.is_none()
                    || !matches!(
                        descriptor.id.as_str(),
                        "best_ammo"
                            | "best_rocket"
                            | "best_launcher"
                            | "best_formation"
                            | "best_ability"
                    )
                {
                    continue;
                }
            }
            let output = rule.tick(context);
            result.status.extend(output.status);
            if output.takeover.is_some() {
                self.owner = Some(index);
                result.takeover = output.takeover;
            }
            result.commands.extend(output.commands);
            result.host_actions.extend(output.host_actions);
            result.stop |= output.stop;
        }
        result
    }
    pub fn command_succeeded(&mut self, command: &GameCommand, now_ms: u64) {
        for (_, rule) in &mut self.rules {
            rule.command_succeeded(command, now_ms);
        }
    }
    pub fn plan_failed(&mut self) {
        if let Some(index) = self.owner {
            self.rules[index].1.plan_failed();
        }
    }
    /// Export every rule's retained state keyed by rule id.
    pub fn export_state(&self, now_ms: u64) -> Value {
        let mut states = serde_json::Map::new();
        for (descriptor, rule) in &self.rules {
            if let Some(state) = rule.export_state(now_ms) {
                states.insert(descriptor.id.clone(), state);
            }
        }
        Value::Object(states)
    }
    /// Restore exported rule state. Per-rule failures are collected rather
    /// than aborting the whole restore; callers surface them as issues.
    pub fn restore_state(&mut self, state: &Value, now_ms: u64) -> Vec<String> {
        let Some(states) = state.as_object() else {
            return vec!["rule state is not an object".into()];
        };
        let mut issues = Vec::new();
        for (descriptor, rule) in &mut self.rules {
            if let Some(rule_state) = states.get(&descriptor.id)
                && let Err(error) = rule.restore_state(rule_state, now_ms)
            {
                issues.push(format!("{}: {error}", descriptor.id));
            }
        }
        issues
    }
}

/// Shared metadata also drives a separate global settings page in the controller.
pub fn behavior_settings_descriptor() -> ModuleDescriptor {
    let mut page = ModuleDescriptor::new("__behaviors", "Behaviors")
        .description("General rules applied independently of the active objective module.");
    for descriptor in builtin_behavior_registry().descriptors() {
        for mut field in descriptor.settings {
            field.key = format!("{}.{}", descriptor.id, field.key);
            field.group = descriptor.name.clone();
            page.settings.push(field);
        }
    }
    page
}
pub fn builtin_behavior_registry() -> BehaviorRegistry {
    let mut registry = BehaviorRegistry::default();
    crate::refresh::register(&mut registry);
    for (priority, id, name, enabled, options) in definitions() {
        let descriptor = ModuleDescriptor::new(id, name)
            .setting(SettingDescriptor::new(
                "enabled",
                "Enabled",
                SettingKind::Boolean,
                json!(enabled),
            ))
            .setting(SettingDescriptor::new(
                "tick_stopped",
                "Run while objective is paused",
                SettingKind::Boolean,
                json!(false),
            ))
            .setting(
                SettingDescriptor::new(
                    "options",
                    "Rule options (JSON)",
                    SettingKind::Json,
                    options,
                )
                .description("See GENERAL_BEHAVIORS.md for options and required observations."),
            );
        registry
            .register(priority, descriptor, move |values| {
                validate(id, values)?;
                Ok(Box::new(BuiltinRule::new(id, values.clone())))
            })
            .expect("unique built-in behavior");
    }
    registry
}
fn definitions() -> Vec<(i32, &'static str, &'static str, bool, Value)> {
    vec![
        (
            0,
            "anti_push",
            "Anti push",
            true,
            json!({"draw_fire_pause_minutes":-1,"death_pause_minutes":-1,"max_deaths":7}),
        ),
        (
            1,
            "stop_button",
            "Safe stop",
            false,
            json!({"requested":false,"close":false}),
        ),
        (
            2,
            "captcha_picker",
            "Captcha picker",
            true,
            json!({"timeout_ms":30000}),
        ),
        (
            3,
            "anti_train",
            "Anti train",
            false,
            json!({"max_enemies":3,"distance":1500,"run":true,"items":[],"condition":{}}),
        ),
        (
            4,
            "ambulance",
            "Ambulance",
            false,
            json!({"health":0.5,"repair_shield":false,"timeout_ms":60000,"return_to_target":true}),
        ),
        (
            5,
            "defense",
            "Defense",
            false,
            json!({"respond_attacks":true,"help":["group","clan","ally"],"help_attack":false,"range":1500,"timeout_ms":15000,"ignored_ids":[],"max_kills":7}),
        ),
        (
            6,
            "urgent_detector",
            "Urgent detector",
            false,
            json!({"interval_ms":10000,"take_quest":false,"profile":""}),
        ),
        (
            7,
            "profile_changer",
            "Profile changer",
            false,
            json!({"interval_ms":1000,"match":"all","conditions":[],"profile":"","close":false,"reload":false}),
        ),
        (8, "fix_x1_travel", "Fix X-1 travel", false, json!({})),
        (9, "fix_bl_travel", "Fix BL travel", true, json!({})),
        (
            10,
            "booty_keys",
            "Booty key manager",
            false,
            json!({"keys":["GREEN","BLUE","RED"],"interval_ms":5000}),
        ),
        (
            11,
            "auto_cloak",
            "Auto cloak",
            false,
            json!({"wait_ms":10000,"only_non_pvp":false,"condition":{}}),
        ),
        (
            12,
            "best_ammo",
            "Auto best ammo",
            false,
            json!({"npc":true,"players":true,"minimum":200,"default":"ammunition_laser_lcb-10","allowed_npc":[],"allowed_players":[],"respect_npc_ammo":true,"only_prometheus":false,"respect_rsb_tag":false,"sab_shield":0.5,"sab_target_shield":10000}),
        ),
        (
            13,
            "best_rocket",
            "Auto best rocket",
            false,
            json!({"npc":true,"players":true,"minimum":1,"default":"ammunition_rocket_r-310","allowed_npc":[],"allowed_players":[]}),
        ),
        (
            14,
            "best_launcher",
            "Auto best rocket launcher",
            false,
            json!({"npc":true,"players":true,"minimum":1,"default":"ammunition_rocketlauncher_eco-10","allowed_npc":[],"allowed_players":[],"sab_shield":0.5,"sab_target_shield":10000}),
        ),
        (
            15,
            "best_formation",
            "Auto best formation",
            false,
            json!({"npc":true,"players":true,"interval_ms":1000,"allowed":[],"default":"default","copy_player":false,"veteran":true,"respect_npc_formation":true}),
        ),
        (
            16,
            "best_ability",
            "Auto best ability",
            false,
            json!({"npc":true,"interval_ms":1000,"health":0.5,"min_target_hp":100000,"allowed":[],"always":[]}),
        ),
        (
            17,
            "custom_events",
            "Custom events",
            false,
            json!({"events":[]}),
        ),
        (
            18,
            "others",
            "Others",
            false,
            json!({"auto_buy_lcb":false,"auto_buy_plt":false,"reload_crash":false,"max_memory_mb":0,"close_disconnected_ms":0,"max_deaths_sid_ko":0,"clan_tag":""}),
        ),
        (
            19,
            "calendar_bonus",
            "Calendar bonus",
            false,
            json!({"interval_ms":60000}),
        ),
    ]
}
fn validate(id: &str, values: &Value) -> Result<(), String> {
    let Some(options) = values.get("options").and_then(Value::as_object) else {
        return Err("options must be an object".into());
    };
    let defaults = definitions()
        .into_iter()
        .find(|(_, rule_id, _, _, _)| *rule_id == id)
        .map(|(_, _, _, _, options)| options)
        .ok_or_else(|| format!("unknown built-in behavior {id}"))?;
    for (key, value) in options {
        let Some(default) = defaults.get(key) else {
            return Err(format!("unknown option {key} for {id}"));
        };
        let type_matches = match default {
            Value::Null => true,
            Value::Bool(_) => value.is_boolean(),
            Value::String(_) => value.is_string(),
            Value::Array(_) => value.is_array(),
            Value::Object(_) => value.is_object(),
            Value::Number(n) if n.is_i64() => value.as_i64().is_some(),
            Value::Number(n) if n.is_u64() => value.as_u64().is_some(),
            Value::Number(_) => value.is_number(),
        };
        if !type_matches {
            return Err(format!("option {key} has the wrong type"));
        }
        if (key.ends_with("_ms")
            || matches!(
                key.as_str(),
                "distance"
                    | "range"
                    | "minimum"
                    | "max_enemies"
                    | "max_deaths"
                    | "max_kills"
                    | "min_target_hp"
                    | "max_memory_mb"
                    | "max_deaths_sid_ko"
                    | "sab_target_shield"
            ))
            && !value.as_u64().is_some_and(|v| v <= 86_400_000)
        {
            return Err(format!("{key} must be a nonnegative integer <= 86400000"));
        }
        if matches!(key.as_str(), "health" | "sab_shield")
            && !value.as_f64().is_some_and(|v| (0.0..=1.0).contains(&v))
        {
            return Err(format!("{key} must be between 0 and 1"));
        }
        if key.ends_with("pause_minutes")
            && !value.as_i64().is_some_and(|v| (-1..=300).contains(&v))
        {
            return Err(format!("{key} must be -1..300"));
        }
        if matches!(
            key.as_str(),
            "events"
                | "conditions"
                | "items"
                | "allowed"
                | "allowed_npc"
                | "allowed_players"
                | "always"
                | "keys"
                | "help"
                | "ignored_ids"
        ) && !value.is_array()
        {
            return Err(format!("{key} must be an array"));
        }
    }
    if let Some(condition) = options.get("condition") {
        validate_condition(condition)?;
    }
    for (key, items) in options
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "conditions" | "items" | "events"))
    {
        if let Some(items) = items.as_array() {
            for item in items {
                if key == "conditions" {
                    validate_condition(item)?;
                }
                if key == "items" && !item.is_object() {
                    return Err("items entries must be objects".into());
                }
                if key == "events" {
                    if !item.is_object() {
                        return Err("events entries must be objects".into());
                    }
                    if let Some(condition) = item.get("condition") {
                        validate_condition(condition)?;
                    }
                    for field in ["item", "activate", "select"] {
                        if let Some(value) = item.get(field) {
                            let valid = if field == "item" {
                                value.is_string()
                            } else {
                                value.is_boolean()
                            };
                            if !valid {
                                return Err(format!("event {field} has the wrong type"));
                            }
                        }
                    }
                    if let Some(minimum) = item.get("minimum") {
                        if !minimum.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0) {
                            return Err("event minimum must be a nonnegative number".into());
                        }
                    }
                    if let Some(cooldown) = item.get("cooldown_ms") {
                        if !cooldown.as_u64().is_some_and(|n| n <= 86_400_000) {
                            return Err(
                                "event cooldown_ms must be a nonnegative integer <= 86400000"
                                    .into(),
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
fn validate_condition(condition: &Value) -> Result<(), String> {
    let Some(fields) = condition.as_object() else {
        return Err("condition must be an object".into());
    };
    for (key, value) in fields {
        let valid = match key.as_str() {
            "hp_below" | "shield_below" => value
                .as_f64()
                .is_some_and(|n| n.is_finite() && (0.0..=1.0).contains(&n)),
            "attacking" => value.is_boolean(),
            "map" => value.as_i64().is_some(),
            "map_time_ms" | "elapsed_ms" | "deaths" => value.as_u64().is_some(),
            "target_name" | "key_empty" => value.is_string(),
            "field" | "npc_kills" | "resources" | "time" => value.is_object(),
            _ => return Err(format!("unknown condition {key}")),
        };
        if !valid {
            return Err(format!("condition {key} has the wrong type"));
        }
    }
    Ok(())
}

struct BuiltinRule {
    id: &'static str,
    settings: Value,
    next_check: u64,
    last_attack: u64,
    dead: bool,
    deaths: BTreeMap<String, VecDeque<u64>>,
    death_count: u64,
    pause_until: Option<u64>,
    target: Option<String>,
    target_since: u64,
    last_hp: Option<f64>,
    kills: BTreeMap<String, u64>,
    traveler: MapTraveler,
    reroute: bool,
    event_times: BTreeMap<usize, u64>,
    map_since: u64,
    map: Option<i32>,
    fired: bool,
    disconnected_since: Option<u64>,
    captcha: VecDeque<String>,
    captcha_signature: Option<Value>,
    pending_item: Option<String>,
}
impl BuiltinRule {
    fn new(id: &'static str, settings: Value) -> Self {
        let settings = with_default_options(id, settings);
        Self {
            id,
            settings,
            next_check: 0,
            last_attack: 0,
            dead: false,
            deaths: BTreeMap::new(),
            death_count: 0,
            pause_until: None,
            target: None,
            target_since: 0,
            last_hp: None,
            kills: BTreeMap::new(),
            traveler: MapTraveler::default(),
            reroute: false,
            event_times: BTreeMap::new(),
            map_since: 0,
            map: None,
            fired: false,
            disconnected_since: None,
            captcha: VecDeque::new(),
            captcha_signature: None,
            pending_item: None,
        }
    }
}
/// Retained per-rule state for checkpoint export. Deadlines become remaining
/// durations and stamps become ages, all relative to the export tick.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct RuleCheckpoint {
    next_check_in_ms: Option<u64>,
    last_attack_age_ms: Option<u64>,
    dead: bool,
    /// Per-name kill/death observation stamps, stored as ages.
    deaths_ages: BTreeMap<String, VecDeque<u64>>,
    death_count: u64,
    pause_in_ms: Option<u64>,
    target: Option<String>,
    target_since_age_ms: Option<u64>,
    last_hp: Option<f64>,
    kills: BTreeMap<String, u64>,
    traveler: crate::TravelerCheckpoint,
    reroute: bool,
    /// Per-rule-event stamps, stored as ages.
    event_ages: BTreeMap<usize, u64>,
    map_elapsed_ms: Option<u64>,
    map: Option<i32>,
    fired: bool,
    disconnected_age_ms: Option<u64>,
    captcha: VecDeque<String>,
    captcha_signature: Option<Value>,
    pending_item: Option<String>,
}
impl BuiltinRule {
    fn options(&self) -> &Value {
        &self.settings["options"]
    }
    fn due(&mut self, now: u64, default: u64) -> bool {
        if now < self.next_check {
            return false;
        }
        self.next_check =
            now.saturating_add(integer(self.options(), "interval_ms", default).max(150));
        true
    }
}
impl GeneralBehavior for BuiltinRule {
    fn update(&mut self, settings: &Value) -> Result<(), String> {
        validate(self.id, settings)?;
        self.settings = with_default_options(self.id, settings.clone());
        Ok(())
    }
    fn validate_settings(&self, settings: &Value) -> Result<(), String> {
        validate(self.id, settings)
    }
    fn command_succeeded(&mut self, command: &GameCommand, now: u64) {
        if let GameCommand::UseItem { item_id, .. } = command {
            if self.pending_item.as_ref() == Some(item_id) {
                self.pending_item = None;
                self.next_check = now.saturating_add(750);
            }
        }
    }
    fn plan_failed(&mut self) {
        self.traveler.route_failed();
    }
    fn export_state(&self, now_ms: u64) -> Option<Value> {
        let checkpoint = RuleCheckpoint {
            next_check_in_ms: crate::remaining_ms(Some(self.next_check), now_ms),
            last_attack_age_ms: crate::age_ms(
                (self.last_attack > 0).then_some(self.last_attack),
                now_ms,
            ),
            dead: self.dead,
            deaths_ages: self
                .deaths
                .iter()
                .map(|(name, stamps)| {
                    (
                        name.clone(),
                        stamps
                            .iter()
                            .map(|at| now_ms.saturating_sub(*at))
                            .collect(),
                    )
                })
                .collect(),
            death_count: self.death_count,
            pause_in_ms: crate::remaining_ms(self.pause_until, now_ms),
            target: self.target.clone(),
            target_since_age_ms: crate::age_ms(
                (self.target_since > 0).then_some(self.target_since),
                now_ms,
            ),
            last_hp: self.last_hp,
            kills: self.kills.clone(),
            traveler: self.traveler.export_checkpoint(now_ms),
            reroute: self.reroute,
            event_ages: self
                .event_times
                .iter()
                .map(|(index, at)| (*index, now_ms.saturating_sub(*at)))
                .collect(),
            map_elapsed_ms: crate::age_ms(
                (self.map_since > 0).then_some(self.map_since),
                now_ms,
            ),
            map: self.map,
            fired: self.fired,
            disconnected_age_ms: crate::age_ms(self.disconnected_since, now_ms),
            captcha: self.captcha.clone(),
            captcha_signature: self.captcha_signature.clone(),
            pending_item: self.pending_item.clone(),
        };
        serde_json::to_value(&checkpoint).ok()
    }
    fn restore_state(&mut self, state: &Value, now_ms: u64) -> Result<(), String> {
        let checkpoint: RuleCheckpoint = serde_json::from_value(state.clone())
            .map_err(|error| format!("corrupt rule checkpoint: {error}"))?;
        self.next_check = crate::rebase_deadline(now_ms, checkpoint.next_check_in_ms)
            .unwrap_or(0);
        self.last_attack = crate::rebase_age(now_ms, checkpoint.last_attack_age_ms)
            .unwrap_or(0);
        self.dead = checkpoint.dead;
        self.deaths = checkpoint
            .deaths_ages
            .iter()
            .map(|(name, ages)| {
                (
                    name.clone(),
                    ages.iter()
                        .map(|age| now_ms.saturating_sub(*age))
                        .collect(),
                )
            })
            .collect();
        self.death_count = checkpoint.death_count;
        self.pause_until = crate::rebase_deadline(now_ms, checkpoint.pause_in_ms);
        self.target = checkpoint.target.clone();
        self.target_since = crate::rebase_age(now_ms, checkpoint.target_since_age_ms)
            .unwrap_or(0);
        self.last_hp = checkpoint.last_hp;
        self.kills = checkpoint.kills.clone();
        self.traveler
            .restore_checkpoint(&checkpoint.traveler, now_ms)
            .map_err(|error| error.to_string())?;
        self.reroute = checkpoint.reroute;
        self.event_times = checkpoint
            .event_ages
            .iter()
            .map(|(index, age)| (*index, now_ms.saturating_sub(*age)))
            .collect();
        self.map_since = crate::rebase_age(now_ms, checkpoint.map_elapsed_ms)
            .unwrap_or(0);
        self.map = checkpoint.map;
        self.fired = checkpoint.fired;
        self.disconnected_since = crate::rebase_age(now_ms, checkpoint.disconnected_age_ms);
        self.captcha = checkpoint.captcha.clone();
        self.captcha_signature = checkpoint.captcha_signature.clone();
        self.pending_item = checkpoint.pending_item.clone();
        Ok(())
    }
    fn tick(&mut self, context: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !boolean(&self.settings, "enabled", false) {
            return out;
        }
        let now = context.now_ms;
        let options = self.options().clone();
        let Some(snapshot) = context.snapshot else {
            if self.id == "others" {
                let since = *self.disconnected_since.get_or_insert(now);
                let delay = integer(&options, "close_disconnected_ms", 0);
                if delay > 0 && now.saturating_sub(since) >= delay && !self.fired {
                    out.host_actions.push(HostAction::Close);
                    out.stop = true;
                    self.fired = true;
                }
            }
            return out;
        };
        self.disconnected_since = None;
        let hero = snapshot
            .entities
            .iter()
            .find(|e| e.category == EntityCategory::Player);
        let killed = snapshot.navigation.hero.killed || hero.is_some_and(|h| h.health == Some(0.0));
        if killed && !self.dead {
            self.death_count += 1;
            if let Some(killer) = hero.and_then(|h| {
                h.fields
                    .get("lastDestroyerId")
                    .or_else(|| h.fields.get("lastDestroyerName"))
            }) {
                self.deaths
                    .entry(killer.to_string())
                    .or_default()
                    .push_back(now);
            }
        }
        self.dead = killed;
        if snapshot.navigation.map_id != self.map {
            self.map = snapshot.navigation.map_id;
            self.map_since = now;
        }
        if context.stopped
            && !boolean(&self.settings, "tick_stopped", false)
            && !matches!(self.id, "anti_push" | "stop_button" | "others")
        {
            return out;
        }
        if self.id == "anti_push" {
            for times in self.deaths.values_mut() {
                while times
                    .front()
                    .is_some_and(|t| now.saturating_sub(*t) >= 86_400_000)
                {
                    times.pop_front();
                }
            }
            // Upstream stopped ticks record deaths; enforcement resumes with the objective.
            if context.stopped {
                return out;
            }
            if let Some(until) = self.pause_until {
                if now < until {
                    out.takeover = Some(wait_plan(snapshot, "Anti push: paused"));
                    return out;
                }
                self.pause_until = None;
            }
            let target = hero.and_then(|h| local_target(snapshot, h));
            let draw = target.is_some_and(|t| flag(t, "isEnemy") && effect(t, "DRAW_FIRE"));
            let excessive = self
                .deaths
                .values()
                .any(|times| times.len() as u64 >= integer(&options, "max_deaths", 7));
            let minutes = if draw {
                signed(&options, "draw_fire_pause_minutes", -1)
            } else if excessive {
                signed(&options, "death_pause_minutes", -1)
            } else {
                0
            };
            if minutes != 0 {
                out.takeover = Some(wait_plan(
                    snapshot,
                    "Anti push: draw fire or repeated deaths",
                ));
                if minutes < 0 {
                    out.host_actions.push(HostAction::Disconnect);
                    out.stop = true;
                } else {
                    self.pause_until = Some(now.saturating_add(minutes as u64 * 60000));
                }
                if excessive {
                    self.deaths.clear();
                }
            }
            return out;
        }
        if killed {
            return out;
        }
        let Some(hero) = hero else {
            return out;
        };
        let target = local_target(snapshot, hero);
        let attacking = flag(hero, "isAttacking")
            || hero
                .fields
                .get("attackingTargetId")
                .is_some_and(|v| !v.is_null() && v.as_i64() != Some(0));
        if attacking {
            self.last_attack = now;
        }
        let safety_enabled = context
            .module_settings
            .get("safety")
            .is_some_and(|s| boolean(s, "enabled", true));
        let repair_threshold = context
            .module_settings
            .get("safety")
            .map(|s| number(s, "min_health_percent", 40.0) / 100.0)
            .unwrap_or(0.4);
        if matches!(self.id, "defense" | "fix_x1_travel" | "fix_bl_travel")
            && safety_enabled
            && hp(hero).is_some_and(|p| p < repair_threshold)
        {
            return out;
        }
        match self.id {
            "stop_button" => {
                if !boolean(&options, "requested", false) {
                    return out;
                }
                if flag(hero, "isGG") {
                    if let Some(portal) = snapshot.navigation.portals.iter().find(|p| {
                        !p.jumping
                            && p.target_map.is_some_and(|id| {
                                snapshot
                                    .navigation
                                    .star_system
                                    .map(id)
                                    .is_some_and(|m| !is_gate(&m.name))
                            })
                    }) {
                        let mut plan = wait_plan(snapshot, "Safe stop: leaving gate");
                        if distance_point(hero, portal.position.x, portal.position.y) < 200.0 {
                            plan.controls.commands.push(GameCommand::JumpPortal {
                                portal_id: portal.id,
                            });
                        } else {
                            plan.actions.push(ActionIntent::MoveTo {
                                destination: Point {
                                    x: portal.position.x,
                                    y: portal.position.y,
                                },
                            });
                        }
                        out.takeover = Some(plan);
                    }
                } else {
                    out.takeover = Some(wait_plan(snapshot, "Safe stop"));
                    out.host_actions.push(if boolean(&options, "close", false) {
                        HostAction::Close
                    } else {
                        HostAction::Disconnect
                    });
                    out.stop = true;
                }
            }
            "captcha_picker" => self.captcha_tick(snapshot, hero, now, &options, &mut out),
            "anti_train" => {
                let enemies = snapshot
                    .entities
                    .iter()
                    .filter(|e| {
                        is_player(e)
                            && (flag(e, "isEnemy") || flag(e, "isBlacklisted"))
                            && distance(hero, e) < number(&options, "distance", 1500.0)
                    })
                    .count();
                if enemies >= integer(&options, "max_enemies", 3).max(1) as usize
                    && condition(
                        &options["condition"],
                        context,
                        hero,
                        target,
                        self.death_count,
                        self.map_since,
                    )
                {
                    if boolean(&options, "run", true) {
                        let mut plan = wait_plan(snapshot, "Anti train: escaping enemies");
                        plan.controls.ship_mode = Some(ShipMode::Run);
                        if let Some(map) = &snapshot.map {
                            if let Some(safe) = map.safe_places.iter().min_by(|a, b| {
                                distance_point(hero, a.x, a.y)
                                    .total_cmp(&distance_point(hero, b.x, b.y))
                            }) {
                                plan.actions.push(ActionIntent::MoveTo {
                                    destination: Point {
                                        x: safe.x,
                                        y: safe.y,
                                    },
                                });
                            }
                        }
                        out.takeover = Some(plan);
                    }
                    self.events(&options["items"], context, hero, target, &mut out);
                }
            }
            "auto_cloak" => {
                if !attacking
                    && !flag(hero, "isInvisible")
                    && now.saturating_sub(self.last_attack) >= integer(&options, "wait_ms", 10000)
                    && (!boolean(&options, "only_non_pvp", false)
                        || hero.fields.get("isPvp").and_then(Value::as_bool) == Some(false))
                    && condition(
                        &options["condition"],
                        context,
                        hero,
                        target,
                        self.death_count,
                        self.map_since,
                    )
                    && self.due(now, 750)
                {
                    use_first(
                        snapshot,
                        &["equipment_extra_cpu_cl04k"],
                        &options,
                        0.0,
                        false,
                        &mut out.commands,
                    );
                }
            }
            "booty_keys" => {
                // Filtering is done before module planning so it can choose another box immediately.
            }
            "best_ammo" | "best_rocket" | "best_launcher" => {
                if let Some(target) = target.filter(|t| {
                    attacking
                        && distance(hero, t)
                            < if self.id == "best_ammo" {
                                730.0
                            } else {
                                1000.0
                            }
                }) {
                    if self.due(now, 500) {
                        self.combat_items(snapshot, hero, target, &options, &mut out);
                    }
                }
            }
            "best_formation" => {
                if self.due(now, 1000) {
                    self.formation(snapshot, hero, target, attacking, &options, &mut out);
                }
            }
            "best_ability" => {
                if self.due(now, 1000) {
                    self.ability(snapshot, hero, target, attacking, &options, &mut out);
                }
            }
            "defense" => self.defense(snapshot, hero, now, &options, &mut out),
            "ambulance" => self.ambulance(snapshot, hero, now, &options, &mut out),
            "custom_events" => self.events(&options["events"], context, hero, target, &mut out),
            "fix_x1_travel" | "fix_bl_travel" => self.travel(context, snapshot, hero, &mut out),
            "profile_changer" => {
                if self.due(now, 1000) {
                    let conditions = options["conditions"].as_array();
                    let matches = conditions.is_some_and(|cs| {
                        !cs.is_empty()
                            && if text(&options, "match") == Some("any") {
                                cs.iter().any(|c| {
                                    condition(
                                        c,
                                        context,
                                        hero,
                                        target,
                                        self.death_count,
                                        self.map_since,
                                    )
                                })
                            } else {
                                cs.iter().all(|c| {
                                    condition(
                                        c,
                                        context,
                                        hero,
                                        target,
                                        self.death_count,
                                        self.map_since,
                                    )
                                })
                            }
                    });
                    if matches && !self.fired {
                        if boolean(&options, "close", false) {
                            out.host_actions.push(HostAction::Close);
                            out.stop = true;
                        } else if let Some(profile) =
                            text(&options, "profile").filter(|s| !s.is_empty())
                        {
                            out.host_actions.push(HostAction::Profile(profile.into()));
                            out.stop = true;
                        }
                        if boolean(&options, "reload", false) {
                            out.host_actions.push(HostAction::Refresh);
                        }
                    }
                    self.fired = matches;
                }
            }
            "urgent_detector" => {
                if self.due(now, 10000) {
                    let waiting = flag(hero, "urgentQuestWaiting");
                    let active = waiting || flag(hero, "urgentQuestAccepted");
                    if active && !self.fired {
                        if waiting && boolean(&options, "take_quest", false) {
                            out.host_actions.push(HostAction::AcceptUrgentQuest);
                        } else if let Some(profile) =
                            text(&options, "profile").filter(|s| !s.is_empty())
                        {
                            out.host_actions.push(HostAction::Profile(profile.into()));
                            out.stop = true;
                        }
                    }
                    self.fired = active;
                }
            }
            "calendar_bonus" => {
                if self.due(now, 60000) && flag(hero, "calendarRewardAvailable") {
                    out.host_actions.push(HostAction::ClaimCalendar);
                }
            }
            "others" => self.others(snapshot, hero, now, &options, &mut out),
            _ => {}
        }
        out
    }
}
fn with_default_options(id: &str, mut settings: Value) -> Value {
    if let Some((_, _, _, _, mut defaults)) = definitions()
        .into_iter()
        .find(|(_, rule, _, _, _)| *rule == id)
    {
        if let Some(options) = settings.get("options").and_then(Value::as_object) {
            for (key, value) in options {
                defaults[key] = value.clone();
            }
        }
        settings["options"] = defaults;
    }
    settings
}

impl BehaviorRunner {
    /// Never let a generic collector pick captcha boxes; key rules filter only known counts.
    pub fn module_snapshot(&self, snapshot: &GameSnapshot) -> GameSnapshot {
        let mut filtered = snapshot.clone();
        filtered.entities.retain(|e| !is_captcha(e));
        filtered
    }
}
pub fn filter_booty_boxes(snapshot: &mut GameSnapshot, settings: &Value) {
    let rule = &settings["booty_keys"];
    if !boolean(rule, "enabled", false) {
        return;
    }
    let Some(keys) = rule["options"]["keys"].as_array() else {
        return;
    };
    let counts = snapshot
        .entities
        .iter()
        .find(|e| e.category == EntityCategory::Player)
        .and_then(|h| h.fields.get("bootyKeys"))
        .cloned()
        .unwrap_or(Value::Null);
    snapshot.entities.retain(|e| {
        !keys.iter().filter_map(Value::as_str).any(|key| {
            booty_box(key) == Some(box_type(e))
                && counts.get(key).and_then(Value::as_f64) == Some(0.0)
        })
    });
}
fn booty_box(key: &str) -> Option<&'static str> {
    Some(match key {
        "GREEN" => "PIRATE_BOOTY_GOLD",
        "BLUE" => "PIRATE_BOOTY_BLUE",
        "RED" => "PIRATE_BOOTY_RED",
        "SILVER" => "PIRATE_BOOTY_SILVER",
        "APOCALYPSE" => "MASQUE_BOOTY_BOX",
        "PROMETHEUS" => "PROMETHEUS_BOOTY_BOX",
        "OBSIDIAN_MICROCHIP" => "BLACK_BOOTY_BOX",
        "PROSPEROUS_FRAGMENT" => "PROSPEROUS_BOOTY_BOX",
        "ASTRAL" => "ASTRAL_BOOTY_BOX",
        "ASTRAL_SUPREME" => "ASTRAL_PRIME_BOOTY_BOX",
        "EMPYRIAN" => "EMPYRIAN_BOOTY_BOX",
        "PERSEUS" => "PERSEUS_BLESSING_BOOTY_BOX",
        "LUCENT" => "LUCENT_ALIEN_EGG_BOOTY_BOX",
        "BLACK_LIGHT_CODE" => "UNSTABLE_BLACKLIGHT_CACHE",
        "BLACK_LIGHT_DECODER" => "BLACKLIGHT-CRYPT",
        _ => return None,
    })
}
fn boolean(value: &Value, key: &str, default: bool) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(default)
}
fn integer(value: &Value, key: &str, default: u64) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(default)
}
fn signed(value: &Value, key: &str, default: i64) -> i64 {
    value.get(key).and_then(Value::as_i64).unwrap_or(default)
}
fn number(value: &Value, key: &str, default: f64) -> f64 {
    value
        .get(key)
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
        .unwrap_or(default)
}
fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}
fn flag(e: &GameEntity, key: &str) -> bool {
    e.fields.get(key).and_then(Value::as_bool) == Some(true)
}
fn field_num(e: &GameEntity, key: &str) -> Option<f64> {
    e.fields
        .get(key)
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|n| n.is_finite())
}
fn hp(e: &GameEntity) -> Option<f64> {
    Some(e.health? / e.max_health.filter(|n| *n > 0.0)?)
}
fn shield(e: &GameEntity) -> Option<f64> {
    Some(e.shield? / e.max_shield.filter(|n| *n > 0.0)?)
}
fn distance_point(e: &GameEntity, x: f64, y: f64) -> f64 {
    (e.position.x - x).hypot(e.position.y - y)
}
fn distance(a: &GameEntity, b: &GameEntity) -> f64 {
    distance_point(a, b.position.x, b.position.y)
}
fn key(e: &GameEntity) -> String {
    e.id.as_ref()
        .map(|id| format!("id:{id}"))
        .unwrap_or_else(|| format!("path:{}", e.path))
}
fn entity_ref(e: &GameEntity) -> EntityRef {
    EntityRef {
        path: e.path.clone(),
        id: e.id.clone(),
        name: e.name.clone(),
        class_name: e.class_name.clone(),
        position: Point {
            x: e.position.x,
            y: e.position.y,
        },
    }
}
fn is_player(e: &GameEntity) -> bool {
    e.category == EntityCategory::Ship && !flag(e, "isPet")
}
fn valid(e: &GameEntity) -> bool {
    e.fields.get("isValid").and_then(Value::as_bool) != Some(false) && e.health != Some(0.0)
}
fn local_target<'a>(s: &'a GameSnapshot, h: &GameEntity) -> Option<&'a GameEntity> {
    let id = h
        .fields
        .get("localTargetId")
        .or_else(|| h.fields.get("lockedTargetId"))?;
    s.entities
        .iter()
        .find(|e| e.id.as_ref() == Some(id) && valid(e))
}
fn effect(e: &GameEntity, name: &str) -> bool {
    flag(e, name)
        || e.fields
            .get("effects")
            .and_then(Value::as_array)
            .is_some_and(|es| {
                es.iter().any(|v| {
                    v.as_str() == Some(name)
                        || effect_id(name).is_some_and(|id| {
                            v.as_i64().or_else(|| {
                                v.as_f64().filter(|n| n.fract() == 0.0).map(|n| n as i64)
                            }) == Some(id)
                                || v.get("_id").or_else(|| v.get("id")).and_then(|id| {
                                    id.as_i64().or_else(|| {
                                        id.as_f64().filter(|n| n.fract() == 0.0).map(|n| n as i64)
                                    })
                                }) == Some(id)
                        })
                })
            })
}
fn effect_id(name: &str) -> Option<i64> {
    Some(match name {
        "LOCATOR" => 1,
        "PET_SPAWN" => 2,
        "ENERGY_LEECH" => 11,
        "NPC_ISH" => 16,
        "BOX_COLLECTING" => 20,
        "BOOTY_COLLECTING" => 21,
        "DRAW_FIRE" => 36,
        "STICKY_BOMB" => 56,
        "POLARITY_POSITIVE" => 65,
        "POLARITY_NEGATIVE" => 66,
        "REPAIR_BOT" => 76,
        "ISH" => 84,
        "INFECTION" => 85,
        _ => name.parse().ok()?,
    })
}
fn protected(e: &GameEntity) -> bool {
    ["ISH", "NPC_ISH", "PET_SPAWN"].iter().any(|s| effect(e, s))
}
fn box_type(e: &GameEntity) -> &str {
    e.fields
        .get("typeName")
        .and_then(Value::as_str)
        .unwrap_or(&e.name)
}
fn is_captcha(e: &GameEntity) -> bool {
    matches!(box_type(e), "POISON_PUSAT_BOX_BLACK" | "BONUS_BOX_RED")
}
fn is_gate(name: &str) -> bool {
    !name.as_bytes().get(1).is_some_and(|b| *b == b'-') && !name.to_lowercase().contains("bl")
}
fn wait_plan(snapshot: &GameSnapshot, status: &str) -> BehaviorDecision {
    BehaviorDecision {
        mode: BehaviorMode::Collector,
        player: snapshot
            .entities
            .iter()
            .find(|e| e.category == EntityCategory::Player)
            .map(entity_ref),
        selected_npc: None,
        selected_box: None,
        actions: vec![],
        note: Some(status.into()),
        controls: ControlRequests::default(),
        suspended: true,
        can_refresh: false,
    }
}
fn ready(snapshot: &GameSnapshot, id: &str, minimum: f64, select: bool) -> bool {
    snapshot.navigation.hero.items.get(id).is_some_and(|i| {
        i.available
            && i.activatable
            && !i.blocked
            && (!select || !i.selected)
            && (minimum == 0.0 || i.quantity.is_some_and(|q| q >= minimum))
    })
}
fn use_first(
    snapshot: &GameSnapshot,
    ids: &[&str],
    options: &Value,
    minimum: f64,
    select: bool,
    commands: &mut Vec<GameCommand>,
) -> Option<String> {
    let allowed = options.get("allowed").and_then(Value::as_array);
    let id = ids.iter().find(|id| {
        allowed.is_none_or(|a| a.is_empty() || a.iter().any(|v| v.as_str() == Some(**id)))
            && ready(snapshot, id, minimum, false)
    })?;
    if select
        && snapshot
            .navigation
            .hero
            .items
            .get(*id)
            .is_some_and(|item| item.selected)
    {
        return None;
    }
    commands.push(GameCommand::UseItem {
        item_id: (*id).into(),
        activate: true,
    });
    Some((*id).into())
}
fn condition(
    c: &Value,
    ctx: &RuleContext<'_>,
    hero: &GameEntity,
    target: Option<&GameEntity>,
    deaths: u64,
    map_since: u64,
) -> bool {
    let Some(c) = c.as_object() else {
        return false;
    };
    c.iter().all(|(k, v)| match k.as_str() {
        "hp_below" => hp(hero).zip(v.as_f64()).is_some_and(|(h, n)| h < n),
        "shield_below" => shield(hero).zip(v.as_f64()).is_some_and(|(h, n)| h < n),
        "attacking" => hero
            .fields
            .get("isAttacking")
            .and_then(Value::as_bool)
            .zip(v.as_bool())
            .is_some_and(|(a, b)| a == b),
        "map" => {
            ctx.snapshot
                .and_then(|s| s.navigation.map_id)
                .map(i64::from)
                == v.as_i64()
        }
        "map_time_ms" => v
            .as_u64()
            .is_some_and(|n| ctx.now_ms.saturating_sub(map_since) >= n),
        "elapsed_ms" => v.as_u64().is_some_and(|n| ctx.now_ms >= n),
        "deaths" => v.as_u64().is_some_and(|n| deaths >= n),
        "target_name" => target
            .zip(v.as_str())
            .is_some_and(|(t, n)| t.name.to_lowercase().contains(&n.to_lowercase())),
        "field" => v
            .as_object()
            .is_some_and(|fs| fs.iter().all(|(k, v)| hero.fields.get(k) == Some(v))),
        "key_empty" => v.as_str().is_some_and(|k| {
            hero.fields
                .get("bootyKeys")
                .and_then(|v| v.get(k))
                .and_then(Value::as_f64)
                == Some(0.0)
        }),
        "npc_kills" | "resources" => v.as_object().is_some_and(|fs| {
            fs.iter().all(|(name, v)| {
                hero.fields
                    .get(if k == "npc_kills" {
                        "npcKills"
                    } else {
                        "resourcesCollected"
                    })
                    .and_then(|f| f.get(name))
                    .and_then(Value::as_u64)
                    .zip(v.as_u64())
                    .is_some_and(|(a, b)| a >= b)
            })
        }),
        "time" => v.as_object().is_some_and(|t| {
            hero.fields.get("localHour") == t.get("hour")
                && hero
                    .fields
                    .get("localMinute")
                    .and_then(Value::as_u64)
                    .zip(t.get("minute").and_then(Value::as_u64))
                    .is_some_and(|(a, b)| a >= b && a <= b + 5)
        }),
        _ => false,
    })
}

impl BuiltinRule {
    fn events(
        &mut self,
        events: &Value,
        ctx: &RuleContext<'_>,
        hero: &GameEntity,
        target: Option<&GameEntity>,
        out: &mut RuleOutput,
    ) {
        let Some(snapshot) = ctx.snapshot else { return };
        for (index, event) in events.as_array().into_iter().flatten().enumerate() {
            let cooldown = integer(event, "cooldown_ms", 1000).max(150);
            if self
                .event_times
                .get(&index)
                .is_some_and(|last| ctx.now_ms.saturating_sub(*last) < cooldown)
            {
                continue;
            }
            if !condition(
                &event["condition"],
                ctx,
                hero,
                target,
                self.death_count,
                self.map_since,
            ) {
                continue;
            }
            if let Some(id) = text(event, "item") {
                if ready(
                    snapshot,
                    id,
                    number(event, "minimum", 0.0),
                    boolean(event, "select", false),
                ) {
                    out.commands.push(GameCommand::UseItem {
                        item_id: id.into(),
                        activate: boolean(event, "activate", true),
                    });
                    self.event_times.insert(index, ctx.now_ms);
                }
            }
        }
    }
    fn captcha_tick(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        now: u64,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        if !s.entities.iter().any(is_captcha) {
            self.target_since = 0;
            self.captcha.clear();
            self.captcha_signature = None;
            return;
        }
        if self.target_since == 0 {
            self.target_since = now.max(1);
        }
        let mut plan = wait_plan(s, "Captcha picker: waiting for instructions");
        let instruction = h.fields.get("captcha");
        if instruction != self.captcha_signature.as_ref() {
            self.captcha.clear();
            self.captcha_signature = instruction.cloned();
            self.next_check = now + 500;
            if let Some(kind) = instruction.and_then(|i| text(i, "box_type")) {
                let mut boxes: Vec<_> = s
                    .entities
                    .iter()
                    .filter(|e| box_type(e) == kind && is_captcha(e))
                    .collect();
                if !boxes.is_empty() {
                    let center = (
                        boxes.iter().map(|e| e.position.x).sum::<f64>() / boxes.len() as f64,
                        boxes.iter().map(|e| e.position.y).sum::<f64>() / boxes.len() as f64,
                    );
                    let angle =
                        |e: &GameEntity| (e.position.x - center.0).atan2(e.position.y - center.1);
                    let first = boxes
                        .iter()
                        .min_by(|a, b| distance(h, a).total_cmp(&distance(h, b)))
                        .map(|e| angle(e))
                        .unwrap_or(0.0);
                    boxes.sort_by(|a, b| {
                        ((angle(a) - first).rem_euclid(std::f64::consts::TAU))
                            .total_cmp(&(angle(b) - first).rem_euclid(std::f64::consts::TAU))
                    });
                    let amount = instruction
                        .and_then(|i| i.get("amount"))
                        .and_then(Value::as_u64)
                        .map(|n| n as usize)
                        .unwrap_or(boxes.len());
                    self.captcha = boxes.into_iter().take(amount).map(key).collect();
                }
            }
        }
        while self.captcha.front().is_some_and(|k| {
            !s.entities
                .iter()
                .any(|e| key(e) == *k && !flag(e, "isCollected"))
        }) {
            self.captcha.pop_front();
        }
        if now.saturating_sub(self.target_since) > integer(o, "timeout_ms", 30000) {
            out.host_actions.push(HostAction::Refresh);
            self.target_since = now;
            plan.note = Some("Captcha picker timed out; refreshing".into());
        } else if now >= self.next_check {
            if let Some(box_entity) = self
                .captcha
                .front()
                .and_then(|k| s.entities.iter().find(|e| key(e) == *k))
            {
                let target = entity_ref(box_entity);
                plan.selected_box = Some(target.clone());
                plan.actions.push(ActionIntent::CollectBox {
                    target,
                    wait_ms: 500,
                });
                plan.note = Some("Captcha picker: collecting requested box".into());
                self.next_check = now
                    + 500
                    + (distance(h, box_entity) / field_num(h, "speed").unwrap_or(300.0).max(1.0)
                        * 1000.0) as u64;
            }
        }
        out.takeover = Some(plan);
    }
    fn combat_items(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        t: &GameEntity,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        let npc = t.category == EntityCategory::Npc;
        let mut options = o.clone();
        options["allowed"] = o[if npc {
            "allowed_npc"
        } else {
            "allowed_players"
        }]
        .clone();
        let minimum = number(o, "minimum", 1.0);
        let default = text(o, "default").unwrap_or("");
        let enabled = boolean(o, if npc { "npc" } else { "players" }, true)
            || hp(h).is_some_and(|p| p <= 0.5)
            || effect(t, "BEST_AMMO");
        let mut candidates = Vec::<&str>::new();
        if enabled && !protected(t) {
            let sab = !effect(t, "NO_SAB")
                && shield(h).is_some_and(|p| p < number(o, "sab_shield", 0.5))
                && t.shield
                    .is_some_and(|p| p > number(o, "sab_target_shield", 10000.0));
            let infection = !effect(t, "INFECTION")
                && (t.health.is_some_and(|p| p >= 100000.0)
                    || field_num(h, "speed")
                        .zip(field_num(t, "speed"))
                        .is_some_and(|(h, t)| h <= t));
            match self.id {
                "best_ammo" => {
                    if boolean(o, "respect_npc_ammo", true) && npc {
                        if let Some(ammo) = text(&Value::Object(t.fields.clone()), "preferredAmmo")
                        {
                            let ammo = ammo.to_owned();
                            if ready(s, &ammo, minimum, true) {
                                out.commands.push(GameCommand::UseItem {
                                    item_id: ammo,
                                    activate: true,
                                });
                                return;
                            }
                        }
                    }
                    if !boolean(o, "only_prometheus", false)
                        || effect(h, "PROMETHEUS")
                        || effect(h, "98")
                    {
                        if !boolean(o, "respect_rsb_tag", false) || !npc || effect(t, "USE_RSB") {
                            candidates
                                .extend(["ammunition_laser_rsb-75", "ammunition_laser_rcb-140"]);
                        }
                        if infection {
                            candidates.push("ammunition_laser_pib-100");
                        }
                        if sab {
                            candidates
                                .extend(["ammunition_laser_cbo-100", "ammunition_laser_sab-50"]);
                        }
                        if npc {
                            let name = t.name.to_lowercase();
                            if ["lanatum", "styxus", "charopos"]
                                .iter()
                                .any(|n| name.contains(n))
                            {
                                candidates.push("ammunition_laser_vb-142");
                            }
                            if name.contains("invoke xvi") || name.contains("mindfire behemoth") {
                                candidates.push("ammunition_laser_a-bl");
                            }
                            if name
                                .replace('1', "i")
                                .replace('3', "e")
                                .replace('5', "s")
                                .contains("mimesis")
                            {
                                candidates.push("ammunition_laser_emaa-20");
                            }
                            if name.contains("demaner") {
                                candidates.push("ammunition_laser_rb-214");
                            }
                            if name.contains("sibelon") {
                                candidates.push("ammunition_laser_sbl-100");
                            }
                            candidates.extend([
                                "ammunition_laser_ucb-100",
                                "ammunition_laser_job-100",
                                "ammunition_laser_mcb-50",
                                "ammunition_laser_mcb-25",
                                "ammunition_laser_lcb-10",
                            ]);
                        } else {
                            candidates.extend([
                                "ammunition_laser_idb-125",
                                "ammunition_laser_ucb-100",
                                "ammunition_laser_sbl-100",
                                "ammunition_laser_a-bl",
                                "ammunition_laser_emaa-20",
                                "ammunition_laser_vb-142",
                                "ammunition_laser_mcb-50",
                                "ammunition_laser_mcb-25",
                                "ammunition_laser_lcb-10",
                            ]);
                        }
                    }
                }
                "best_rocket" => {
                    if field_num(t, "speed")
                        .zip(field_num(h, "speed"))
                        .is_some_and(|(t, h)| t > h)
                        && (distance(h, t) > 400.0 || hp(h).is_some_and(|p| p < 0.4))
                    {
                        candidates.extend([
                            "ammunition_specialammo_r-ic3",
                            "ammunition_specialammo_dcr-250",
                            "ammunition_specialammo_k-300m",
                            "ammunition_specialammo_rc-100",
                        ]);
                    }
                    if t.shield.is_some_and(|p| p > 500000.0) {
                        candidates.push("ammunition_specialammo_sr-5");
                    }
                    if flag(t, "attackingHero") && hp(h).is_some_and(|p| p < 0.5) {
                        candidates.push("ammunition_specialammo_pld-8");
                    }
                    if distance(h, t) <= 500.0 {
                        candidates.push("ammunition_rocket_agt-500");
                    }
                    if npc {
                        candidates.push("ammunition_rocket_bdr-1211");
                    }
                    candidates.extend([
                        "ammunition_specialammo_sp-100x",
                        "ammunition_rocket_plt-3030",
                        "ammunition_rocket_plt-2021",
                        "ammunition_rocket_bdr-1211",
                        "ammunition_rocket_plt-2026",
                        "ammunition_rocket_r-310",
                    ]);
                }
                "best_launcher" => {
                    if infection {
                        candidates.push("ammunition_rocketlauncher_pir-100");
                    }
                    if sab {
                        candidates.extend([
                            "ammunition_rocketlauncher_cbr",
                            "ammunition_rocketlauncher_sar-02",
                            "ammunition_rocketlauncher_sar-01",
                        ]);
                    }
                    if t.health.is_some_and(|p| p > 60000.0) {
                        candidates.push("ammunition_rocketlauncher_ubr-100");
                    }
                    candidates.extend([
                        "ammunition_rocketlauncher_hstrm-01",
                        "ammunition_rocketlauncher_bdr1212",
                        "ammunition_rocketlauncher_eco-10",
                    ]);
                }
                _ => {}
            }
        }
        candidates.push(default);
        self.pending_item = use_first(s, &candidates, &options, minimum, true, &mut out.commands);
    }
    fn formation(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        t: Option<&GameEntity>,
        attacking: bool,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        let Some(t) = t else { return };
        if !attacking
            || !boolean(
                o,
                if t.category == EntityCategory::Npc {
                    "npc"
                } else {
                    "players"
                },
                true,
            )
            || distance(h, t) > 1000.0
            || effect(h, "76")
        {
            return;
        }
        if t.category == EntityCategory::Npc
            && boolean(o, "respect_npc_formation", true)
            && t.fields
                .get("preferredFormation")
                .is_some_and(|v| !v.is_null())
        {
            return;
        }
        let copied = t.fields.get("formation").and_then(Value::as_str);
        let mut ids = Vec::new();
        if t.category != EntityCategory::Npc && boolean(o, "copy_player", false) {
            ids.extend(copied);
        }
        if t.category == EntityCategory::Npc
            && (boolean(o, "veteran", true) || effect(t, "USE_VETERAN"))
            && hp(t).is_some_and(|p| p <= 0.15)
            && t.health.is_some_and(|p| p < 200000.0)
        {
            ids.push("drone_formation_f-3d-vt");
        }
        if distance(h, t) > 600.0
            && field_num(t, "speed")
                .zip(field_num(h, "speed"))
                .is_some_and(|(t, h)| t >= h)
        {
            ids.push("drone_formation_f-3d-wl");
        }
        if t.shield.is_some_and(|p| p > 100000.0)
            && shield(t).is_some_and(|p| p > 0.5)
            && (hp(t).is_some_and(|p| p < 0.2) || h.max_shield.is_some_and(|p| p < 1000.0))
        {
            ids.extend(["drone_formation_f-09-mo", "drone_formation_f-06-da"]);
        }
        if hp(h).is_some_and(|p| p < 0.2) && h.shield.is_some_and(|p| p > 300000.0) {
            ids.push("drone_formation_f-10-cr");
        }
        if hp(h).is_some_and(|p| p < 0.7)
            && shield(h).is_some_and(|p| p < 0.1)
            && h.max_shield.is_some_and(|p| p > 50000.0)
        {
            ids.push("drone_formation_f-07-di");
        }
        if t.category == EntityCategory::Npc {
            if t.health.is_some_and(|p| p > 30000.0) && distance(h, t) <= 600.0 {
                ids.push("drone_formation_f-3d-dr");
            }
            if distance(h, t) <= 600.0 {
                ids.push("drone_formation_f-13-bt");
            }
            ids.push("drone_formation_f-12-ba");
        } else {
            ids.push("drone_formation_f-05-pi");
        }
        ids.extend([
            "drone_formation_f-3d-dr",
            "drone_formation_f-04-st",
            "drone_formation_f-06-da",
        ]);
        ids.extend(text(o, "default"));
        self.pending_item = use_first(s, &ids, o, 0.0, true, &mut out.commands);
    }
    fn ability(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        t: Option<&GameEntity>,
        attacking: bool,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        use crate::rule_support as abilities;
        let always: Vec<_> = o["always"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        use_first(s, &always, o, 0.0, true, &mut out.commands);
        let mut ids = Vec::new();
        if hp(h).is_some_and(|p| p < 0.1) {
            ids.extend_from_slice(abilities::EMERGENCY);
        }
        let group_injured = s
            .navigation
            .group
            .members
            .iter()
            .filter(|m| Some(m.map_id) == s.navigation.map_id)
            .filter_map(|m| {
                s.entities
                    .iter()
                    .find(|e| e.id.as_ref().and_then(Value::as_i64) == Some(i64::from(m.id)))
            })
            .any(|e| {
                valid(e)
                    && flag(e, "isAttacked")
                    && hp(e).is_some_and(|p| p <= number(o, "health", 0.5))
            });
        if (hp(h).is_some_and(|p| p <= number(o, "health", 0.5)) || group_injured)
            && !effect(h, "REPAIR_BOT")
        {
            // Targeted repairs require a friendly lock; never heal the enemy target.
            if t.is_some_and(|t| {
                t.category != EntityCategory::Npc
                    && (t.fields.get("isEnemy").and_then(Value::as_bool) == Some(false)
                        || s.navigation.group.members.iter().any(|m| {
                            t.id.as_ref().and_then(Value::as_i64) == Some(i64::from(m.id))
                        }))
                    && distance(h, t) < 650.0
            }) {
                ids.extend_from_slice(abilities::HEALTH_WITH_TARGET);
            }
            ids.extend_from_slice(abilities::HEALTH_WITHOUT_TARGET);
        }
        if shield(h).is_some_and(|p| p <= number(o, "health", 0.5)) {
            ids.extend_from_slice(abilities::SHIELD);
        } else if group_injured {
            ids.extend_from_slice(abilities::TANK);
        }
        let threatened = s
            .entities
            .iter()
            .any(|e| is_player(e) && flag(e, "attackingHero"));
        if threatened {
            ids.extend_from_slice(abilities::EVADE);
        }
        if let Some(t) = t.filter(|t| t.category != EntityCategory::Npc || boolean(o, "npc", true))
        {
            if distance(h, t) >= 650.0
                || field_num(t, "speed")
                    .zip(field_num(h, "speed"))
                    .is_some_and(|(t, h)| t > h)
            {
                ids.extend_from_slice(abilities::SPEED);
            }
            if attacking
                && !protected(t)
                && t.health
                    .is_some_and(|p| p >= number(o, "min_target_hp", 100000.0))
            {
                if distance(h, t) < 650.0 {
                    ids.extend_from_slice(abilities::DAMAGE_RANGE);
                }
                ids.extend_from_slice(abilities::DAMAGE);
            }
        }
        if threatened {
            ids.extend_from_slice(abilities::LAST_CHANCE);
        }
        if hp(h).is_some_and(|p| p <= number(o, "health", 0.5))
            && s.entities
                .iter()
                .any(|e| e.category == EntityCategory::Npc && flag(e, "attackingHero"))
        {
            ids.extend_from_slice(abilities::LAST_CHANCE_ORCUS);
        }
        ids.extend_from_slice(abilities::ALWAYS);
        ids.retain(|id| abilities::SUPPORTED.contains(id));
        self.pending_item = use_first(s, &ids, o, 0.0, true, &mut out.commands);
    }
    fn defense(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        now: u64,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        if flag(h, "isGG") {
            return;
        }
        let range = number(o, "range", 1500.0);
        let ignored = |e: &GameEntity| {
            o["ignored_ids"]
                .as_array()
                .is_some_and(|ids| e.id.as_ref().is_some_and(|id| ids.contains(id)))
                || self.kills.get(&key(e)).copied().unwrap_or(0) >= integer(o, "max_kills", 7)
        };
        let assists = |e: &GameEntity| {
            o["help"].as_array().is_some_and(|help| {
                help.iter()
                    .filter_map(Value::as_str)
                    .any(|kind| match kind {
                        "group" => s.navigation.group.members.iter().any(|m| {
                            e.id.as_ref().and_then(Value::as_i64) == Some(i64::from(m.id))
                        }),
                        "clan" => field_num(h, "clanId")
                            .is_some_and(|id| id > 0.0 && field_num(e, "clanId") == Some(id)),
                        "ally" => flag(e, "isAlly"),
                        "everyone" => {
                            e.fields.get("isEnemy").and_then(Value::as_bool) == Some(false)
                        }
                        _ => false,
                    })
            })
        };
        let candidate = s
            .entities
            .iter()
            .filter(|e| is_player(e) && valid(e) && !ignored(e) && distance(h, e) <= range)
            .find(|e| {
                (boolean(o, "respond_attacks", true) && flag(e, "attackingHero"))
                    || s.entities
                        .iter()
                        .filter(|ally| is_player(ally) && assists(ally))
                        .any(|ally| {
                            e.fields
                                .get("attackingTargetId")
                                .is_some_and(|id| ally.id.as_ref() == Some(id))
                                || (boolean(o, "help_attack", false)
                                    && ally
                                        .fields
                                        .get("attackingTargetId")
                                        .is_some_and(|id| e.id.as_ref() == Some(id)))
                        })
            })
            .or_else(|| {
                self.target.as_ref().and_then(|k| {
                    s.entities.iter().find(|e| {
                        key(e) == *k && valid(e) && distance(h, e) <= range && !ignored(e)
                    })
                })
            });
        if let Some(t) = candidate {
            let target_key = key(t);
            if self.target.as_ref() != Some(&target_key)
                || self
                    .last_hp
                    .zip(t.health)
                    .is_some_and(|(last, hp)| hp < last)
            {
                self.target_since = now;
            }
            self.target = Some(target_key);
            self.last_hp = t.health;
            if now.saturating_sub(self.target_since) > integer(o, "timeout_ms", 15000) {
                self.target = None;
                return;
            }
            let mut plan = wait_plan(s, "Defense: responding to attack");
            plan.controls.ship_mode = Some(ShipMode::Attack);
            let target = entity_ref(t);
            plan.selected_npc = Some(target.clone());
            plan.actions.push(ActionIntent::AttackNpc { target });
            if distance(h, t) > 600.0 {
                plan.actions.push(ActionIntent::MoveTo {
                    destination: entity_ref(t).position,
                });
            }
            plan.suspended = false;
            out.takeover = Some(plan);
        } else {
            if let Some(k) = self.target.take() {
                if s.entities
                    .iter()
                    .any(|e| key(e) == k && e.health == Some(0.0))
                {
                    *self.kills.entry(k).or_default() += 1;
                }
            }
        }
    }
    fn ambulance(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        now: u64,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        let threshold = number(o, "health", 0.5);
        let needs = |e: &GameEntity| {
            valid(e)
                && (hp(e).is_some_and(|p| p < threshold)
                    || (boolean(o, "repair_shield", false)
                        && shield(e).is_some_and(|p| p < threshold)))
        };
        let member = s
            .navigation
            .group
            .members
            .iter()
            .filter(|m| Some(m.map_id) == s.navigation.map_id)
            .find_map(|m| {
                s.entities.iter().find(|e| {
                    e.id.as_ref().and_then(Value::as_i64) == Some(i64::from(m.id))
                        && needs(e)
                        && flag(e, "isAttacked")
                })
            });
        let Some(member) = member else {
            self.target = None;
            return;
        };
        let shield_only = hp(member).is_none_or(|p| p >= threshold);
        let ids = if shield_only {
            vec!["ability_aegis_shield-repair"]
        } else {
            vec![
                "ability_aegis_hp-repair",
                "ability_aegis_repair-pod",
                "ability_solace",
                "ability_solace-plus_nano-cluster-repairer-plus",
            ]
        };
        let Some(ability) = ids.into_iter().find(|id| ready(s, id, 0.0, false)) else {
            self.target = None;
            return;
        };
        let k = key(member);
        if self.target.as_ref() != Some(&k) {
            self.target = Some(k);
            self.target_since = now;
        }
        if now.saturating_sub(self.target_since) > integer(o, "timeout_ms", 60000) {
            self.target = None;
            return;
        }
        let mut plan = wait_plan(s, "Ambulance: assisting group member");
        plan.controls.pet_enabled = Some(true);
        if distance(h, member) > 650.0 {
            plan.actions.push(ActionIntent::MoveTo {
                destination: entity_ref(member).position,
            });
        } else if ability.contains("repair-pod")
            || ability.contains("solace")
            || h.fields.get("localTargetId") == member.id.as_ref()
        {
            plan.controls.commands.push(GameCommand::UseItem {
                item_id: ability.into(),
                activate: true,
            });
        } else {
            // Select without attacking via a host action, never misuse AttackNpc on an ally.
            out.status
                .push("Ambulance requires game.select_target service to lock group member".into());
            if let Some(id) = member
                .id
                .as_ref()
                .and_then(Value::as_i64)
                .and_then(|id| i32::try_from(id).ok())
            {
                out.host_actions.push(HostAction::SelectTarget(id));
            }
        }
        out.takeover = Some(plan);
    }
    fn travel(
        &mut self,
        ctx: &RuleContext<'_>,
        s: &GameSnapshot,
        _h: &GameEntity,
        out: &mut RuleOutput,
    ) {
        let system = &s.navigation.star_system;
        let Some(current_id) = s
            .navigation
            .map_id
            .or_else(|| s.map.as_ref().and_then(|map| map.id))
            .filter(|map| *map != -1)
        else {
            return;
        };
        let Some(current_name) = system
            .map(current_id)
            .map(|map| map.name.clone())
            .or_else(|| xenomit_game_api::star_map::map_name(current_id).map(str::to_owned))
        else {
            return;
        };
        let destination = ctx.module_settings.get("working_map");
        let Some(target_id) = destination.and_then(|value| crate::resolve_map_id(value, system))
        else {
            return;
        };
        let Some(target_name) = system
            .map(target_id)
            .map(|map| map.name.clone())
            .or_else(|| xenomit_game_api::star_map::map_name(target_id).map(str::to_owned))
        else {
            return;
        };
        if current_id == target_id {
            self.reroute = false;
            return;
        }
        if current_name == "4-4" {
            self.reroute = false;
            return;
        }
        let high = |name: &str| {
            name.len() == 3
                && matches!(name.as_bytes()[0], b'1'..=b'3')
                && name.as_bytes()[1] == b'-'
                && matches!(name.as_bytes()[2], b'6'..=b'8')
        };
        let home = target_name.len() == 3
            && matches!(target_name.as_bytes()[0], b'1'..=b'3')
            && target_name.as_bytes()[1] == b'-'
            && matches!(target_name.as_bytes()[2], b'1' | b'2');
        let bl = current_name.to_lowercase().contains("bl");
        let faction = s.navigation.hero.faction;
        let trigger = if self.id == "fix_x1_travel" {
            home && (high(&current_name) || bl)
        } else {
            s.navigation.hero.level.is_some_and(|l| l < 25)
                && high(&target_name)
                && faction.is_some_and(|f| {
                    target_name.as_bytes()[0] - b'0' != f as u8
                        && (bl
                            || (high(&current_name)
                                && current_name.as_bytes()[0] - b'0' == f as u8))
                })
        };
        if trigger {
            self.reroute = true;
        }
        if !self.reroute {
            return;
        }
        let Some(via) = crate::resolve_map_id(&json!("4-4"), system) else {
            out.status
                .push("Travel rule: 4-4 missing from map catalog".into());
            return;
        };
        let travel = self.traveler.tick(s, via, ctx.now_ms);
        if let TravelState::Failed(error) = travel.state {
            out.takeover = Some(wait_plan(s, &error));
            return;
        }
        let mut plan = wait_plan(s, &travel.status);
        plan.controls.ship_mode = Some(ShipMode::Run);
        plan.controls.pet_enabled = travel.disable_pet.then_some(false);
        plan.controls.commands.extend(travel.command);
        if let Some(destination) = travel.destination {
            plan.actions.push(ActionIntent::MoveTo { destination });
        }
        out.takeover = Some(plan);
    }
    fn others(
        &mut self,
        s: &GameSnapshot,
        h: &GameEntity,
        now: u64,
        o: &Value,
        out: &mut RuleOutput,
    ) {
        if !self.due(now, 1000) {
            return;
        }
        let credits = field_num(h, "credits");
        let ko_limit = integer(o, "max_deaths_sid_ko", 0);
        if ko_limit > 0
            && self.death_count >= ko_limit
            && h.fields
                .get("sidStatus")
                .and_then(Value::as_str)
                .is_some_and(|s| s.contains("KO"))
        {
            out.host_actions.push(HostAction::Disconnect);
            out.stop = true;
        }
        if boolean(o, "auto_buy_lcb", false)
            && credits.is_some_and(|c| c >= 100000.0)
            && s.navigation
                .hero
                .items
                .get("ammunition_laser_lcb-10")
                .and_then(|i| i.quantity)
                .is_some_and(|q| q <= 10000.0)
        {
            use_first(
                s,
                &["buy_ammunition_laser_lcb-10"],
                &json!({}),
                0.0,
                false,
                &mut out.commands,
            );
        }
        if boolean(o, "auto_buy_plt", false)
            && credits.is_some_and(|c| c >= 50000.0)
            && s.navigation
                .hero
                .items
                .get("ammunition_rocket_plt-2026")
                .and_then(|i| i.quantity)
                .is_some_and(|q| q <= 1000.0)
        {
            use_first(
                s,
                &["buy_ammunition_rocket_plt-2026"],
                &json!({}),
                0.0,
                false,
                &mut out.commands,
            );
        }
        let memory = number(o, "max_memory_mb", 0.0);
        let refresh = (memory > 0.0 && field_num(h, "clientMemoryMb").is_some_and(|n| n >= memory))
            || (boolean(o, "reload_crash", false)
                && field_num(h, "pingMs").is_some_and(|n| n > 10000.0)
                && s.navigation
                    .portals
                    .iter()
                    .any(|p| distance_point(h, p.position.x, p.position.y) < 200.0));
        if refresh
            && self
                .event_times
                .get(&0)
                .is_none_or(|last| now.saturating_sub(*last) >= 120000)
        {
            out.host_actions.push(HostAction::Refresh);
            self.event_times.insert(0, now);
        }
        if let Some(tag) = text(o, "clan_tag").filter(|t| !t.is_empty()) {
            for player in s.entities.iter().filter(|e| {
                is_player(e)
                    && field_num(h, "clanId")
                        .is_some_and(|id| id > 0.0 && field_num(e, "clanId") == Some(id))
            }) {
                if let Some(id) = player.id.as_ref().and_then(Value::as_i64) {
                    out.host_actions.push(HostAction::TagPlayer {
                        id: id as i32,
                        tag: tag.into(),
                    });
                }
            }
        }
    }
}

/// Parse the supported rule condition expression subset into the engine's
/// typed JSON representation. Empty input means an unconditional match.
/// Unsupported names or malformed arguments return a validation error; they
/// are never silently treated as true.
pub fn parse_rule_condition(source: &str) -> Result<Value, String> {
    let source = source.trim();
    if source.is_empty() || source.eq_ignore_ascii_case("true") {
        return Ok(json!({}));
    }
    let open = source
        .find('(')
        .ok_or_else(|| "Expected condition as name(arguments)".to_owned())?;
    if !source.ends_with(')') {
        return Err("Condition is missing its closing ')'".into());
    }
    let name = source[..open].trim().to_ascii_lowercase().replace('_', "-");
    let args = split_condition_args(&source[open + 1..source.len() - 1])?;
    let one = || {
        if args.len() == 1 {
            Ok(args[0].as_str())
        } else {
            Err(format!("{name} expects one argument"))
        }
    };
    let two = || {
        if args.len() == 2 {
            Ok((args[0].as_str(), args[1].as_str()))
        } else {
            Err(format!("{name} expects two arguments"))
        }
    };
    match name.as_str() {
        "all" | "any" | "none" => {
            let parsed = args
                .iter()
                .map(|arg| parse_rule_condition(arg))
                .collect::<Result<Vec<_>, _>>()?;
            let key = name.replace('-', "_");
            Ok(json!({ (key): parsed }))
        }
        "not" => Ok(json!({"not": parse_rule_condition(one()?)?})),
        "hp-below" | "shield-below" => {
            let value = parse_condition_number(one()?)?;
            if !(0.0..=1.0).contains(&value) {
                return Err(format!("{name} expects a fraction between 0 and 1"));
            }
            let key = name.replace('-', "_");
            Ok(json!({ (key): value }))
        }
        "attacking" => {
            let value = one()?
                .parse::<bool>()
                .map_err(|_| "attacking expects true or false")?;
            Ok(json!({"attacking": value}))
        }
        "map" => {
            let raw = one()?.trim().trim_matches(['"', '\'']);
            let id = raw
                .parse::<i64>()
                .ok()
                .or_else(|| {
                    xenomit_game_api::star_map::MAPS
                        .iter()
                        .find(|(_, name, _)| name.eq_ignore_ascii_case(raw))
                        .map(|(id, _, _)| i64::from(*id))
                })
                .ok_or_else(|| "map expects a known map id or name".to_owned())?;
            Ok(json!({"map": id}))
        }
        "map-time" | "elapsed" => {
            let seconds = parse_condition_number(one()?)?;
            if seconds < 0.0 || seconds > u64::MAX as f64 / 1000.0 {
                return Err(format!("{name} expects a non-negative duration in seconds"));
            }
            let key = if name == "map-time" {
                "map_time_ms"
            } else {
                "elapsed_ms"
            };
            Ok(json!({key: (seconds * 1000.0).round() as u64}))
        }
        "deaths" => Ok(json!({"deaths": parse_condition_u64(one()?)?})),
        "target-name" | "key-empty" => {
            let value = parse_condition_string(one()?);
            let key = if name == "target-name" {
                "target_name"
            } else {
                "key_empty"
            };
            Ok(json!({key: value}))
        }
        "field" => {
            let (key, value) = two()?;
            let key = parse_condition_string(key);
            let value = parse_condition_scalar(value)?;
            Ok(json!({"field": {(key): value}}))
        }
        "npc-kills" | "resources" => {
            let (key, count) = two()?;
            let key = parse_condition_string(key);
            let count = parse_condition_u64(count)?;
            let field = if name == "npc-kills" {
                "npc_kills"
            } else {
                "resources"
            };
            Ok(json!({(field): {(key): count}}))
        }
        "time" => {
            let (hour, minute) = two()?;
            let hour = parse_condition_u64(hour)?;
            let minute = parse_condition_u64(minute)?;
            if hour > 23 || minute > 59 {
                return Err("time expects an hour from 0-23 and minute from 0-59".into());
            }
            Ok(json!({"time": {"hour": hour, "minute": minute}}))
        }
        _ => Err(format!("Unsupported rule condition '{name}'")),
    }
}

/// Parse the supported Extra Actions syntax into an existing typed command or
/// host action. The returned shape can be stored directly in a custom-event
/// row and is validated again by the behavior registry at runtime.
pub fn parse_rule_action(source: &str) -> Result<Value, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("Choose a supported action".into());
    }
    let (name, args) = if let Some(open) = source.find('(') {
        if !source.ends_with(')') {
            return Err("Action is missing its closing ')'".into());
        }
        (
            source[..open].trim().to_ascii_lowercase().replace('_', "-"),
            split_condition_args(&source[open + 1..source.len() - 1])?,
        )
    } else {
        (source.to_ascii_lowercase().replace('_', "-"), Vec::new())
    };
    let arg = |index: usize| {
        args.get(index)
            .map(String::as_str)
            .ok_or_else(|| format!("{name} is missing argument {}", index + 1))
    };
    let exact = |count: usize| {
        if args.len() == count {
            Ok(())
        } else {
            Err(format!("{name} expects {count} argument(s)"))
        }
    };
    let parse_int = |index: usize| {
        parse_condition_u64(arg(index)?)
            .and_then(|n| i64::try_from(n).map_err(|_| "Integer is too large".into()))
    };
    let command = match name.as_str() {
        "use-item" => {
            if !(1..=2).contains(&args.len()) {
                return Err("use-item expects item-id and optional activate flag".into());
            }
            let item_id = parse_condition_string(arg(0)?);
            let activate = if let Some(value) = args.get(1) {
                value
                    .parse::<bool>()
                    .map_err(|_| "use-item activate must be true or false")?
            } else {
                true
            };
            json!({"command":"use_item","item_id":item_id,"activate":activate})
        }
        "refine-ore" | "sell-ore" => {
            exact(2)?;
            let ore_type = parse_int(0)?;
            let amount = parse_condition_u64(arg(1)?)?;
            if name == "refine-ore" {
                json!({"command":"refine_ore","ore_type":ore_type,"amount":amount})
            } else {
                json!({"command":"sell_ore","ore_type":ore_type,"amount":amount})
            }
        }
        "jump-portal" => {
            exact(1)?;
            json!({"command":"jump_portal","portal_id":parse_int(0)?})
        }
        "set-pet" => {
            exact(1)?;
            let enabled = arg(0)?
                .parse::<bool>()
                .map_err(|_| "set-pet expects true or false")?;
            json!({"command":"set_pet","enabled":enabled})
        }
        "select-configuration" => {
            exact(1)?;
            let configuration = parse_int(0)?;
            if !(1..=2).contains(&configuration) {
                return Err("configuration must be 1 or 2".into());
            }
            json!({"command":"select_configuration","configuration":configuration})
        }
        "select-pet-gear" => {
            if !(1..=2).contains(&args.len()) {
                return Err("select-pet-gear expects gear-type and optional target".into());
            }
            json!({"command":"select_pet_gear","gear_type":parse_int(0)?,"target":if args.len()==2 {parse_int(1)?} else {0}})
        }
        "accept-group-invite" => {
            exact(1)?;
            json!({"command":"accept_group_invite","inviter_id":parse_int(0)?})
        }
        "invite-group-member" => {
            exact(1)?;
            json!({"command":"invite_group_member","player_id":parse_int(0)?})
        }
        "kick-group-member" => {
            exact(1)?;
            json!({"command":"kick_group_member","player_id":parse_int(0)?})
        }
        "leave-group" => {
            exact(0)?;
            json!({"command":"leave_group"})
        }
        "set-group-invites" => {
            exact(1)?;
            let open = arg(0)?
                .parse::<bool>()
                .map_err(|_| "set-group-invites expects true or false")?;
            json!({"command":"set_group_invites","open":open})
        }
        "refresh" => {
            exact(0)?;
            return Ok(json!({"host_action":"refresh"}));
        }
        "disconnect" => {
            exact(0)?;
            return Ok(json!({"host_action":"disconnect"}));
        }
        "close" => {
            exact(0)?;
            return Ok(json!({"host_action":"close"}));
        }
        "profile" => {
            exact(1)?;
            return Ok(json!({"host_action":"profile","name":parse_condition_string(arg(0)?)}));
        }
        "accept-urgent-quest" => {
            exact(0)?;
            return Ok(json!({"host_action":"accept_urgent_quest"}));
        }
        "claim-calendar" => {
            exact(0)?;
            return Ok(json!({"host_action":"claim_calendar"}));
        }
        _ => return Err(format!("Unsupported Extra Action '{name}'")),
    };
    let _: GameCommand = serde_json::from_value(command.clone())
        .map_err(|e| format!("Invalid typed action: {e}"))?;
    Ok(json!({"command":command}))
}

fn split_condition_args(source: &str) -> Result<Vec<String>, String> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let (mut args, mut start, mut depth, mut quote, mut escaped) =
        (Vec::new(), 0usize, 0i32, None, false);
    for (index, ch) in source.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote.is_some() {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if ch == active {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                args.push(source[start..index].trim().to_owned());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
        if depth < 0 {
            return Err("Unexpected ')' in condition".into());
        }
    }
    if quote.is_some() || depth != 0 {
        return Err("Unclosed string or nested condition".into());
    }
    args.push(source[start..].trim().to_owned());
    if args.iter().any(String::is_empty) {
        return Err("Condition arguments cannot be empty".into());
    }
    Ok(args)
}

fn parse_condition_string(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let first = value.as_bytes()[0] as char;
        if (first == '"' || first == '\'') && value.ends_with(first) {
            if first == '"' {
                return serde_json::from_str(value)
                    .unwrap_or_else(|_| value[1..value.len() - 1].into());
            }
            return value[1..value.len() - 1]
                .replace("\\'", "'")
                .replace("\\\\", "\\");
        }
    }
    value.to_owned()
}

fn parse_condition_scalar(value: &str) -> Result<Value, String> {
    let value = value.trim();
    if value.starts_with('"') {
        return serde_json::from_str(value).map_err(|_| "Invalid quoted string".into());
    }
    if value.starts_with('\'') {
        return Ok(json!(parse_condition_string(value)));
    }
    serde_json::from_str(value)
        .map_err(|_| "field value must be a quoted string, number, or boolean".into())
}
fn parse_condition_number(value: &str) -> Result<f64, String> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|n| n.is_finite())
        .ok_or_else(|| "Expected a finite number".into())
}
fn parse_condition_u64(value: &str) -> Result<u64, String> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| "Expected a non-negative whole number".into())
}
