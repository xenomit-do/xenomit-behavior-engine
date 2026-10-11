use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use xenomit_game_api::EntityCategory;

use crate::StepError;

/// Which behavior plan the engine should produce on each step.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviorMode {
    #[default]
    Collector,
    NpcKiller,
    KillAndCollect,
}

/// Complete, serializable behavior configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BehaviorConfig {
    pub mode: BehaviorMode,
    /// If set, only an entity matching this selector is used as the player.
    /// Otherwise the first entity categorized as `Player` is used.
    pub player: Option<EntitySelector>,
    /// Transform stage-space snapshot positions into the coordinate space used
    /// for distances, radii, and `MoveTo` intents.
    pub coordinates: CoordinateTransform,
    pub collector: CollectorConfig,
    pub npc_killer: NpcKillerConfig,
    pub kill_and_collect: KillAndCollectConfig,
    /// Temporary map travel suspends this mode and resumes it on arrival.
    pub working_map: Option<i32>,
    pub pet_enabled: bool,
}

impl Default for BehaviorConfig {
    fn default() -> Self {
        Self {
            mode: BehaviorMode::Collector,
            player: None,
            coordinates: CoordinateTransform::default(),
            collector: CollectorConfig::default(),
            npc_killer: NpcKillerConfig::default(),
            kill_and_collect: KillAndCollectConfig::default(),
            working_map: None,
            pet_enabled: true,
        }
    }
}

impl BehaviorConfig {
    /// Parse configuration from JSON and validate it before use.
    pub fn from_json(json: &str) -> Result<Self, StepError> {
        let config: Self = serde_json::from_str(json)
            .map_err(|error| StepError::InvalidConfig(error.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    /// Validate selector constraints and numeric ranges.
    pub fn validate(&self) -> Result<(), StepError> {
        if let Some(player) = &self.player {
            validate_selector(player, "player")?;
        }

        for (index, rule) in self.collector.boxes.iter().enumerate() {
            if rule.collect {
                validate_selector(&rule.selector, &format!("collector.boxes[{index}]"))?;
            }
        }
        for (index, rule) in self.npc_killer.npcs.iter().enumerate() {
            validate_non_negative(rule.combat_radius, "NPC combat_radius")?;
            if rule.kill {
                validate_selector(&rule.selector, &format!("npc_killer.npcs[{index}]"))?;
            }
        }

        validate_non_negative(self.collector.max_distance, "collector.max_distance")?;
        validate_non_negative(self.collector.pickup_radius, "collector.pickup_radius")?;
        validate_non_negative(self.collector.danger_distance, "collector.danger_distance")?;
        validate_non_negative(
            self.kill_and_collect.collect_radius,
            "kill_and_collect.collect_radius",
        )?;
        if self.npc_killer.max_circle_iterations == 0
            || self.npc_killer.max_circle_iterations > 1000
        {
            return Err(StepError::InvalidConfig(
                "npc_killer.max_circle_iterations must be between 1 and 1000".into(),
            ));
        }
        validate_non_negative(
            self.npc_killer.max_target_distance,
            "npc_killer.max_target_distance",
        )?;
        validate_non_negative(
            self.npc_killer.position_tolerance,
            "npc_killer.position_tolerance",
        )?;
        validate_non_negative(
            self.kill_and_collect.max_box_distance_from_npc,
            "kill_and_collect.max_box_distance_from_npc",
        )?;

        let health_threshold = self.kill_and_collect.collect_above_target_health;
        if !health_threshold.is_finite() || !(0.0..=1.0).contains(&health_threshold) {
            return Err(StepError::InvalidConfig(
                "kill_and_collect.collect_above_target_health must be between 0 and 1".into(),
            ));
        }

        self.coordinates.validate()?;
        Ok(())
    }
}

/// A conjunctive entity selector. Every populated property must match.
/// String comparisons are case-insensitive; `*_contains` properties allow
/// partial matches. At least one property must be set for an enabled rule.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct EntitySelector {
    pub category: Option<EntityCategory>,
    pub name: Option<String>,
    pub name_contains: Option<String>,
    pub class_name: Option<String>,
    pub class_name_contains: Option<String>,
    /// Exact display-list path match. Paths may change between snapshots.
    pub path: Option<String>,
    pub id: Option<Value>,
    /// Exact equality against raw snapshot fields, such as `type` or `kind`.
    pub fields: BTreeMap<String, Value>,
}

impl EntitySelector {
    pub fn name(name: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            ..Self::default()
        }
    }

    pub(crate) fn is_constrained(&self) -> bool {
        self.category.is_some()
            || self.name.as_deref().is_some_and(|value| !value.is_empty())
            || self
                .name_contains
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .class_name
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self
                .class_name_contains
                .as_deref()
                .is_some_and(|value| !value.is_empty())
            || self.path.as_deref().is_some_and(|value| !value.is_empty())
            || self.id.is_some()
            || !self.fields.is_empty()
    }
}

/// Per-box allowlist entry. Lower priority numbers are selected first.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BoxRule {
    pub selector: EntitySelector,
    pub collect: bool,
    pub priority: i32,
    /// Base pickup wait; retry, travel time and 30ms padding are added.
    pub wait_ms: u64,
    /// Cargo resources cannot be collected with a full hold.
    pub resource: bool,
}

impl Default for BoxRule {
    fn default() -> Self {
        Self {
            selector: EntitySelector::default(),
            collect: true,
            priority: 100,
            wait_ms: 500,
            resource: false,
        }
    }
}

/// Per-NPC allowlist entry. `combat_radius` is the desired player-to-NPC
/// distance while attacking, in transformed snapshot coordinate units.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcRule {
    pub selector: EntitySelector,
    pub kill: bool,
    pub priority: i32,
    pub combat_radius: f64,
    /// Restrict this rule to these map IDs. An empty list applies everywhere.
    pub map_ids: Vec<i32>,
    pub flags: Vec<NpcFlag>,
}

impl Default for NpcRule {
    fn default() -> Self {
        Self {
            selector: EntitySelector::default(),
            kill: true,
            priority: 100,
            combat_radius: 560.0,
            map_ids: Vec::new(),
            flags: Vec::new(),
        }
    }
}

/// Flags consulted by Darkbot's LootModule and LootCollectorModule.
/// Attack/item flags are forwarded to built-in manager policy and adapters.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NpcFlag {
    NoCircle,
    IgnoreOwnership,
    IgnoreAttacked,
    IgnoreBoxes,
    Passive,
    AttackSecond,
    LeechOnly,
    AggressiveFollow,
    NoSab,
    UseRsb,
    PetLocator,
}

/// Collection range and target ranking settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CollectorConfig {
    pub boxes: Vec<BoxRule>,
    pub max_distance: f64,
    pub pickup_radius: f64,
    /// Reduces the effective priority of the previously selected box, making
    /// target changes less frequent. Lower priority numbers are better.
    pub current_target_priority_bonus: i32,
    pub ignore_contested_boxes: bool,
    pub stay_away_from_enemies: bool,
    pub danger_distance: f64,
    pub auto_cloak: bool,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            boxes: Vec::new(),
            max_distance: 5_000.0,
            pickup_radius: 250.0,
            current_target_priority_bonus: 0,
            ignore_contested_boxes: false,
            stay_away_from_enemies: false,
            danger_distance: 1500.0,
            auto_cloak: false,
        }
    }
}

/// NPC target ranking and orbit distance settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NpcKillerConfig {
    pub npcs: Vec<NpcRule>,
    pub max_target_distance: f64,
    pub position_tolerance: f64,
    /// Reduces the effective priority of the previously selected NPC.
    pub current_target_priority_bonus: i32,
    pub only_kill_preferred: bool,
    pub max_circle_iterations: usize,
    pub run_config_in_circle: bool,
}

impl Default for NpcKillerConfig {
    fn default() -> Self {
        Self {
            npcs: Vec::new(),
            max_target_distance: 5_000.0,
            position_tolerance: 50.0,
            current_target_priority_bonus: 0,
            only_kill_preferred: false,
            max_circle_iterations: 5,
            run_config_in_circle: false,
        }
    }
}

/// Settings that govern collection while an NPC is being attacked.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KillAndCollectConfig {
    pub collect_during_combat: bool,
    /// Avoid detouring to boxes when the active target is below this health
    /// fraction. Unknown health is treated as full health.
    pub collect_above_target_health: f64,
    /// Distance cap for NPCs with IGNORE_BOXES, also capped at 2 * NPC radius.
    pub max_box_distance_from_npc: f64,
    /// Hero-to-box limit while fighting. NPC-to-box limit applies only to
    /// targets with IGNORE_BOXES, as in LootCollectorModule.
    pub collect_radius: f64,
}

impl Default for KillAndCollectConfig {
    fn default() -> Self {
        Self {
            collect_during_combat: true,
            collect_above_target_health: 0.25,
            max_box_distance_from_npc: 800.0,
            collect_radius: 500.0,
        }
    }
}

/// Affine conversion from the API's stage-space positions into the coordinate
/// system expected by behavior distances and movement intents.
///
/// The default is identity. Translation affects returned movement points but
/// not distances; the matrix can also scale or rotate the coordinate system.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CoordinateTransform {
    pub xx: f64,
    pub xy: f64,
    pub yx: f64,
    pub yy: f64,
    pub translate_x: f64,
    pub translate_y: f64,
}

impl Default for CoordinateTransform {
    fn default() -> Self {
        Self {
            xx: 1.0,
            xy: 0.0,
            yx: 0.0,
            yy: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
        }
    }
}

impl CoordinateTransform {
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.xx * x + self.xy * y + self.translate_x,
            self.yx * x + self.yy * y + self.translate_y,
        )
    }

    /// Convert a behavior point back to snapshot/world coordinates. Engine
    /// construction validates that this matrix is finite and invertible.
    pub fn inverse_apply(&self, x: f64, y: f64) -> (f64, f64) {
        let determinant = self.xx * self.yy - self.xy * self.yx;
        let x = x - self.translate_x;
        let y = y - self.translate_y;
        (
            (self.yy * x - self.xy * y) / determinant,
            (-self.yx * x + self.xx * y) / determinant,
        )
    }

    fn validate(&self) -> Result<(), StepError> {
        let values = [
            self.xx,
            self.xy,
            self.yx,
            self.yy,
            self.translate_x,
            self.translate_y,
        ];
        if values.iter().any(|value| !value.is_finite()) {
            return Err(StepError::InvalidConfig(
                "coordinate transform values must be finite".into(),
            ));
        }
        let determinant = self.xx * self.yy - self.xy * self.yx;
        if !determinant.is_finite() || determinant.abs() < f64::EPSILON {
            return Err(StepError::InvalidConfig(
                "coordinate transform matrix must be invertible".into(),
            ));
        }
        Ok(())
    }
}

fn validate_selector(selector: &EntitySelector, label: &str) -> Result<(), StepError> {
    if selector.is_constrained() {
        Ok(())
    } else {
        Err(StepError::InvalidConfig(format!(
            "{label} needs at least one selector property"
        )))
    }
}

fn validate_non_negative(value: f64, label: &str) -> Result<(), StepError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(StepError::InvalidConfig(format!(
            "{label} must be a finite, non-negative number"
        )))
    }
}
