//! Retained, cooperative module instances and an immutable factory registry.
use crate::{
    BehaviorDecision, CancellationToken, OperationHost, RestoreError,
    checkpoint::{ModuleState, settings_fingerprint},
};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};
use xenomit_game_api::{GameSnapshot, ModuleConfig, ModuleDescriptor};

pub mod cubikon;
#[path = "modules/game_modes.rs"]
pub(crate) mod game_modes;
pub mod item_restock;
pub mod presets;
pub mod skylab;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterruptionReason {
    ClientRefresh,
    /// The runtime is draining for a bot handoff. Modules should report
    /// `Ready` at a durable state boundary so a clean checkpoint can be taken;
    /// `TaskSequence` resumes by step name on the successor bot.
    Handoff,
}

pub struct InterruptionContext<'a> {
    pub now_ms: u64,
    pub snapshot: Option<&'a GameSnapshot>,
    pub game_ready: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InterruptionStatus {
    Ready,
    Deferred(String),
}

pub struct TickContext<'a> {
    /// Monotonic run time; independent of snapshot timestamps and bridge loss.
    pub now_ms: u64,
    pub snapshot: Option<&'a GameSnapshot>,
    pub game_ready: bool,
    pub cancellation: &'a CancellationToken,
    pub operations: &'a mut dyn OperationHost,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RunState {
    Running,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Debug)]
pub struct ModuleOutput {
    pub state: RunState,
    pub status: String,
    /// Optional existing combat/collection plan. Other commands use operations
    /// so their completion/failure is observed before sequencing advances.
    pub plan: Option<BehaviorDecision>,
}
impl ModuleOutput {
    pub fn running(status: impl Into<String>) -> Self {
        Self {
            state: RunState::Running,
            status: status.into(),
            plan: None,
        }
    }
    pub fn completed(status: impl Into<String>) -> Self {
        Self {
            state: RunState::Completed,
            status: status.into(),
            plan: None,
        }
    }
    pub fn failed(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            state: RunState::Failed(error.clone()),
            status: error,
            plan: None,
        }
    }
}

/// Implement a state machine with ordinary Rust fields. tick must return
/// promptly: use operations, TaskSequence, or wait state rather than sleeping.
pub trait BehaviorModule: Send {
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput;
    /// Whether a pending host interruption can safely take effect now.
    /// Implementations must opt in explicitly; unknown modules defer.
    fn interruption_status(
        &self,
        _context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        InterruptionStatus::Deferred("Module has not declared a safe interruption point".into())
    }
    /// True while this module performs work that cannot resume after a kill —
    /// purchases, gate sessions, or multi-message operations. Such work may
    /// pause a drain's countdown but never blocks the hard deadline.
    fn non_interruptible(&self) -> bool {
        false
    }
    /// Export retained state at the current step boundary. Timestamps are
    /// stored relative to `now_ms`. `best_effort` marks degraded snapshots
    /// taken outside a safe point; returning `None` means fresh restart.
    fn export_state(&self, _now_ms: u64, _best_effort: bool) -> Option<ModuleState> {
        None
    }
    /// Restore a previously exported state blob. Missing step names, module
    /// ids, or operation handles must fail; unknown fields are ignored.
    fn restore_state(&mut self, _state: &ModuleState, _now_ms: u64) -> Result<(), RestoreError> {
        Err(RestoreError::ShapeMismatch(
            "module does not support checkpoint restore".into(),
        ))
    }
    /// Update this same instance; never reset a sequence just to edit settings.
    fn update_settings(&mut self, _settings: &Value) -> Result<(), String> {
        Err("This module's settings can only be changed while stopped".into())
    }
    fn plan_failed(&mut self, _plan: &BehaviorDecision) {}
    /// Called only after all requested controls/actions were accepted.
    fn plan_succeeded(&mut self, _plan: &BehaviorDecision, _now_ms: u64) {}
    fn command_succeeded(&mut self, _command: &xenomit_game_api::GameCommand, _now_ms: u64) {}
}

type Factory = dyn Fn(&Value) -> Result<Box<dyn BehaviorModule>, String> + Send + Sync;
#[derive(Clone)]
struct Registration {
    descriptor: ModuleDescriptor,
    factory: Arc<Factory>,
}

#[derive(Clone, Default)]
pub struct ModuleRegistryBuilder {
    modules: BTreeMap<String, Registration>,
}
impl ModuleRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register<F>(
        &mut self,
        descriptor: ModuleDescriptor,
        factory: F,
    ) -> Result<&mut Self, String>
    where
        F: Fn(&Value) -> Result<Box<dyn BehaviorModule>, String> + Send + Sync + 'static,
    {
        if descriptor.id.is_empty() || descriptor.name.is_empty() {
            return Err("module ID and name must not be empty".into());
        }
        if self.modules.contains_key(&descriptor.id) {
            return Err(format!("duplicate module ID {}", descriptor.id));
        }
        let mut keys = Vec::<String>::new();
        for field in &descriptor.settings {
            if field.key.split('.').any(str::is_empty)
                || keys.iter().any(|key| {
                    key == &field.key
                        || key.starts_with(&format!("{}.", field.key))
                        || field.key.starts_with(&format!("{key}."))
                })
            {
                return Err(format!("overlapping/invalid setting path {}", field.key));
            }
            field.validate(&field.default)?;
            keys.push(field.key.clone());
        }
        descriptor.prepare_settings(serde_json::json!({}))?;
        self.modules.insert(
            descriptor.id.clone(),
            Registration {
                descriptor,
                factory: Arc::new(factory),
            },
        );
        Ok(self)
    }
    /// Consumes the builder. Registry contents cannot change after attachment.
    pub fn build(self) -> ModuleRegistry {
        ModuleRegistry {
            modules: self.modules,
        }
    }
}
#[derive(Clone)]
pub struct ModuleRegistry {
    modules: BTreeMap<String, Registration>,
}
impl ModuleRegistry {
    pub fn into_builder(self) -> ModuleRegistryBuilder {
        ModuleRegistryBuilder {
            modules: self.modules,
        }
    }
    pub fn filtered(&self, keep: impl Fn(&str) -> bool) -> Self {
        Self {
            modules: self
                .modules
                .iter()
                .filter(|(id, _)| keep(id))
                .map(|(id, r)| (id.clone(), r.clone()))
                .collect(),
        }
    }
    pub fn descriptors(&self) -> Vec<ModuleDescriptor> {
        self.modules
            .values()
            .map(|m| m.descriptor.clone())
            .collect()
    }
    pub fn create(
        &self,
        config: ModuleConfig,
        cancellation: CancellationToken,
    ) -> Result<ModuleRunner, String> {
        let registration = self
            .modules
            .get(&config.module_id)
            .ok_or_else(|| format!("unknown module {}", config.module_id))?;
        let settings = registration.descriptor.prepare_settings(config.settings)?;
        let module = (registration.factory)(&settings)?;
        Ok(ModuleRunner {
            descriptor: registration.descriptor.clone(),
            settings,
            module,
            cancellation,
            terminal: None,
        })
    }
}

/// One selected module for one run. Changing IDs requires stop + a new run.
pub struct ModuleRunner {
    descriptor: ModuleDescriptor,
    settings: Value,
    module: Box<dyn BehaviorModule>,
    cancellation: CancellationToken,
    terminal: Option<ModuleOutput>,
}
impl ModuleRunner {
    pub fn descriptor(&self) -> &ModuleDescriptor {
        &self.descriptor
    }
    pub fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        reason: InterruptionReason,
    ) -> InterruptionStatus {
        if self.terminal.is_some() {
            InterruptionStatus::Ready
        } else {
            self.module.interruption_status(context, reason)
        }
    }
    /// True while the running module performs work that cannot resume after
    /// a kill. A terminal module is never non-interruptible.
    pub fn non_interruptible(&self) -> bool {
        self.terminal.is_none() && self.module.non_interruptible()
    }
    /// Export the module's checkpoint, stamped with the prepared settings
    /// fingerprint so a successor can reject incompatible state.
    pub fn export_state(&self, now_ms: u64, best_effort: bool) -> Option<ModuleState> {
        let mut state = self.module.export_state(now_ms, best_effort)?;
        state.settings_fingerprint = Some(settings_fingerprint(&self.settings));
        Some(state)
    }
    /// Restore module state. Clean checkpoints require an exact settings
    /// fingerprint; degraded blobs tolerate a missing one but never a wrong
    /// one. Callers decide whether a failure aborts the restore (clean) or
    /// starts fresh (degraded).
    pub fn restore_state(&mut self, state: &ModuleState, now_ms: u64) -> Result<(), RestoreError> {
        state.check_schema()?;
        let fingerprint = settings_fingerprint(&self.settings);
        match &state.settings_fingerprint {
            Some(found) if found == &fingerprint => {}
            Some(_) => return Err(RestoreError::SettingsMismatch),
            None if !state.degraded => return Err(RestoreError::MissingField("settings_fingerprint")),
            None => {}
        }
        self.module.restore_state(state, now_ms)
    }
    pub fn update(&mut self, config: &ModuleConfig) -> Result<(), String> {
        if config.module_id != self.descriptor.id {
            return Err("Stop the running module before changing modules".into());
        }
        let settings = self.descriptor.prepare_settings(config.settings.clone())?;
        if settings != self.settings {
            self.module.update_settings(&settings)?;
            self.settings = settings;
        }
        Ok(())
    }
    pub fn tick(
        &mut self,
        now_ms: u64,
        snapshot: Option<&GameSnapshot>,
        game_ready: bool,
        operations: &mut dyn OperationHost,
    ) -> ModuleOutput {
        if let Some(output) = &self.terminal {
            return output.clone();
        }
        let output = if self.cancellation.is_cancelled() {
            ModuleOutput {
                state: RunState::Cancelled,
                status: "Cancelled".into(),
                plan: None,
            }
        } else {
            self.module.tick(&mut TickContext {
                now_ms,
                snapshot,
                game_ready,
                cancellation: &self.cancellation,
                operations,
            })
        };
        if output.state != RunState::Running {
            self.terminal = Some(output.clone());
            self.cancellation.cancel();
        }
        output
    }
    pub fn plan_failed(&mut self, plan: &BehaviorDecision) {
        self.module.plan_failed(plan);
    }
    pub fn plan_succeeded(&mut self, plan: &BehaviorDecision, now_ms: u64) {
        self.module.plan_succeeded(plan, now_ms);
    }
    pub fn command_succeeded(&mut self, command: &xenomit_game_api::GameCommand, now_ms: u64) {
        self.module.command_succeeded(command, now_ms);
    }
}
impl Drop for ModuleRunner {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod interruption_tests {
    use super::*;

    struct Unspecified;
    impl BehaviorModule for Unspecified {
        fn tick(&mut self, _context: &mut TickContext<'_>) -> ModuleOutput {
            ModuleOutput::running("test")
        }
    }

    #[test]
    fn unspecified_modules_defer() {
        let module = Unspecified;
        let context = InterruptionContext {
            now_ms: 0,
            snapshot: None,
            game_ready: false,
        };
        assert!(matches!(
            module.interruption_status(&context, InterruptionReason::ClientRefresh),
            InterruptionStatus::Deferred(_)
        ));
    }
}
