//! Behavior ports from the upstream SharedPlugin that fit the engine's
//! snapshot and typed-command contract.
use crate::{
    ActionIntent, BehaviorDecision, BehaviorMode, BehaviorRegistry, ControlRequests, EntityRef,
    GeneralBehavior, ModuleRegistryBuilder, Point, RuleContext, RuleOutput, ShipMode,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xenomit_game_api::{
    EntityCategory, GameCommand, GameEntity, GameSnapshot, ModuleDescriptor, SettingDescriptor,
    SettingKind,
};

/// Register SharedPlugin behaviors supported by normalized snapshots and typed
/// manager commands. Call before constructing `BehaviorRuntime`.
pub fn register_shared_behaviors(registry: &mut BehaviorRegistry) -> Result<(), String> {
    registry.register(
        30,
        behavior_descriptor(
            "shared_simple_healing",
            "Shared: Simple healing",
            json!({
                "hp_enabled": true, "hp_threshold": 0.5,
                "shield_enabled": false, "shield_threshold": 0.5,
                "pod_enabled": false, "pod_threshold": 0.5
            }),
        ),
        |settings| {
            validate_percent_options(
                settings,
                &["hp_threshold", "shield_threshold", "pod_threshold"],
            )?;
            Ok(Box::new(SimpleHealing {
                settings: settings.clone(),
                last_attempt: BTreeMap::new(),
            }))
        },
    )?;
    registry.register(
        31,
        behavior_descriptor(
            "shared_crowd_avoidance",
            "Shared: Crowd avoidance",
            json!({
                "count": 4, "radius": 1200.0, "npcs": false, "enemies": true,
                "allies": false, "run_mode": true, "preferred_zone_only": false
            }),
        ),
        |settings| {
            validate_range(settings, "count", 1.0, 100.0)?;
            validate_range(settings, "radius", 1.0, 20_000.0)?;
            Ok(Box::new(CrowdAvoidance {
                settings: settings.clone(),
            }))
        },
    )?;
    registry.register(
        32,
        behavior_descriptor(
            "shared_solaris_inc",
            "Shared: Solaris ability",
            json!({
                "min_npcs": 3, "max_distance": 1000.0, "extra_flag_only": false,
                "use_pet_kamikaze": false, "stick_min_hp": 0.0
            }),
        ),
        |settings| {
            validate_range(settings, "min_npcs", 1.0, 100.0)?;
            validate_range(settings, "max_distance", 1.0, 20_000.0)?;
            validate_range(settings, "stick_min_hp", 0.0, 1.0)?;
            Ok(Box::new(SolarisAbility {
                settings: settings.clone(),
                last_use: None,
            }))
        },
    )?;
    registry.register(
        33,
        behavior_descriptor(
            "shared_repair_pet",
            "Shared: Repair PET",
            json!({"hp_threshold": 0.8}),
        ),
        |settings| {
            validate_range(settings, "hp_threshold", 0.0, 1.0)?;
            Ok(Box::new(RepairPet {
                settings: settings.clone(),
                repairing: false,
                last_attempt: None,
            }))
        },
    )?;
    registry.register(
        34,
        behavior_descriptor(
            "shared_fix_pet_stuck",
            "Shared: Fix PET stuck",
            json!({"stuck_seconds": 60}),
        ),
        |settings| {
            validate_range(settings, "stuck_seconds", 1.0, 3_600.0)?;
            Ok(Box::new(FixPetStuck {
                settings: settings.clone(),
                inactive_since: None,
                last_refresh: None,
                pending_refresh: false,
            }))
        },
    )?;
    Ok(())
}

/// Register supported SharedPlugin objectives before attaching a runtime.
pub fn register_shared_modules(registry: &mut ModuleRegistryBuilder) -> Result<(), String> {
    crate::shared_spaceball::register(registry)
}

fn behavior_descriptor(id: &'static str, name: &'static str, options: Value) -> ModuleDescriptor {
    ModuleDescriptor::new(id, name)
        .description("Ported SharedPlugin behavior; settings are global behavior settings.")
        .setting(SettingDescriptor::new(
            "enabled",
            "Enabled",
            SettingKind::Boolean,
            json!(false),
        ))
        .setting(SettingDescriptor::new(
            "tick_stopped",
            "Run while objective is paused",
            SettingKind::Boolean,
            json!(false),
        ))
        .setting(SettingDescriptor::new(
            "options",
            "Options",
            SettingKind::Json,
            options,
        ))
}

struct SimpleHealing {
    settings: Value,
    last_attempt: BTreeMap<String, u64>,
}
impl GeneralBehavior for SimpleHealing {
    fn tick(&mut self, ctx: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !active(&self.settings, ctx) {
            return out;
        }
        let Some(s) = ctx.snapshot else { return out };
        let Some(hero) = hero(s) else { return out };
        let opts = &self.settings["options"];
        let ship = field_text(hero, "shipType")
            .unwrap_or(&hero.class_name)
            .to_ascii_lowercase();
        let hp = percent(hero.health, hero.max_health).or_else(|| field_number(hero, "hpPercent"));
        let shield =
            percent(hero.shield, hero.max_shield).or_else(|| field_number(hero, "shieldPercent"));
        let abilities: &[&str] = if ship.contains("aegis") || ship.contains("hammerclaw") {
            &[
                "ability_aegis_hp-repair",
                "ability_solace",
                "ability_solace-plus_nano-cluster-repairer-plus",
            ]
        } else if ship.contains("orcus") {
            &["ability_orcus_assimilate"]
        } else if ship.contains("solace-plus") {
            &[
                "ability_solace-plus_nano-cluster-repairer-plus",
                "ability_solace",
            ]
        } else if ship.contains("solace") {
            &["ability_solace"]
        } else {
            &[]
        };
        if opts["hp_enabled"].as_bool().unwrap_or(true)
            && hp.is_some_and(|v| v <= opts["hp_threshold"].as_f64().unwrap_or(0.5))
        {
            if let Some(id) = abilities
                .iter()
                .find(|id| ready(s, id) && self.due(id, ctx.now_ms))
            {
                self.last_attempt.insert((*id).into(), ctx.now_ms);
                out.commands.push(GameCommand::UseItem {
                    item_id: (*id).into(),
                    activate: true,
                });
            }
        }
        if opts["shield_enabled"].as_bool().unwrap_or(false)
            && shield.is_some_and(|v| v <= opts["shield_threshold"].as_f64().unwrap_or(0.5))
            && ready(s, "ability_aegis_shield-repair")
            && self.due("ability_aegis_shield-repair", ctx.now_ms)
        {
            self.last_attempt
                .insert("ability_aegis_shield-repair".into(), ctx.now_ms);
            out.commands.push(GameCommand::UseItem {
                item_id: "ability_aegis_shield-repair".into(),
                activate: true,
            });
        }
        if opts["pod_enabled"].as_bool().unwrap_or(false)
            && hp.is_some_and(|v| v <= opts["pod_threshold"].as_f64().unwrap_or(0.5))
            && ready(s, "ability_aegis_repair-pod")
            && self.due("ability_aegis_repair-pod", ctx.now_ms)
        {
            self.last_attempt
                .insert("ability_aegis_repair-pod".into(), ctx.now_ms);
            out.commands.push(GameCommand::UseItem {
                item_id: "ability_aegis_repair-pod".into(),
                activate: true,
            });
        }
        out
    }
    fn update(&mut self, settings: &Value) -> Result<(), String> {
        validate_percent_options(
            settings,
            &["hp_threshold", "shield_threshold", "pod_threshold"],
        )?;
        self.settings = settings.clone();
        Ok(())
    }
    fn validate_settings(&self, settings: &Value) -> Result<(), String> {
        validate_percent_options(
            settings,
            &["hp_threshold", "shield_threshold", "pod_threshold"],
        )
    }
}
impl SimpleHealing {
    fn due(&self, id: &str, now: u64) -> bool {
        self.last_attempt
            .get(id)
            .is_none_or(|last| now.saturating_sub(*last) >= 250)
    }
}

struct CrowdAvoidance {
    settings: Value,
}
impl GeneralBehavior for CrowdAvoidance {
    fn tick(&mut self, ctx: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !active(&self.settings, ctx) {
            return out;
        }
        let Some(s) = ctx.snapshot else { return out };
        let Some(h) = hero(s) else { return out };
        let o = &self.settings["options"];
        if !o["npcs"].as_bool().unwrap_or(false)
            && !o["enemies"].as_bool().unwrap_or(true)
            && !o["allies"].as_bool().unwrap_or(false)
        {
            return out;
        }
        if o["preferred_zone_only"].as_bool().unwrap_or(false)
            && h.fields.get("inPreferredZone").and_then(Value::as_bool) != Some(true)
        {
            return out;
        }
        // Collection and map travel must retain control of movement.
        if truth(h, "isCollecting") || truth(h, "isBootyCollecting") || truth(h, "isTraveling") {
            return out;
        }
        let radius = o["radius"].as_f64().unwrap_or(1200.0);
        let candidates: Vec<_> = s
            .entities
            .iter()
            .filter(|e| {
                let d = dist(h.position, e.position);
                if d > radius {
                    return false;
                }
                match e.category {
                    EntityCategory::Npc => o["npcs"].as_bool().unwrap_or(false),
                    EntityCategory::Ship | EntityCategory::Player => {
                        if truth(e, "isPet") || same_group(s, e) {
                            return false;
                        }
                        let enemy = truth(e, "isEnemy") || truth(e, "isBlacklisted");
                        if enemy {
                            o["enemies"].as_bool().unwrap_or(true)
                        } else {
                            o["allies"].as_bool().unwrap_or(false) && !same_clan(h, e)
                        }
                    }
                    _ => false,
                }
            })
            .collect();
        if candidates.len() < o["count"].as_u64().unwrap_or(4) as usize || near_safe_place(s, h) {
            return out;
        }
        let closest = candidates
            .into_iter()
            .min_by(|a, b| dist(h.position, a.position).total_cmp(&dist(h.position, b.position)))
            .unwrap();
        let dx = h.position.x - closest.position.x;
        let dy = h.position.y - closest.position.y;
        let length = dx.hypot(dy);
        let (ux, uy) = if length > 0.01 {
            (dx / length, dy / length)
        } else {
            (1.0, 0.0)
        };
        let mut plan = wait_plan(s, "Shared crowd avoidance");
        plan.controls.ship_mode = o["run_mode"]
            .as_bool()
            .unwrap_or(true)
            .then_some(ShipMode::Run);
        plan.actions.push(ActionIntent::MoveTo {
            destination: Point {
                x: closest.position.x + ux * (radius + 1500.0),
                y: closest.position.y + uy * (radius + 1500.0),
            },
        });
        out.takeover = Some(plan);
        out
    }
    fn update(&mut self, settings: &Value) -> Result<(), String> {
        validate_range(settings, "count", 1.0, 100.0)?;
        validate_range(settings, "radius", 1.0, 20_000.0)?;
        self.settings = settings.clone();
        Ok(())
    }
    fn validate_settings(&self, settings: &Value) -> Result<(), String> {
        validate_range(settings, "count", 1.0, 100.0)?;
        validate_range(settings, "radius", 1.0, 20_000.0)
    }
}

struct SolarisAbility {
    settings: Value,
    last_use: Option<u64>,
}
impl GeneralBehavior for SolarisAbility {
    fn tick(&mut self, ctx: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !active(&self.settings, ctx) {
            return out;
        }
        let Some(s) = ctx.snapshot else { return out };
        let Some(h) = hero(s) else { return out };
        let ship = field_text(h, "shipType")
            .unwrap_or(&h.class_name)
            .to_ascii_lowercase();
        let (ability, active_ms) = if ship.contains("solaris-plus") {
            ("ability_solaris-plus_incinerate-plus", 10_000)
        } else if ship.contains("solaris") {
            ("ability_solaris_inc", 10_000)
        } else if ship.contains("paladin") {
            ("ability_paladin_ripper", 3_000)
        } else {
            return out;
        };
        let o = &self.settings["options"];
        let max_dist = o["max_distance"].as_f64().unwrap_or(1000.0);
        let count = s
            .entities
            .iter()
            .filter(|e| {
                e.category == EntityCategory::Npc
                    && dist(h.position, e.position) <= max_dist
                    && (!o["extra_flag_only"].as_bool().unwrap_or(false)
                        || truth(e, "hasSolarisAbility"))
            })
            .count();
        if count == 0 {
            return out;
        }
        let cooldown = 30_000u64;
        if count >= o["min_npcs"].as_u64().unwrap_or(3) as usize
            && self
                .last_use
                .is_none_or(|t| ctx.now_ms.saturating_sub(t) >= cooldown)
            && ready(s, ability)
        {
            self.last_use = Some(ctx.now_ms);
            out.commands.push(GameCommand::UseItem {
                item_id: ability.into(),
                activate: true,
            });
            if o["use_pet_kamikaze"].as_bool().unwrap_or(false)
                && s.navigation.hero.pet_active
                && s.navigation.hero.pet_gears.contains(&10)
            {
                out.commands.push(GameCommand::SelectPetGear {
                    gear_type: 10,
                    target: 0,
                });
            }
        }
        let hp = percent(h.health, h.max_health).or_else(|| field_number(h, "hpPercent"));
        if self
            .last_use
            .is_some_and(|t| ctx.now_ms.saturating_sub(t) <= active_ms)
            && hp.is_some_and(|v| v >= o["stick_min_hp"].as_f64().unwrap_or(0.0))
        {
            if let Some(target) = s
                .entities
                .iter()
                .filter(|e| {
                    e.category == EntityCategory::Npc && dist(h.position, e.position) <= max_dist
                })
                .min_by(|a, b| {
                    dist(h.position, a.position).total_cmp(&dist(h.position, b.position))
                })
            {
                let mut plan = wait_plan(s, "Shared Solaris ability window");
                plan.actions.push(ActionIntent::MoveTo {
                    destination: Point {
                        x: target.position.x,
                        y: target.position.y,
                    },
                });
                out.takeover = Some(plan);
            }
        }
        out
    }
    fn update(&mut self, v: &Value) -> Result<(), String> {
        validate_range(v, "min_npcs", 1.0, 100.0)?;
        validate_range(v, "max_distance", 1.0, 20_000.0)?;
        validate_range(v, "stick_min_hp", 0.0, 1.0)?;
        self.settings = v.clone();
        Ok(())
    }
    fn validate_settings(&self, v: &Value) -> Result<(), String> {
        validate_range(v, "min_npcs", 1.0, 100.0)?;
        validate_range(v, "max_distance", 1.0, 20_000.0)?;
        validate_range(v, "stick_min_hp", 0.0, 1.0)
    }
}

struct RepairPet {
    settings: Value,
    repairing: bool,
    last_attempt: Option<u64>,
}
impl GeneralBehavior for RepairPet {
    fn tick(&mut self, ctx: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !active(&self.settings, ctx) {
            return out;
        }
        let Some(s) = ctx.snapshot else { return out };
        let Some(h) = hero(s) else { return out };
        if !s.navigation.hero.owns_pet || !s.navigation.hero.pet_active {
            self.repairing = false;
            return out;
        }
        let Some(pet) = s.entities.iter().find(|e| {
            let class_pet = truth(e, "isPet") || e.class_name.eq_ignore_ascii_case("pet");
            let owned = truth(e, "isOwnPet")
                || owner_id(e).is_some_and(|id| h.id.as_ref().and_then(Value::as_i64) == Some(id));
            class_pet && owned
        }) else {
            return out;
        };
        let Some(hp) = percent(pet.health, pet.max_health) else {
            return out;
        };
        let threshold = self.settings["options"]["hp_threshold"]
            .as_f64()
            .unwrap_or(0.8)
            .clamp(0.05, 0.95);
        if hp < threshold {
            self.repairing = true;
        }
        if self.repairing
            && hp < 0.99
            && !under_attack(s, h)
            && !hero_in_combat(h)
            && s.navigation.hero.pet_gears.contains(&9)
            && s.navigation.hero.pet_gear != Some(9)
            && self
                .last_attempt
                .is_none_or(|t| ctx.now_ms.saturating_sub(t) >= 5_000)
        {
            out.commands.push(GameCommand::SelectPetGear {
                gear_type: 9,
                target: 0,
            });
            self.last_attempt = Some(ctx.now_ms);
        }
        if hp >= 0.99 {
            self.repairing = false;
        }
        out
    }
    fn update(&mut self, v: &Value) -> Result<(), String> {
        validate_range(v, "hp_threshold", 0.0, 1.0)?;
        self.settings = v.clone();
        Ok(())
    }
    fn validate_settings(&self, v: &Value) -> Result<(), String> {
        validate_range(v, "hp_threshold", 0.0, 1.0)
    }
}

struct FixPetStuck {
    settings: Value,
    inactive_since: Option<u64>,
    last_refresh: Option<u64>,
    pending_refresh: bool,
}
impl GeneralBehavior for FixPetStuck {
    fn tick(&mut self, ctx: &RuleContext<'_>) -> RuleOutput {
        let mut out = RuleOutput::default();
        if !active(&self.settings, ctx) {
            return out;
        }
        let Some(s) = ctx.snapshot else { return out };
        let Some(h) = hero(s) else { return out };
        if !s.navigation.hero.owns_pet || !pet_enabled(ctx, h) {
            self.inactive_since = None;
            self.pending_refresh = false;
            return out;
        }
        if s.navigation.hero.pet_active {
            self.inactive_since = None;
            self.pending_refresh = false;
            return out;
        }
        if self
            .last_refresh
            .is_some_and(|t| ctx.now_ms.saturating_sub(t) < 60_000)
        {
            return out;
        }
        if activity(h) {
            let since = *self.inactive_since.get_or_insert(ctx.now_ms);
            let duration = self.settings["options"]["stuck_seconds"]
                .as_u64()
                .unwrap_or(60)
                .saturating_mul(1000);
            if ctx.now_ms.saturating_sub(since) >= duration {
                self.pending_refresh = true;
            }
        } else if !self.pending_refresh {
            self.inactive_since = None;
        }
        if self.pending_refresh
            && !under_attack(s, h)
            && !hero_in_combat(h)
            && !(s.entities.iter().any(|e| e.category == EntityCategory::Npc)
                && (truth(h, "isGG")
                    || s.navigation.map_id.is_some_and(|id| {
                        s.navigation
                            .star_system
                            .map(id)
                            .is_some_and(|m| m.name.contains("GG") || m.name.contains("Eternal"))
                    })))
        {
            out.host_actions.push(crate::HostAction::Refresh);
            self.last_refresh = Some(ctx.now_ms);
            self.inactive_since = None;
            self.pending_refresh = false;
        }
        out
    }
    fn update(&mut self, v: &Value) -> Result<(), String> {
        validate_range(v, "stuck_seconds", 1.0, 3_600.0)?;
        self.settings = v.clone();
        Ok(())
    }
    fn validate_settings(&self, v: &Value) -> Result<(), String> {
        validate_range(v, "stuck_seconds", 1.0, 3_600.0)
    }
}

fn hero(s: &GameSnapshot) -> Option<&GameEntity> {
    s.entities
        .iter()
        .find(|e| e.category == EntityCategory::Player && truth(e, "isHero"))
        .or_else(|| {
            s.entities
                .iter()
                .find(|e| e.category == EntityCategory::Player)
        })
}
fn active(settings: &Value, ctx: &RuleContext<'_>) -> bool {
    settings["enabled"].as_bool().unwrap_or(false)
        && (!ctx.stopped || settings["tick_stopped"].as_bool().unwrap_or(false))
}
fn validate_range(settings: &Value, key: &str, min: f64, max: f64) -> Result<(), String> {
    let value = settings["options"][key]
        .as_f64()
        .ok_or_else(|| format!("options.{key} must be a number"))?;
    if !value.is_finite() || value < min || value > max {
        return Err(format!("options.{key} must be between {min} and {max}"));
    }
    Ok(())
}
fn validate_percent_options(settings: &Value, keys: &[&str]) -> Result<(), String> {
    for key in keys {
        validate_range(settings, key, 0.0, 1.0)?;
    }
    Ok(())
}
fn owner_id(e: &GameEntity) -> Option<i64> {
    e.fields
        .get("ownerId")
        .or_else(|| e.fields.get("ownerID"))
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
}
fn pet_enabled(ctx: &RuleContext<'_>, hero: &GameEntity) -> bool {
    ctx.module_settings
        .get("pet_enabled")
        .or_else(|| ctx.module_settings.get("pet_active"))
        .and_then(Value::as_bool)
        == Some(true)
        || ctx
            .module_settings
            .pointer("/features/pet_active")
            .and_then(Value::as_bool)
            == Some(true)
        || hero.fields.get("petEnabled").and_then(Value::as_bool) == Some(true)
}
fn has_target(e: &GameEntity, key: &str) -> bool {
    e.fields.get(key).is_some_and(|v| {
        v.as_i64().is_some_and(|id| id > 0) || v.as_str().is_some_and(|s| !s.is_empty() && s != "0")
    })
}
fn hero_in_combat(h: &GameEntity) -> bool {
    truth(h, "attacking")
        && (has_target(h, "attackingTargetId")
            || has_target(h, "localTargetId")
            || has_target(h, "lockedTargetId"))
}
fn truth(e: &GameEntity, key: &str) -> bool {
    e.fields.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn field_text<'a>(e: &'a GameEntity, key: &str) -> Option<&'a str> {
    e.fields.get(key).and_then(Value::as_str)
}
fn field_number(e: &GameEntity, key: &str) -> Option<f64> {
    e.fields
        .get(key)
        .and_then(Value::as_f64)
        .filter(|n| n.is_finite())
}
fn percent(value: Option<f64>, max: Option<f64>) -> Option<f64> {
    value.zip(max).filter(|(_, m)| *m > 0.0).map(|(v, m)| v / m)
}
fn ready(s: &GameSnapshot, id: &str) -> bool {
    s.navigation
        .hero
        .items
        .get(id)
        .is_some_and(|i| i.available && i.activatable && !i.blocked && !i.selected)
}
fn dist(a: xenomit_game_api::Point, b: xenomit_game_api::Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
fn same_group(s: &GameSnapshot, e: &GameEntity) -> bool {
    e.id.as_ref().and_then(Value::as_i64).is_some_and(|id| {
        s.navigation
            .group
            .members
            .iter()
            .any(|m| i64::from(m.id) == id)
    })
}
fn same_clan(a: &GameEntity, b: &GameEntity) -> bool {
    let clan = |e: &GameEntity| e.fields.get("clanId").and_then(Value::as_i64).unwrap_or(0);
    clan(a) > 0 && clan(a) == clan(b)
}
fn near_safe_place(s: &GameSnapshot, h: &GameEntity) -> bool {
    s.map.as_ref().is_some_and(|m| {
        m.safe_places.iter().any(|p| {
            dist(h.position, xenomit_game_api::Point { x: p.x, y: p.y }) <= p.radius.max(500.0)
        })
    }) || s
        .navigation
        .portals
        .iter()
        .any(|p| dist(h.position, p.position) <= 500.0)
}
fn activity(h: &GameEntity) -> bool {
    hero_in_combat(h) || truth(h, "isCollecting") || truth(h, "isBootyCollecting")
}
fn under_attack(s: &GameSnapshot, h: &GameEntity) -> bool {
    s.entities.iter().any(|e| {
        (e.category == EntityCategory::Npc
            || e.category == EntityCategory::Player
            || e.category == EntityCategory::Ship)
            && (truth(e, "attackingHero")
                || h.id.as_ref().is_some_and(|id| {
                    e.fields
                        .get("attackingTargetId")
                        .is_some_and(|target| target_matches(target, id))
                }))
            && dist(h.position, e.position) <= 1500.0
    })
}
fn target_matches(target: &Value, hero_id: &Value) -> bool {
    target == hero_id
        || target
            .as_i64()
            .zip(hero_id.as_str().and_then(|id| id.parse::<i64>().ok()))
            .is_some_and(|(a, b)| a == b)
        || target
            .as_str()
            .and_then(|id| id.parse::<i64>().ok())
            .zip(hero_id.as_i64())
            .is_some_and(|(a, b)| a == b)
}
fn wait_plan(s: &GameSnapshot, note: &str) -> BehaviorDecision {
    BehaviorDecision {
        mode: BehaviorMode::Collector,
        player: hero(s).map(as_ref),
        selected_npc: None,
        selected_box: None,
        actions: Vec::new(),
        note: Some(note.into()),
        controls: ControlRequests::default(),
        suspended: true,
        can_refresh: false,
    }
}
fn as_ref(e: &GameEntity) -> EntityRef {
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
