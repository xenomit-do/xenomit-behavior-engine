//! Configurable game behavior planning for clients that expose
//! [`xenomit_game_api::GameSnapshot`]s.
//!
//! The engine does not connect to a client or issue reflected calls. Pass each
//! snapshot to [`BehaviorEngine::step`] and execute the returned typed
//! [`ActionIntent`]s in the application that owns the `GameClient`.

/// Crate version, embedded in snapshot `writer.engine` provenance fields.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[path = "modules/builtins.rs"]
mod builtins;
pub mod checkpoint;
pub mod interruption;
pub mod plugins;
mod shared_spaceball;
pub use plugins::{PLUGIN_FORMAT_VERSION, PluginCatalog, PluginManifest, PluginSelection};
mod config;
mod context;
mod engine;
mod refresh;
mod rule_support;
pub mod rules;
pub mod shared;
pub use rules::{
    BehaviorRegistry, BehaviorRunner, GeneralBehavior, HostAction, RuleContext, RuleOutput,
    behavior_settings_descriptor, builtin_behavior_registry, filter_booty_boxes,
    condition_matches_snapshot, parse_rule_action, parse_rule_condition,
};
pub mod modules;
pub mod operations;
pub mod pathfinding;
pub use pathfinding::{MovementRouter, ObstacleNavigation, PATH_CLEARANCE};
pub mod safety;
pub mod sequence;
pub use safety::{SafetyCheckpoint, SafetyConfig, SafetyManager, SafetyOutcome};
pub mod travel;
pub use travel::{
    MapTraveler, TravelDecision, TravelState, TravelerCheckpoint, resolve_map_id,
    shortest_map_route,
};

pub use builtins::{builtin_behavior_config, builtin_registry, builtin_registry_builder};
pub use checkpoint::{
    CHECKPOINT_SCHEMA, MIN_SUPPORTED_SCHEMA, ModuleState, RestoreError, SequenceCheckpoint,
    TerminalOutput, TerminalState, age_ms, rebase_age, rebase_deadline, remaining_ms,
    settings_fingerprint,
};
pub use config::{
    AvoidanceConfig, BehaviorConfig, BehaviorMode, BoxRule, CollectorConfig, CoordinateTransform,
    EntitySelector, FormationConfig, KillAndCollectConfig, NpcFlag, NpcKillerConfig, NpcRule,
    RoamingConfig,
};
pub use context::{EntityObservation, ModuleContext, Navigation};
pub use engine::{
    ActionIntent, BehaviorDecision, BehaviorEngine, CollectionWait, ControlRequests,
    EngineCheckpoint, EntityRef, Point, ShipMode, StepError,
};
pub use modules::presets::{
    BoxPresetEntry, NpcPresetEntry, box_preset_choices, box_preset_entries, box_preset_rules,
    npc_preset_choices, npc_preset_rules,
};
pub use modules::skylab::{SkylabAction, SkylabRow, next_skylab_action, skylab_rows};
pub use modules::{
    BehaviorModule, InterruptionContext, InterruptionReason, InterruptionStatus, ModuleOutput,
    ModuleRegistry, ModuleRegistryBuilder, ModuleRunner, RunState, TickContext,
};
pub use operations::{
    CancellationToken, OperationHandle, OperationHost, OperationPoll, OperationRequest,
    ServiceContext, ServiceRegistry, ServiceRegistryBuilder, ThreadedOperationHost,
};
pub use sequence::{RetryPolicy, SequenceProgress, TaskSequence};
pub use xenomit_game_api::{ModuleDescriptor, SettingChoice, SettingDescriptor, SettingKind};
