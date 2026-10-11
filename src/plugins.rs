//! Versioned, self-contained plugin contributions and immutable activation snapshots.
use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};
use xenomit_game_api::{GameCommand, GameSnapshot, PluginDescriptor, setting_value};

pub const PLUGIN_FORMAT_VERSION: u32 = 1;
pub type PluginSelection = BTreeMap<String, bool>;

#[derive(Clone)]
pub struct PluginCatalog {
    plugins: BTreeMap<String, PluginDescriptor>,
    modules: ModuleRegistry,
    behaviors: BehaviorRegistry,
}
impl PluginCatalog {
    pub fn builtin() -> Self {
        let mut modules = builtin_registry_builder();
        crate::shared::register_shared_modules(&mut modules).expect("valid SharedPlugin modules");
        let mut behaviors = builtin_behavior_registry();
        crate::shared::register_shared_behaviors(&mut behaviors)
            .expect("valid SharedPlugin behaviors");
        let modules = modules.build();
        let mut plugins = BTreeMap::new();
        for (id, name, description) in [
            (
                "base",
                "Base",
                "Collection, NPC objectives and essential safety rules.",
            ),
            (
                "combat",
                "Combat",
                "Combat assistance, item selection and defensive rules.",
            ),
            (
                "gg",
                "Galaxy gates",
                "Galaxy gates, group gates and Eternal Blacklight.",
            ),
            ("event", "Events", "Event maps and event objectives."),
            (
                "economy",
                "Economy",
                "Skylab, resource processing and account utilities.",
            ),
            (
                "utility",
                "Utilities",
                "Travel fixes, profiles and general automation.",
            ),
        ] {
            plugins.insert(
                id.into(),
                PluginDescriptor {
                    id: id.into(),
                    name: name.into(),
                    description: description.into(),
                    default_enabled: true,
                    module_ids: vec![],
                    behavior_ids: vec![],
                    required_plugins: vec![],
                },
            );
        }
        for d in modules.descriptors() {
            plugins
                .get_mut(module_owner(&d.id))
                .expect("built-in owner")
                .module_ids
                .push(d.id);
        }
        for d in behaviors.descriptors() {
            plugins
                .get_mut(behavior_owner(&d.id))
                .expect("built-in owner")
                .behavior_ids
                .push(d.id);
        }
        Self {
            plugins,
            modules,
            behaviors,
        }
    }
    pub fn descriptors(&self) -> Vec<PluginDescriptor> {
        self.plugins.values().cloned().collect()
    }
    pub fn settings_descriptor(&self) -> ModuleDescriptor {
        let mut d = ModuleDescriptor::new("__plugins", "Plugins").description("Enable or disable bundles while the bot is stopped. Third-party plugin manifests load from the plugins directory at startup.");
        for p in self.plugins.values() {
            d.settings.push(
                SettingDescriptor::new(
                    &p.id,
                    &p.name,
                    SettingKind::Boolean,
                    json!(p.default_enabled),
                )
                .description(&p.description)
                .group("Plugins"),
            );
        }
        d
    }
    fn enabled_ids(&self, selection: &PluginSelection) -> Result<BTreeSet<String>, String> {
        for (id, enabled) in selection {
            if *enabled && !self.plugins.contains_key(id) {
                return Err(format!(
                    "Unknown enabled plugin {id}; reinstall its manifest or disable it"
                ));
            }
        }
        let enabled: BTreeSet<_> = self
            .plugins
            .values()
            .filter(|p| selection.get(&p.id).copied().unwrap_or(p.default_enabled))
            .map(|p| p.id.clone())
            .collect();
        for id in &enabled {
            for dep in &self.plugins[id].required_plugins {
                if !enabled.contains(dep) {
                    return Err(format!("Plugin {id} requires enabled plugin {dep}"));
                }
            }
        }
        Ok(enabled)
    }
    pub fn module_enabled(&self, id: &str, selection: &PluginSelection) -> Result<bool, String> {
        let enabled = self.enabled_ids(selection)?;
        Ok(self
            .plugins
            .values()
            .any(|p| enabled.contains(&p.id) && p.module_ids.iter().any(|m| m == id)))
    }
    pub fn registries(
        &self,
        selection: &PluginSelection,
    ) -> Result<(ModuleRegistry, BehaviorRegistry), String> {
        let enabled = self.enabled_ids(selection)?;
        let modules: BTreeSet<_> = self
            .plugins
            .values()
            .filter(|p| enabled.contains(&p.id))
            .flat_map(|p| p.module_ids.iter().cloned())
            .collect();
        let rules: BTreeSet<_> = self
            .plugins
            .values()
            .filter(|p| enabled.contains(&p.id))
            .flat_map(|p| p.behavior_ids.iter().cloned())
            .collect();
        Ok((
            self.modules.filtered(|id| modules.contains(id)),
            self.behaviors.filtered(|id| rules.contains(id)),
        ))
    }
    pub fn module_descriptors(
        &self,
        selection: &PluginSelection,
    ) -> Result<Vec<ModuleDescriptor>, String> {
        Ok(self.registries(selection)?.0.descriptors())
    }
    pub fn behavior_settings_descriptor(
        &self,
        selection: &PluginSelection,
    ) -> Result<ModuleDescriptor, String> {
        let mut page = ModuleDescriptor::new("__behaviors", "Behaviors").description(
            "General rules from enabled plugins, independent of the selected objective.",
        );
        for d in self.registries(selection)?.1.descriptors() {
            for mut field in d.settings {
                field.key = format!("{}.{}", d.id, field.key);
                field.group = d.name.clone();
                page.settings.push(field);
            }
        }
        Ok(page)
    }
    /// Invalid files are rejected atomically. A failed plugin cannot contribute partial factories.
    /// Dependencies resolve after all files are read, so names do not dictate load order.
    pub fn load_directory(&mut self, directory: &Path) -> Vec<String> {
        if !directory.exists() {
            return vec![];
        }
        let entries = match std::fs::read_dir(directory) {
            Ok(e) => e,
            Err(e) => return vec![format!("{}: {e}", directory.display())],
        };
        let mut files: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();
        files.sort();
        let mut errors = vec![];
        for file in files {
            let result = std::fs::metadata(&file)
                .map_err(|e| e.to_string())
                .and_then(|m| {
                    if m.len() > 1_048_576 {
                        Err("manifest exceeds 1 MiB".into())
                    } else {
                        Ok(())
                    }
                })
                .and_then(|_| std::fs::read_to_string(&file).map_err(|e| e.to_string()))
                .and_then(|text| {
                    serde_json::from_str::<PluginManifest>(&text).map_err(|e| e.to_string())
                })
                .and_then(|manifest| self.register_manifest(manifest));
            if let Err(e) = result {
                errors.push(format!("{}: {e}", file.display()));
            }
        }
        // Reject unresolved dependencies after discovery, including dependents of a rejected file.
        loop {
            let rejected: Vec<_> = self
                .plugins
                .values()
                .filter_map(|p| {
                    p.required_plugins
                        .iter()
                        .find(|dep| !self.plugins.contains_key(*dep))
                        .map(|dep| (p.id.clone(), dep.clone()))
                })
                .collect();
            if rejected.is_empty() {
                break;
            }
            for (id, dependency) in rejected {
                if let Some(p) = self.plugins.remove(&id) {
                    self.modules = self
                        .modules
                        .filtered(|m| !p.module_ids.iter().any(|id| id == m));
                    self.behaviors = self
                        .behaviors
                        .filtered(|m| !p.behavior_ids.iter().any(|id| id == m));
                    errors.push(format!(
                        "Plugin {id} rejected: missing dependency {dependency}"
                    ));
                }
            }
        }
        errors
    }
    pub fn register_manifest(&mut self, manifest: PluginManifest) -> Result<(), String> {
        manifest.validate()?;
        if self.plugins.contains_key(&manifest.id) {
            return Err(format!("Duplicate plugin {}", manifest.id));
        }
        let mut next = self.clone();
        let mut builder = next.modules.into_builder();
        let mut metadata = PluginDescriptor {
            id: manifest.id.clone(),
            name: manifest.name,
            description: manifest.description,
            default_enabled: manifest.default_enabled,
            module_ids: vec![],
            behavior_ids: vec![],
            required_plugins: manifest.required_plugins,
        };
        for contribution in manifest.modules {
            let mut descriptor = contribution.descriptor;
            descriptor.id = format!("{}:{}", manifest.id, descriptor.id);
            for step in contribution.implementation.services() {
                if !descriptor.required_services.contains(&step) {
                    descriptor.required_services.push(step);
                }
            }
            metadata.module_ids.push(descriptor.id.clone());
            builder.register(descriptor, move |values| {
                contribution.implementation.instantiate(values)
            })?;
        }
        for contribution in manifest.behaviors {
            let mut descriptor = contribution.descriptor.clone();
            descriptor.id = format!("{}:{}", manifest.id, descriptor.id);
            if descriptor
                .settings
                .iter()
                .any(|s| matches!(s.key.as_str(), "enabled" | "tick_stopped"))
            {
                return Err(
                    "behavior enabled/tick_stopped settings are supplied by the plugin format"
                        .into(),
                );
            }
            descriptor.settings.push(SettingDescriptor::new(
                "enabled",
                "Enabled",
                SettingKind::Boolean,
                json!(false),
            ));
            descriptor.settings.push(SettingDescriptor::new(
                "tick_stopped",
                "Run while paused",
                SettingKind::Boolean,
                json!(false),
            ));
            metadata.behavior_ids.push(descriptor.id.clone());
            next.behaviors
                .register(contribution.priority, descriptor, move |values| {
                    Ok(Box::new(ManifestRule {
                        spec: contribution.clone(),
                        settings: values.clone(),
                        last_sent: None,
                    }))
                })?;
        }
        next.modules = builder.build();
        next.plugins.insert(metadata.id.clone(), metadata);
        next.validate_dependencies()?;
        *self = next;
        Ok(())
    }
    fn validate_dependencies(&self) -> Result<(), String> {
        fn visit(
            id: &str,
            catalog: &PluginCatalog,
            stack: &mut BTreeSet<String>,
            done: &mut BTreeSet<String>,
        ) -> Result<(), String> {
            if done.contains(id) {
                return Ok(());
            }
            if !stack.insert(id.into()) {
                return Err(format!("Cyclic plugin dependency at {id}"));
            }
            if let Some(p) = catalog.plugins.get(id) {
                for dep in &p.required_plugins {
                    visit(dep, catalog, stack, done)?;
                }
            }
            stack.remove(id);
            done.insert(id.into());
            Ok(())
        }
        let mut done = BTreeSet::new();
        for id in self.plugins.keys() {
            visit(id, self, &mut BTreeSet::new(), &mut done)?;
        }
        Ok(())
    }
}
fn module_owner(id: &str) -> &'static str {
    match id {
        "galaxy" | "gop" | "low" | "hades" | "eternal_blacklight" | "shared_simple_galaxy_gate" => {
            "gg"
        }
        "treacherous_domain" | "chaos_spawner" | "shared_spaceball" => "event",
        "skylab" | "palladium" => "economy",
        "follow" => "utility",
        "blacklight_farmer" => "combat",
        _ => "base",
    }
}
fn behavior_owner(id: &str) -> &'static str {
    match id {
        "refresh" | "anti_push" | "stop_button" | "captcha_picker" | "booty_keys" => "base",
        "anti_train"
        | "ambulance"
        | "defense"
        | "auto_cloak"
        | "best_ammo"
        | "best_rocket"
        | "best_launcher"
        | "best_formation"
        | "best_ability"
        | "shared_simple_healing"
        | "shared_solaris_inc"
        | "shared_crowd_avoidance"
        | "shared_repair_pet" => "combat",
        "shared_gg_alert_closer" => "gg",
        "calendar_bonus" | "shared_auto_refine" | "shared_ore_seller" | "shared_autobuy" => {
            "economy"
        }
        _ => "utility",
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    pub format_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub default_enabled: bool,
    #[serde(default)]
    pub required_plugins: Vec<String>,
    #[serde(default)]
    pub modules: Vec<ManifestModule>,
    #[serde(default)]
    pub behaviors: Vec<ManifestBehavior>,
}
impl PluginManifest {
    fn validate(&self) -> Result<(), String> {
        if self.format_version != PLUGIN_FORMAT_VERSION {
            return Err(format!("Unsupported plugin format {}", self.format_version));
        }
        valid_id(&self.id)?;
        if self.name.trim().is_empty() {
            return Err("Plugin name is empty".into());
        }
        for dep in &self.required_plugins {
            valid_id(dep)?;
            if dep == &self.id {
                return Err("Plugin cannot depend on itself".into());
            }
        }
        if self.modules.len() + self.behaviors.len() > 256 {
            return Err("Too many plugin contributions".into());
        }
        for m in &self.modules {
            valid_id(&m.descriptor.id)?;
            m.implementation
                .instantiate(&m.descriptor.prepare_settings(json!({}))?)?;
        }
        for b in &self.behaviors {
            valid_id(&b.descriptor.id)?;
            b.condition.validate()?;
            if b.cooldown_ms == 0 {
                return Err("Behavior cooldown must be positive".into());
            }
            if b.commands.is_empty() {
                return Err("Behavior must supply a command".into());
            }
        }
        Ok(())
    }
}
fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(format!("Invalid plugin/local contribution ID {id}"));
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestModule {
    pub descriptor: ModuleDescriptor,
    pub implementation: ModuleImplementation,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModuleImplementation {
    Sequence { steps: Vec<ManifestStep> },
    Planner { config: Box<BehaviorConfig> },
}
impl ModuleImplementation {
    fn services(&self) -> Vec<String> {
        match self {
            Self::Planner { .. } => vec![],
            Self::Sequence { steps } => steps
                .iter()
                .filter_map(|s| match s {
                    ManifestStep::Service { service, .. } => Some(service.clone()),
                    ManifestStep::Command { .. } => Some("game.command".into()),
                    _ => None,
                })
                .collect(),
        }
    }
    fn instantiate(&self, settings: &Value) -> Result<Box<dyn BehaviorModule>, String> {
        match self {
            Self::Planner { config } => Ok(Box::new(ManifestPlanner(
                BehaviorEngine::new(config.as_ref().clone()).map_err(|e| e.to_string())?,
            ))),
            Self::Sequence { steps } => {
                if steps.is_empty() || steps.len() > 1024 {
                    return Err("Sequence requires 1..1024 steps".into());
                }
                let mut sequence = TaskSequence::new();
                for (i, step) in steps.iter().enumerate() {
                    let name = format!("step-{i}");
                    match step {
                        ManifestStep::Delay { duration_ms } => {
                            if *duration_ms == 0 {
                                return Err("Delay must be positive".into());
                            }
                            sequence = sequence.delay(name, Duration::from_millis(*duration_ms));
                        }
                        ManifestStep::Command {
                            command,
                            timeout_ms,
                        } => {
                            sequence = sequence.operation(
                                name,
                                OperationRequest::game_command(command.clone()),
                                Duration::from_millis(*timeout_ms),
                                RetryPolicy::default(),
                            )?;
                        }
                        ManifestStep::Service {
                            service,
                            input,
                            requires_game,
                            timeout_ms,
                        } => {
                            if service.trim().is_empty() {
                                return Err("Service name is empty".into());
                            }
                            let mut request = OperationRequest::service(
                                service,
                                expand_settings(input, settings)?,
                            );
                            request.requires_game = *requires_game;
                            sequence = sequence.operation(
                                name,
                                request,
                                Duration::from_millis(*timeout_ms),
                                RetryPolicy::default(),
                            )?;
                        }
                    }
                }
                Ok(Box::new(sequence))
            }
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestStep {
    Delay {
        duration_ms: u64,
    },
    Command {
        command: GameCommand,
        timeout_ms: u64,
    },
    Service {
        service: String,
        #[serde(default)]
        input: Value,
        #[serde(default)]
        requires_game: bool,
        timeout_ms: u64,
    },
}
fn expand_settings(value: &Value, settings: &Value) -> Result<Value, String> {
    if let Some(object) = value.as_object() {
        if object.len() == 1 {
            if let Some(key) = object.get("$setting").and_then(Value::as_str) {
                return setting_value(settings, key)
                    .cloned()
                    .ok_or_else(|| format!("Missing referenced setting {key}"));
            }
        }
        return object
            .iter()
            .map(|(k, v)| Ok((k.clone(), expand_settings(v, settings)?)))
            .collect::<Result<serde_json::Map<_, _>, String>>()
            .map(Value::Object);
    }
    if let Some(array) = value.as_array() {
        return array
            .iter()
            .map(|v| expand_settings(v, settings))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array);
    }
    Ok(value.clone())
}
struct ManifestPlanner(BehaviorEngine);
impl BehaviorModule for ManifestPlanner {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        crate::interruption::safe_idle_status(context)
    }
    fn export_state(&self, now_ms: u64, _best_effort: bool) -> Option<crate::ModuleState> {
        let data = serde_json::to_value(self.0.export_checkpoint(now_ms)).ok()?;
        Some(crate::ModuleState::clean(String::new(), data))
    }
    fn restore_state(
        &mut self,
        state: &crate::ModuleState,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        state.check_schema()?;
        let checkpoint: crate::EngineCheckpoint =
            serde_json::from_value(state.data.clone()).map_err(|error| {
                crate::RestoreError::Corrupt(format!("plugin planner checkpoint: {error}"))
            })?;
        self.0.restore_checkpoint(&checkpoint, now_ms)
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        let mut output = ModuleOutput::running("Plugin planner");
        if context.game_ready {
            if let Some(snapshot) = context.snapshot {
                output.plan = Some(
                    self.0
                        .step(snapshot, context.now_ms)
                        .into_world_coordinates(self.0.config().coordinates),
                );
            }
        }
        output
    }
    fn plan_failed(&mut self, plan: &BehaviorDecision) {
        if let Some(target) = &plan.selected_box {
            self.0.collection_failed(target);
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestBehavior {
    pub descriptor: ModuleDescriptor,
    pub priority: i32,
    pub condition: ManifestCondition,
    pub cooldown_ms: u64,
    pub commands: Vec<GameCommand>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ManifestCondition {
    Map { map_id: i32 },
    HeroField { key: String, equals: Value },
    All { conditions: Vec<ManifestCondition> },
    Any { conditions: Vec<ManifestCondition> },
}
impl ManifestCondition {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::HeroField { key, equals } if key.is_empty() || equals.is_null() => {
                Err("Field condition requires a key and non-null expected observation".into())
            }
            Self::All { conditions } | Self::Any { conditions } => {
                if conditions.is_empty() || conditions.len() > 64 {
                    return Err("Conditions require 1..64 entries".into());
                }
                for c in conditions {
                    c.validate()?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn matches(&self, snapshot: &GameSnapshot) -> bool {
        match self {
            Self::Map { map_id } => snapshot.navigation.map_id == Some(*map_id),
            Self::HeroField { key, equals } => {
                snapshot
                    .entities
                    .iter()
                    .find(|e| e.fields.get("isHero").and_then(Value::as_bool) == Some(true))
                    .or_else(|| {
                        snapshot.entities.iter().find(|e| {
                            e.category == xenomit_game_api::EntityCategory::Player
                                && e.fields.get("isHero").and_then(Value::as_bool) != Some(false)
                        })
                    })
                    .and_then(|e| {
                        setting_value(&Value::Object(e.fields.clone().into_iter().collect()), key)
                            .cloned()
                    })
                    .as_ref()
                    == Some(equals)
            }
            Self::All { conditions } => conditions.iter().all(|c| c.matches(snapshot)),
            Self::Any { conditions } => conditions.iter().any(|c| c.matches(snapshot)),
        }
    }
}
struct ManifestRule {
    spec: ManifestBehavior,
    settings: Value,
    last_sent: Option<u64>,
}
impl GeneralBehavior for ManifestRule {
    fn update(&mut self, settings: &Value) -> Result<(), String> {
        self.settings = settings.clone();
        Ok(())
    }
    fn tick(&mut self, context: &RuleContext<'_>) -> RuleOutput {
        let mut output = RuleOutput::default();
        if self.settings.get("enabled").and_then(Value::as_bool) != Some(true)
            || (context.stopped
                && self.settings.get("tick_stopped").and_then(Value::as_bool) != Some(true))
        {
            return output;
        }
        if self
            .last_sent
            .is_some_and(|last| context.now_ms.saturating_sub(last) < self.spec.cooldown_ms)
        {
            return output;
        }
        if context
            .snapshot
            .is_some_and(|s| self.spec.condition.matches(s))
        {
            output.commands = self.spec.commands.clone();
            self.last_sent = Some(context.now_ms);
        }
        output
    }
}
