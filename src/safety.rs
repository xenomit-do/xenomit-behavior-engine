//! Cooperative repair/escape/revive policy using observed native manager state.
use crate::{CoordinateTransform, ModuleContext, ObstacleNavigation, Point};
use serde::{Deserialize, Serialize};
use xenomit_game_api::{EntityCategory, GameCommand, GameSnapshot};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    pub enabled: bool,
    pub min_health_percent: f64,
    pub max_health_percent: f64,
    pub roaming_repair_health_percent: f64,
    pub repair_shield_percent: f64,
    pub run_from_enemy: bool,
    pub run_in_sight: bool,
    pub sight_distance: f64,
    pub max_deaths: u32,
    pub revive_wait_seconds: u64,
    pub auto_revive: bool,
}
impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_health_percent: 40.0,
            max_health_percent: 95.0,
            roaming_repair_health_percent: 50.0,
            repair_shield_percent: 100.0,
            run_from_enemy: true,
            run_in_sight: false,
            sight_distance: 2500.0,
            max_deaths: 10,
            revive_wait_seconds: 5,
            auto_revive: true,
        }
    }
}
impl SafetyConfig {
    pub fn validate(&self) -> Result<(), String> {
        for value in [
            self.min_health_percent,
            self.max_health_percent,
            self.roaming_repair_health_percent,
            self.repair_shield_percent,
        ] {
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err("Safety percentages must be between 0 and 100".into());
            }
        }
        if self.min_health_percent > self.max_health_percent {
            return Err("Repair resume health must be at least the retreat health".into());
        }
        if !self.sight_distance.is_finite()
            || self.sight_distance < 0.0
            || self.revive_wait_seconds > 3600
        {
            return Err("Invalid safety distance or revive delay".into());
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct SafetyManager {
    repairing_since: Option<u64>,
    enemy_until: u64,
    dead_since: Option<u64>,
    deaths: u32,
    last_revive: Option<u64>,
    revive_attempts: u32,
}
pub struct SafetyOutcome {
    pub context: ModuleContext,
    pub commands: Vec<GameCommand>,
    pub repairing: bool,
    pub failure: Option<String>,
}
impl SafetyManager {
    pub fn deaths(&self) -> u32 {
        self.deaths
    }
    pub fn tick(
        &mut self,
        snapshot: &GameSnapshot,
        now_ms: u64,
        config: &SafetyConfig,
        transform: CoordinateTransform,
    ) -> SafetyOutcome {
        let hero = snapshot
            .entities
            .iter()
            .find(|e| e.category == EntityCategory::Player);
        let mut outcome = SafetyOutcome {
            context: hero
                .map(|hero| ModuleContext::from_snapshot(snapshot, hero, transform))
                .unwrap_or_default(),
            commands: vec![],
            repairing: false,
            failure: None,
        };
        if snapshot.navigation.hero.killed {
            if self.dead_since.is_none() {
                self.dead_since = Some(now_ms);
                self.deaths += 1;
                self.last_revive = None;
                self.revive_attempts = 0;
            }
            outcome.context.safe_to_work = false;
            outcome.context.safety_status = Some(format!(
                "Destroyed ({}/{} deaths); waiting for free base repair",
                self.deaths, config.max_deaths
            ));
            if config.max_deaths > 0 && self.deaths >= config.max_deaths {
                outcome.failure = Some("Maximum deaths reached".into());
                return outcome;
            }
            if !config.enabled || !config.auto_revive {
                return outcome;
            }
            if self.revive_attempts >= 12 {
                outcome.failure =
                    Some("Free base revive did not complete after 12 attempts".into());
                return outcome;
            }
            let elapsed = now_ms.saturating_sub(self.dead_since.unwrap_or(now_ms));
            if elapsed >= config.revive_wait_seconds.saturating_mul(1000)
                && self
                    .last_revive
                    .is_none_or(|last| now_ms.saturating_sub(last) >= 10_000)
                && let Some(repair_type) = snapshot.navigation.hero.free_repair_type
            {
                outcome.commands.push(GameCommand::Revive { repair_type });
                self.last_revive = Some(now_ms);
                self.revive_attempts += 1;
            }
            return outcome;
        }
        self.dead_since = None;
        if !config.enabled {
            self.repairing_since = None;
            self.enemy_until = 0;
            return outcome;
        }
        let Some(hero) = hero else {
            outcome.context.safe_to_work = false;
            return outcome;
        };
        let hp = percentage(hero.health, hero.max_health);
        let shield = percentage(hero.shield, hero.max_shield);
        let enemy = snapshot.entities.iter().any(|e| {
            let observation = outcome.context.observation(e);
            observation.enemy
                && ((config.run_from_enemy && observation.attacking_hero)
                    || (config.run_in_sight
                        && (e.position.x - hero.position.x).hypot(e.position.y - hero.position.y)
                            < config.sight_distance))
        });
        if enemy {
            self.enemy_until = now_ms.saturating_add(15_000);
        }
        let escaping = now_ms < self.enemy_until;
        let roaming = outcome.context.hero_attacking.is_none();
        if hp.is_some_and(|hp| {
            hp < config.min_health_percent || (roaming && hp < config.roaming_repair_health_percent)
        }) {
            self.repairing_since.get_or_insert(now_ms);
        }
        if hp.is_some_and(|hp| hp >= config.max_health_percent)
            && shield.is_none_or(|shield| shield >= config.repair_shield_percent)
        {
            self.repairing_since = None;
        }
        if self
            .repairing_since
            .is_some_and(|at| now_ms.saturating_sub(at) > 300_000)
        {
            outcome.failure = Some("Repair did not recover health within five minutes".into());
            return outcome;
        }
        outcome.repairing = self.repairing_since.is_some();
        if !escaping && !outcome.repairing {
            return outcome;
        }
        outcome.context.safe_to_work = false;
        let navigation =
            ObstacleNavigation::from_snapshot(snapshot, CoordinateTransform::default());
        let from = Point {
            x: hero.position.x,
            y: hero.position.y,
        };
        let hero_faction = hero
            .fields
            .get("faction")
            .and_then(serde_json::Value::as_i64)
            .filter(|faction| *faction > 0);
        // Portals and friendly bases only; battle stations are not assumed safe.
        let destination = snapshot
            .map
            .as_ref()
            .into_iter()
            .flat_map(|m| &m.safe_places)
            .filter(|place| {
                place.kind == "portal"
                    || (place.kind == "base"
                        && hero_faction.is_some()
                        && snapshot.entities.iter().any(|entity| {
                            (entity.position.x - place.x).hypot(entity.position.y - place.y) < 1.0
                                && entity
                                    .fields
                                    .get("faction")
                                    .and_then(serde_json::Value::as_i64)
                                    == hero_faction
                        }))
            })
            .filter_map(|place| {
                navigation.nearest_point(Point {
                    x: place.x,
                    y: place.y,
                })
            })
            .filter_map(|goal| {
                navigation.find_path(from, goal).ok().map(|path| {
                    let mut previous = from;
                    let mut cost = 0.0;
                    for point in path {
                        cost += (point.x - previous.x).hypot(point.y - previous.y);
                        previous = point;
                    }
                    (goal, cost)
                })
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(goal, _)| goal);
        if let Some(destination) = destination {
            let near = (destination.x - from.x).hypot(destination.y - from.y) <= 150.0;
            let (x, y) = transform.apply(destination.x, destination.y);
            outcome.context.safety_destination = (!near).then_some(Point { x, y });
            outcome.context.safety_waiting = near;
        } else {
            outcome.context.safety_waiting = true;
        }
        outcome.context.safety_status = Some(
            if escaping {
                "Retreating from enemies"
            } else if outcome.context.safety_waiting {
                "Waiting for health/shield repair"
            } else {
                "Moving to safety for repair"
            }
            .into(),
        );
        if outcome.repairing
            && outcome.context.safety_waiting
            && !escaping
            && let Some((id, _)) = snapshot.navigation.hero.items.iter().find(|(id, item)| {
                id.starts_with("equipment_extra_repbot_")
                    && item.available
                    && item.activatable
                    && !item.blocked
                    && !item.selected
            })
        {
            outcome.commands.push(GameCommand::UseItem {
                item_id: id.clone(),
                activate: true,
            });
        }
        outcome
    }
}
fn percentage(value: Option<f64>, max: Option<f64>) -> Option<f64> {
    let max = max.filter(|max| max.is_finite() && *max > 0.0)?;
    value
        .filter(|value| value.is_finite())
        .map(|value| (value / max * 100.0).clamp(0.0, 100.0))
}

/// Retained safety-policy state for checkpoint export. Deadlines are stored
/// as remaining durations and stamps as ages, relative to `now_ms`.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct SafetyCheckpoint {
    /// How long the hero has been repairing, if a repair is in progress.
    pub repairing_age_ms: Option<u64>,
    /// Remaining time the remembered threat stays relevant.
    pub enemy_remaining_ms: Option<u64>,
    /// How long ago the hero was observed dead.
    pub dead_age_ms: Option<u64>,
    /// Deaths observed during this run.
    pub deaths: u32,
    /// How long ago the last revive was requested.
    pub last_revive_age_ms: Option<u64>,
    /// Revive attempts for the current death.
    pub revive_attempts: u32,
}

impl SafetyManager {
    /// Export safety state relative to `now_ms`.
    pub fn export_checkpoint(&self, now_ms: u64) -> SafetyCheckpoint {
        SafetyCheckpoint {
            repairing_age_ms: crate::checkpoint::age_ms(self.repairing_since, now_ms),
            enemy_remaining_ms: crate::checkpoint::remaining_ms(Some(self.enemy_until), now_ms),
            dead_age_ms: crate::checkpoint::age_ms(self.dead_since, now_ms),
            deaths: self.deaths,
            last_revive_age_ms: crate::checkpoint::age_ms(self.last_revive, now_ms),
            revive_attempts: self.revive_attempts,
        }
    }

    /// Restore safety state exported by [`SafetyManager::export_checkpoint`].
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: &SafetyCheckpoint,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        self.repairing_since = crate::checkpoint::rebase_age(now_ms, checkpoint.repairing_age_ms);
        self.enemy_until = crate::checkpoint::rebase_deadline(now_ms, checkpoint.enemy_remaining_ms)
            .unwrap_or(0);
        self.dead_since = crate::checkpoint::rebase_age(now_ms, checkpoint.dead_age_ms);
        self.deaths = checkpoint.deaths;
        self.last_revive = crate::checkpoint::rebase_age(now_ms, checkpoint.last_revive_age_ms);
        self.revive_attempts = checkpoint.revive_attempts;
        Ok(())
    }
}
