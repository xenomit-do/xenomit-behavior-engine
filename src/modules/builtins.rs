//! Built-in module registration and settings translation owned by the engine.
use crate::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xenomit_game_api::{EntityCategory, ModuleConfig, setting_value};
pub fn builtin_behavior_config(module: &ModuleConfig) -> Result<BehaviorConfig, String> {
    if let Some(value) = module
        .settings
        .get("behavior_config")
        .filter(|value| !value.is_null())
    {
        return serde_json::from_value(value.clone())
            .map_err(|error| format!("invalid behavior_config: {error}"));
    }

    let mut config = BehaviorConfig {
        player: Some(EntitySelector {
            category: Some(EntityCategory::Player),
            ..EntitySelector::default()
        }),
        ..BehaviorConfig::default()
    };
    let features = module.settings.get("features");
    let enabled = |name: &str, fallback: bool| {
        features
            .and_then(Value::as_object)
            .and_then(|values| values.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(fallback)
    };
    let box_rule = BoxRule {
        selector: EntitySelector {
            fields: BTreeMap::from([("entityKind".into(), json!("box"))]),
            ..EntitySelector::default()
        },
        wait_ms: 1_500,
        ..BoxRule::default()
    };
    let box_class_rule = BoxRule {
        selector: EntitySelector {
            class_name_contains: Some("Box".into()),
            ..EntitySelector::default()
        },
        wait_ms: 1_500,
        ..BoxRule::default()
    };
    let ore_rule = |type_id: Option<i32>| {
        let mut fields = BTreeMap::new();
        if let Some(type_id) = type_id {
            fields.insert("typeID".into(), json!(type_id));
        }
        BoxRule {
            selector: EntitySelector {
                fields: {
                    fields.insert("entityKind".into(), json!("ore"));
                    fields
                },
                ..EntitySelector::default()
            },
            wait_ms: 1_500,
            resource: true,
            ..BoxRule::default()
        }
    };
    let ore_class_rule = |type_id: Option<i32>| {
        let mut fields = BTreeMap::new();
        if let Some(type_id) = type_id {
            fields.insert("typeID".into(), json!(type_id));
        }
        BoxRule {
            selector: EntitySelector {
                class_name_contains: Some("Ore".into()),
                fields,
                ..EntitySelector::default()
            },
            wait_ms: 1_500,
            resource: true,
            ..BoxRule::default()
        }
    };

    match module.module_id.as_str() {
        "kill_collect" => {
            config.mode = BehaviorMode::KillAndCollect;
            config.npc_killer = NpcKillerConfig {
                npcs: configured_npc_rules(module),
                ..NpcKillerConfig::default()
            };
            if enabled("collect_boxes", true) {
                config.collector.boxes.extend(configured_box_rules(
                    module,
                    "boxes",
                    vec![box_rule.clone(), box_class_rule.clone()],
                ));
            }
            if enabled("collect_cargo", true) {
                config.collector.boxes.extend(configured_box_rules(
                    module,
                    "collectables",
                    vec![ore_rule(None), ore_class_rule(None)],
                ));
            }
            config.kill_and_collect = KillAndCollectConfig {
                collect_during_combat: enabled("collect_boxes", true)
                    || enabled("collect_cargo", true),
                ..KillAndCollectConfig::default()
            };
        }
        "npc_killer" => {
            config.mode = BehaviorMode::NpcKiller;
            config.npc_killer = NpcKillerConfig {
                npcs: configured_npc_rules(module),
                ..NpcKillerConfig::default()
            };
        }
        "collect" => {
            config.mode = BehaviorMode::Collector;
            if enabled("collect_boxes", true) {
                config.collector.boxes.extend(configured_box_rules(
                    module,
                    "boxes",
                    vec![box_rule.clone(), box_class_rule.clone()],
                ));
            }
            if enabled("collect_cargo", true) {
                config.collector.boxes.extend(configured_box_rules(
                    module,
                    "collectables",
                    vec![ore_rule(None), ore_class_rule(None)],
                ));
            }
        }
        "palladium" => {
            config.mode = BehaviorMode::Collector;
            config.collector.boxes.extend(configured_box_rules(
                module,
                "collectables",
                vec![ore_rule(Some(8)), ore_class_rule(Some(8))],
            ));
        }
        other => {
            return Err(format!(
                "the behavior engine does not implement the {other} module yet"
            ));
        }
    }
    let number = |key: &str, fallback: f64| {
        setting_value(&module.settings, key)
            .and_then(Value::as_f64)
            .unwrap_or(fallback)
    };
    config.formations = FormationConfig {
        attack: tree_value(module, "general.offensive.formation").and_then(string_or_null),
        roam: tree_value(module, "general.roam.formation").and_then(string_or_null),
        run: tree_value(module, "general.run.formation").and_then(string_or_null),
        repair: tree_value(module, "general.safety.repair.formation").and_then(string_or_null),
    };
    config.collector.cloak_key = tree_value(module, "collect.auto_cloack_key")
        .and_then(string_or_null);
    config.roaming = RoamingConfig {
        keep: tree_value(module, "general.roaming.keep").and_then(Value::as_bool).unwrap_or(true),
        sequential: tree_value(module, "general.roaming.sequential").and_then(Value::as_bool).unwrap_or(false),
        enemy_cbs_invisible: tree_value(module, "general.roaming.enemy_cbs_invisible").and_then(Value::as_bool).unwrap_or(false),
    };
    config.avoidance = AvoidanceConfig {
        mines: tree_value(module, "miscellaneous.avoid_mines").and_then(Value::as_bool).unwrap_or(true),
        cbs: tree_value(module, "miscellaneous.avoid_cbs").and_then(Value::as_bool).unwrap_or(true),
        radiation: tree_value(module, "miscellaneous.avoid_radiation").and_then(Value::as_bool).unwrap_or(true),
    };
    config.auto_refine = tree_value(module, "miscellaneous.auto_refine")
        .and_then(Value::as_bool).unwrap_or(false);
    config.instant_repair_above = tree_value(module, "general.safety.instant_repair")
        .and_then(Value::as_u64).unwrap_or(0);
    config.drone_repair_percent = tree_value(module, "miscellaneous.drone_repair_percentage")
        .and_then(Value::as_f64).unwrap_or(config.drone_repair_percent);
    config.npc_killer.max_circle_iterations = tree_value(module, "loot.max_circle_iterations")
        .and_then(Value::as_u64).unwrap_or(config.npc_killer.max_circle_iterations as u64)
        .clamp(1, 1000) as usize;
    config.npc_killer.run_config_in_circle = tree_value(module, "loot.run_config_in_circle")
        .and_then(Value::as_bool).unwrap_or(config.npc_killer.run_config_in_circle);
    config.npc_killer.group_npcs = tree_value(module, "loot.group_npcs")
        .and_then(Value::as_bool).unwrap_or(true);
    config.npc_killer.ammo_key = tree_value(module, "loot.ammo_key").and_then(string_or_null);
    config.npc_killer.sab_key = tree_value(module, "loot.sab.key").and_then(string_or_null);
    config.npc_killer.rsb_enabled = tree_value(module, "loot.rsb.enabled")
        .and_then(Value::as_bool).unwrap_or(false);
    config.npc_killer.rsb_key = tree_value(module, "loot.rsb.key").and_then(string_or_null);
    config.npc_killer.ship_ability_key = tree_value(module, "loot.ship_ability")
        .and_then(string_or_null);
    config.npc_killer.ship_ability_min = tree_value(module, "loot.ship_ability_min")
        .and_then(Value::as_f64).unwrap_or(config.npc_killer.ship_ability_min);
    config.collector.max_distance = number("max_box_distance", config.collector.max_distance);
    config.collector.pickup_radius = number("pickup_radius", config.collector.pickup_radius);
    config.collector.ignore_contested_boxes = module
        .settings
        .get("ignore_contested_boxes")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    config.collector.stay_away_from_enemies = module
        .settings
        .get("stay_away_from_enemies")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    config.collector.auto_cloak = module
        .settings
        .get("auto_cloak")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    config.npc_killer.max_target_distance =
        number("max_npc_distance", config.npc_killer.max_target_distance);
    config.npc_killer.only_kill_preferred = module
        .settings
        .get("only_kill_preferred")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    config.kill_and_collect.collect_radius =
        number("collect_radius", config.kill_and_collect.collect_radius);
    config.working_map = module
        .settings
        .get("working_map")
        .and_then(|value| resolve_map_id(value, &xenomit_game_api::StarSystem::default()));
    config.pet_enabled = enabled("pet_active", true);
    let flags: Vec<NpcFlag> = serde_json::from_value(
        module
            .settings
            .get("npc_flags")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(|e| format!("invalid NPC flags: {e}"))?;
    for rule in &mut config.npc_killer.npcs {
        rule.combat_radius = number("npc_radius", rule.combat_radius);
        if let Some(row) = table_row(module.settings.get("npc_table"), &rule.selector) {
            rule.combat_radius = row.get("radius").and_then(Value::as_f64).unwrap_or(rule.combat_radius);
            rule.priority = row.get("priority").and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok()).unwrap_or(rule.priority);
            rule.attack_key = row.get("attack_key").and_then(string_or_null);
            rule.attack_formation = row.get("attack_formation").and_then(string_or_null);
            if let Some(extra) = row.get("extra").and_then(Value::as_array) {
                rule.flags = extra.iter().filter_map(Value::as_str).filter_map(npc_flag_from_tree).collect();
            } else {
                rule.flags = flags.clone();
            }
        } else {
            rule.flags = flags.clone();
        }
    }
    for rule in &mut config.collector.boxes {
        rule.wait_ms = module
            .settings
            .get("wait_ms")
            .and_then(Value::as_u64)
            .unwrap_or(rule.wait_ms);
        if let Some(row) = table_row(module.settings.get("box_table"), &rule.selector) {
            rule.wait_ms = row.get("wait").and_then(Value::as_u64).unwrap_or(rule.wait_ms);
            rule.priority = row.get("priority").and_then(Value::as_i64)
                .and_then(|n| i32::try_from(n).ok()).unwrap_or(rule.priority);
        }
    }
    config.validate().map_err(|e| e.to_string())?;
    Ok(config)
}

struct BuiltinModule {
    id: String,
    engine: BehaviorEngine,
    settings: Value,
    safety_config: SafetyConfig,
    safety: SafetyManager,
    traveler: MapTraveler,
    traveling_to: Option<i32>,
    last_configuration: Option<u64>,
    last_pet: Option<u64>,
    last_pet_gear: Option<u64>,
    requested_pet_gear: Option<(i32, i32, Option<i32>)>,
    current_map: Option<i32>,
    combat_progress: Option<(String, f64, f64, u64)>,
    last_item: BTreeMap<String, u64>,
    last_refine: BTreeMap<i32, (u64, u64)>,
    last_refine_at: Option<u64>,
    last_group_command: BTreeMap<String, u64>,
    last_drone_repair: Option<u64>,
}
impl BehaviorModule for BuiltinModule {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        if self.traveling_to.is_some() {
            return InterruptionStatus::Deferred("Waiting for map travel to finish".into());
        }
        crate::interruption::safe_idle_status(context)
    }
    fn export_state(&self, now_ms: u64, _best_effort: bool) -> Option<crate::ModuleState> {
        let data = serde_json::to_value(self.checkpoint(now_ms)).ok()?;
        Some(crate::ModuleState::clean(String::new(), data))
    }
    fn restore_state(
        &mut self,
        state: &crate::ModuleState,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        state.check_schema()?;
        let checkpoint: BuiltinCheckpoint =
            serde_json::from_value(state.data.clone()).map_err(|error| {
                crate::RestoreError::Corrupt(format!("built-in module checkpoint: {error}"))
            })?;
        self.engine.restore_checkpoint(&checkpoint.engine, now_ms)?;
        self.safety.restore_checkpoint(&checkpoint.safety, now_ms)?;
        self.traveler
            .restore_checkpoint(&checkpoint.traveler, now_ms)?;
        self.traveling_to = checkpoint.traveling_to;
        self.last_configuration =
            crate::rebase_age(now_ms, checkpoint.last_configuration_age_ms);
        self.last_pet = crate::rebase_age(now_ms, checkpoint.last_pet_age_ms);
        self.last_pet_gear = crate::rebase_age(now_ms, checkpoint.last_pet_gear_age_ms);
        self.requested_pet_gear = checkpoint.requested_pet_gear;
        self.current_map = checkpoint.current_map;
        self.combat_progress = checkpoint.combat_progress.map(|(key, hp, shield, age)| {
            (key, hp, shield, now_ms.saturating_sub(age))
        });
        self.last_item = checkpoint
            .last_item_ages
            .iter()
            .map(|(item, age)| (item.clone(), now_ms.saturating_sub(*age)))
            .collect();
        Ok(())
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
            return ModuleOutput::running("Waiting for game data");
        };
        // Legacy names are resolved from the live catalog first, then the
        // bundled map table. The latter keeps saved settings usable when a
        // backend does not expose the full XML catalog.
        if self
            .settings
            .get("behavior_config")
            .is_none_or(Value::is_null)
            && let Some(name) = self.settings.get("working_map").and_then(Value::as_str)
        {
            let target = snapshot
                .navigation
                .star_system
                .maps
                .iter()
                .find(|map| map.name.eq_ignore_ascii_case(name))
                .map(|map| map.id)
                .or_else(|| {
                    xenomit_game_api::star_map::MAPS
                        .iter()
                        .find(|(_, map_name, _)| map_name.eq_ignore_ascii_case(name))
                        .map(|(id, _, _)| *id)
                });
            let Some(target) = target else {
                if snapshot.navigation.star_system.maps.is_empty() {
                    return ModuleOutput::running("Waiting for game map catalog");
                }
                return ModuleOutput::failed(format!(
                    "Working map {name} is absent from the game and bundled catalogs"
                ));
            };
            if self.engine.config().working_map != Some(target) {
                let mut config = self.engine.config().clone();
                config.working_map = Some(target);
                if let Err(error) = self.engine.replace_config(config) {
                    return ModuleOutput::failed(error.to_string());
                }
            }
        }
        let mut safety = self.safety.tick(
            snapshot,
            context.now_ms,
            &self.safety_config,
            self.engine.config().coordinates,
        );
        self.attack_observations(snapshot, &mut safety.context, context.now_ms);
        if let Some(error) = safety.failure {
            return ModuleOutput::failed(error);
        }
        let navigation =
            ObstacleNavigation::from_snapshot(snapshot, self.engine.config().coordinates);
        let mut travel = None;
        if safety.context.safe_to_work
            && let Some(target) = self.traveling_to
        {
            if self.engine.config().working_map != Some(target) {
                self.traveling_to = None;
                self.traveler.reset();
            } else {
                let result = self.traveler.tick(snapshot, target, context.now_ms);
                match result.state {
                    TravelState::Failed(error) => return ModuleOutput::failed(error),
                    TravelState::Arrived => self.traveling_to = None,
                    TravelState::Traveling => travel = Some(result),
                }
            }
        }
        let mut plan = if travel.is_some() {
            BehaviorDecision {
                mode: self.engine.config().mode,
                player: snapshot
                    .entities
                    .iter()
                    .find(|e| e.category == EntityCategory::Player)
                    .map(|e| EntityRef {
                        path: e.path.clone(),
                        id: e.id.clone(),
                        name: e.name.clone(),
                        class_name: e.class_name.clone(),
                        position: Point {
                            x: e.position.x,
                            y: e.position.y,
                        },
                    }),
                selected_npc: None,
                selected_box: None,
                actions: vec![],
                note: None,
                suspended: true,
                can_refresh: false,
                controls: ControlRequests {
                    travel_to_map: self.traveling_to,
                    ship_mode: Some(ShipMode::Run),
                    ..Default::default()
                },
            }
        } else {
            self.engine
                .step_with_context(snapshot, context.now_ms, &safety.context, &navigation)
                .into_world_coordinates(self.engine.config().coordinates)
        };
        // Destruction can remove the hero entity. Recovery commands must still
        // execute while ordinary movement/attacks stay suspended.
        if !plan
            .actions
            .iter()
            .any(|action| matches!(action, ActionIntent::WaitForCollection { .. }))
            || snapshot.navigation.hero.killed
        {
            plan.controls.commands.extend(safety.commands);
        }
        if snapshot.navigation.hero.killed {
            plan.suspended = true;
            plan.actions.clear();
            plan.selected_npc = None;
            plan.note = safety.context.safety_status;
        }
        if let Some(target) = plan.controls.travel_to_map {
            self.traveling_to = Some(target);
            let travel =
                travel.unwrap_or_else(|| self.traveler.tick(snapshot, target, context.now_ms));
            if let TravelState::Failed(error) = travel.state {
                return ModuleOutput::failed(error);
            }
            plan.actions = travel
                .destination
                .map(|destination| vec![ActionIntent::MoveTo { destination }])
                .unwrap_or_default();
            plan.controls.commands.extend(travel.command);
            if travel.disable_pet {
                plan.controls.pet_enabled = Some(false);
            }
            plan.note = Some(travel.status);
        }
        self.manager_commands(snapshot, &mut plan, safety.repairing, context.now_ms);
        ModuleOutput {
            state: RunState::Running,
            status: plan.note.clone().unwrap_or_else(|| "Running".into()),
            plan: Some(plan),
        }
    }
    fn update_settings(&mut self, settings: &Value) -> Result<(), String> {
        let safety = safety_config(settings)?;
        self.engine
            .replace_config(builtin_behavior_config(&ModuleConfig {
                module_id: self.id.clone(),
                settings: settings.clone(),
            })?)
            .map_err(|e| e.to_string())?;
        self.safety_config = safety;
        self.settings = settings.clone();
        self.traveler.reset();
        self.traveling_to = None;
        Ok(())
    }
    fn plan_failed(&mut self, plan: &BehaviorDecision) {
        if let Some(target) = &plan.selected_box {
            self.engine.collection_failed(target);
        }
    }
    fn command_succeeded(&mut self, command: &xenomit_game_api::GameCommand, now_ms: u64) {
        if let xenomit_game_api::GameCommand::SelectPetGear { gear_type, target } = command {
            self.requested_pet_gear = Some((*gear_type, *target, self.current_map));
        }
        if let xenomit_game_api::GameCommand::UseItem { item_id, .. } = command {
            self.last_item.insert(item_id.clone(), now_ms);
            if item_id == "equipment_extra_cpu_cl04k" {
                self.engine.cloak_used(now_ms);
            }
        }
    }
}

fn safety_config(settings: &Value) -> Result<SafetyConfig, String> {
    let mut config: SafetyConfig =
        serde_json::from_value(settings.get("safety").cloned().unwrap_or_else(|| json!({})))
            .map_err(|e| format!("Invalid safety settings: {e}"))?;
    let tree = settings.get("config_tree");
    config.revive_location = match tree_value_from(tree, "general.safety.revive").and_then(Value::as_str) {
        Some("portal") => xenomit_game_api::ReviveLocation::Portal,
        Some("spot") => xenomit_game_api::ReviveLocation::Spot,
        _ => xenomit_game_api::ReviveLocation::Base,
    };
    config.revive_wait_after_seconds = tree_value_from(tree, "general.safety.wait_after_revive")
        .and_then(Value::as_u64).unwrap_or(0);
    config.remember_enemies_for_seconds = tree_value_from(tree, "general.running.remember_enemies_for")
        .and_then(Value::as_u64).unwrap_or(config.remember_enemies_for_seconds);
    config.stop_running_no_sight = tree_value_from(tree, "general.running.stop_running_no_sight")
        .and_then(Value::as_bool).unwrap_or(true);
    config.run_port_distance = tree_value_from(tree, "general.running.run_furthest_port")
        .and_then(Value::as_f64).unwrap_or(0.0);
    config.enemies_tag = tree_value_from(tree, "general.running.enemies_tag")
        .and_then(string_or_null);
    config.player_tags = settings.get("player_tags")
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default();
    config.validate()?;
    Ok(config)
}

fn tree_value<'a>(module: &'a ModuleConfig, path: &str) -> Option<&'a Value> {
    tree_value_from(module.settings.get("config_tree"), path)
}

fn tree_value_from<'a>(root: Option<&'a Value>, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root?, |value, key| value.get(key))
}

fn string_or_null(value: &Value) -> Option<String> {
    value.as_str().filter(|value| !value.trim().is_empty()).map(str::to_owned)
}

fn table_row<'a>(table: Option<&'a Value>, selector: &EntitySelector) -> Option<&'a serde_json::Map<String, Value>> {
    let rows = table?.as_object()?;
    rows.iter().find_map(|(name, row)| {
        let short = name.rsplit([':', '.', '/']).next().unwrap_or(name);
        let matches = name == "*"
            || selector.name.as_deref().is_some_and(|value| value.eq_ignore_ascii_case(name) || value.eq_ignore_ascii_case(short))
            || selector.name_contains.as_deref().is_some_and(|value| value.eq_ignore_ascii_case(name) || value.eq_ignore_ascii_case(short))
            || selector.class_name_contains.as_deref().is_some_and(|value| value.eq_ignore_ascii_case(name) || value.eq_ignore_ascii_case(short));
        matches.then_some(row).and_then(Value::as_object)
    })
}

fn npc_flag_from_tree(value: &str) -> Option<NpcFlag> {
    match value.to_ascii_lowercase().replace('_', "").as_str() {
        "nocircle" => Some(NpcFlag::NoCircle),
        "ignoreownership" => Some(NpcFlag::IgnoreOwnership),
        "ignoreattacked" => Some(NpcFlag::IgnoreAttacked),
        "ignoreboxes" => Some(NpcFlag::IgnoreBoxes),
        "aggressivefollow" => Some(NpcFlag::AggressiveFollow),
        "passive" => Some(NpcFlag::Passive),
        "attacksecond" => Some(NpcFlag::AttackSecond),
        "leechonly" => Some(NpcFlag::LeechOnly),
        "nosab" => Some(NpcFlag::NoSab),
        "usersb" => Some(NpcFlag::UseRsb),
        "petlocator" => Some(NpcFlag::PetLocator),
        "oppositeconfig" => Some(NpcFlag::OppositeConfig),
        _ => None,
    }
}

fn hotkey_item(snapshot: &xenomit_game_api::GameSnapshot, key: Option<&str>) -> Option<String> {
    let key = key?.trim();
    if key.is_empty() { return None; }
    snapshot.navigation.features.as_ref()?.slot_bar.as_ref()?
        .iter()
        .find(|slot| slot.key.trim().eq_ignore_ascii_case(key) && !slot.item_id.trim().is_empty())
        .map(|slot| slot.item_id.clone())
}

fn ability_ready(snapshot: &xenomit_game_api::GameSnapshot, item_id: &str, distance: Option<f64>) -> bool {
    snapshot.navigation.features.as_ref()
        .and_then(|features| features.abilities.as_ref())
        .and_then(|abilities| abilities.iter().find(|ability| ability.item_id == item_id))
        .is_some_and(|ability| {
            ability.ready == Some(true)
                && ability.cooldown_ms.unwrap_or(0) == 0
                && distance.is_some_and(|distance| {
                    ability.min_distance.is_none_or(|min| distance >= min)
                        && ability.max_distance.is_none_or(|max| distance <= max)
                })
        })
}

impl BuiltinModule {
    fn attack_observations(
        &mut self,
        snapshot: &xenomit_game_api::GameSnapshot,
        context: &mut ModuleContext,
        now_ms: u64,
    ) {
        context.casting_ability = self
            .settings
            .get("offensive_ability")
            .and_then(Value::as_str)
            .and_then(|id| {
                snapshot
                    .navigation
                    .hero
                    .items
                    .get(id)
                    .map(|item| (id, item))
            })
            .is_some_and(|(id, item)| {
                item.available
                    && item.activatable
                    && !item.blocked
                    && self
                        .last_item
                        .get(id)
                        .is_none_or(|last| now_ms.saturating_sub(*last) >= 5_000)
            });
        for entity in snapshot
            .entities
            .iter()
            .filter(|entity| entity.category == EntityCategory::Npc)
        {
            let key = entity
                .id
                .as_ref()
                .map(|id| format!("id:{id}"))
                .unwrap_or_else(|| format!("path:{}", entity.path));
            let map_id = snapshot
                .navigation
                .map_id
                .or_else(|| snapshot.map.as_ref().and_then(|map| map.id));
            if let Some(rule) = self.engine.npc_rule_on_map(entity, map_id)
                && let Some(observation) = context.entities.get_mut(&key)
            {
                let mut radius = observation.radius.unwrap_or(rule.combat_radius);
                if context.hero_locked_target.as_deref() != Some(key.as_str()) {
                    radius = radius.min(600.0);
                }
                if context.casting_ability {
                    radius = radius.min(
                        self.settings
                            .get("offensive_ability_range")
                            .and_then(Value::as_f64)
                            .unwrap_or(600.0),
                    );
                }
                if observation.moving == Some(false) {
                    radius = radius.min(
                        self.settings
                            .get("stationary_radius")
                            .and_then(Value::as_f64)
                            .unwrap_or(800.0),
                    );
                }
                observation.radius = Some(radius);
            }
        }
        let Some(target) = context.hero_attacking.as_ref().and_then(|target_key| {
            snapshot.entities.iter().find(|entity| {
                let key = entity
                    .id
                    .as_ref()
                    .map(|id| format!("id:{id}"))
                    .unwrap_or_else(|| format!("path:{}", entity.path));
                target_key == &key
            })
        }) else {
            self.combat_progress = None;
            return;
        };
        let Some((hp, shield)) = target
            .health
            .zip(target.shield)
            .filter(|(hp, shield)| hp.is_finite() && shield.is_finite())
        else {
            self.combat_progress = None;
            return;
        };
        let key = context.hero_attacking.clone().unwrap();
        if self
            .combat_progress
            .as_ref()
            .is_none_or(|(previous, last_hp, last_shield, _)| {
                previous != &key || hp < *last_hp || shield < *last_shield
            })
        {
            self.combat_progress = Some((key.clone(), hp, shield, now_ms));
        }
        if self
            .combat_progress
            .as_ref()
            .is_some_and(|(_, _, _, at)| now_ms.saturating_sub(*at) > 30_000)
            && let Some(observation) = context.entities.get_mut(&key)
        {
            observation.bugged = true;
        }
    }
    fn manager_commands(
        &mut self,
        snapshot: &xenomit_game_api::GameSnapshot,
        plan: &mut BehaviorDecision,
        repairing: bool,
        now_ms: u64,
    ) {
        use xenomit_game_api::GameCommand;
        let hero = &snapshot.navigation.hero;
        self.current_map = snapshot.map.as_ref().and_then(|map| map.id);
        if !hero.pet_active {
            self.requested_pet_gear = None;
        }
        if hero.killed {
            return;
        }
        let desired_pet = plan.controls.pet_enabled;
        if hero.owns_pet
            && !hero.pet_destroyed
            && desired_pet.is_some_and(|desired| desired != hero.pet_active)
            && self
                .last_pet
                .is_none_or(|last| now_ms.saturating_sub(last) >= 2_500)
        {
            plan.controls.commands.push(GameCommand::SetPet {
                enabled: desired_pet.unwrap(),
            });
            self.last_pet = Some(now_ms);
        }
        let configuration_key = if repairing {
            "repair_config"
        } else {
            match plan.controls.ship_mode {
                Some(ShipMode::Attack) => "offensive_config",
                Some(ShipMode::Run) => "run_config",
                _ => "roam_config",
            }
        };
        let npc_entity = plan.selected_npc.as_ref().and_then(|target| {
            snapshot
                .entities
                .iter()
                .find(|entity| entity.id == target.id && entity.path == target.path)
        });
        let npc_rule = npc_entity.and_then(|entity| self.engine.npc_rule_on_map(
            entity,
            snapshot.navigation.map_id.or_else(|| snapshot.map.as_ref().and_then(|map| map.id)),
        ));
        let locator_target = npc_entity
            .and_then(|entity| entity.fields.get("npcTypeId"))
            .and_then(Value::as_f64)
            .filter(|n| *n > 0.0 && *n <= f64::from(i32::MAX))
            .map(|n| n as i32);
        let gear =
            if plan.controls.npc_flags.contains(&NpcFlag::PetLocator) && locator_target.is_some() {
                Some(6)
            } else {
                self.settings
                    .get("pet_gear")
                    .and_then(Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok())
            };
        let gear_target =
            if gear == Some(6) && plan.controls.npc_flags.contains(&NpcFlag::PetLocator) {
                locator_target.unwrap_or(0)
            } else {
                self.settings
                    .get("pet_locator_target")
                    .and_then(Value::as_i64)
                    .and_then(|n| i32::try_from(n).ok())
                    .unwrap_or(0)
            };
        let observed_pet_gears = snapshot
            .navigation
            .features
            .as_ref()
            .and_then(|features| features.pet.as_ref())
            .and_then(|pet| pet.gears.as_deref())
            .unwrap_or(&hero.pet_gears);
        if hero.pet_active
            && desired_pet != Some(false)
            && let Some(gear_type) = gear
            && observed_pet_gears.contains(&gear_type)
            && (hero.pet_gear != Some(gear_type) || gear_target != 0)
            && self.requested_pet_gear != Some((gear_type, gear_target, self.current_map))
            && self
                .last_pet_gear
                .is_none_or(|last| now_ms.saturating_sub(last) >= 5_000)
        {
            plan.controls.commands.push(GameCommand::SelectPetGear {
                gear_type,
                target: gear_target,
            });
            self.last_pet_gear = Some(now_ms);
        }
        let mut desired_configuration = self.settings.get(configuration_key)
            .and_then(Value::as_u64).and_then(|n| u8::try_from(n).ok())
            .filter(|n| (1..=2).contains(n));
        if npc_rule.is_some_and(|rule| rule.flags.contains(&NpcFlag::OppositeConfig)) {
            desired_configuration = hero.configuration.map(|current| if current == 1 { 2 } else { 1 });
        }
        if setting_value(&self.settings, "features.switch_configs")
            .and_then(Value::as_bool)
            .unwrap_or(true)
            && let Some(configuration) = desired_configuration
            && hero
                .configuration
                .is_some_and(|current| current != configuration)
            && self
                .last_configuration
                .is_none_or(|last| now_ms.saturating_sub(last) >= 5_000)
        {
            plan.controls
                .commands
                .push(GameCommand::SelectConfiguration { configuration });
            self.last_configuration = Some(now_ms);
        }
        let mut items = Vec::new();
        let formation_key = npc_rule.and_then(|rule| rule.attack_formation.as_deref())
            .or_else(|| match (repairing, plan.controls.ship_mode) {
                (true, _) => self.engine.config().formations.repair.as_deref(),
                (_, Some(ShipMode::Attack)) => self.engine.config().formations.attack.as_deref(),
                (_, Some(ShipMode::Run)) => self.engine.config().formations.run.as_deref(),
                _ => self.engine.config().formations.roam.as_deref(),
            });
        if let Some(id) = hotkey_item(snapshot, formation_key)
            && hero.items.get(&id).is_some_and(|item| item.available && item.activatable && !item.blocked && !item.selected)
        {
            items.push((id, true));
        }
        if plan.controls.cloak {
            let cloak_key = self.engine.config().collector.cloak_key.as_deref();
            let cloak_item = hotkey_item(snapshot, cloak_key)
                .or_else(|| cloak_key.is_none().then_some("equipment_extra_cpu_cl04k".to_owned()));
            if let Some(id) = cloak_item.filter(|id| hero.items.get(id)
                .is_some_and(|item| item.quantity.is_some_and(|n| n > 0.0))) {
                items.push((id, true));
            }
        }
        if plan.selected_npc.is_some() {
            let no_sab = plan.controls.npc_flags.contains(&NpcFlag::NoSab);
            let requested_ammo_key = npc_rule.and_then(|rule| rule.attack_key.as_deref())
                .or(self.engine.config().npc_killer.ammo_key.as_deref());
            let configured_laser = if requested_ammo_key.is_some() {
                hotkey_item(snapshot, requested_ammo_key)
                    .filter(|id| id.starts_with("ammunition_laser_"))
            } else {
                self.settings.get("laser_ammo").and_then(Value::as_str)
                    .filter(|id| !id.is_empty()).map(str::to_owned)
            };
            let fallback = || {
                if requested_ammo_key.is_some() { return None; }
                hero.items
                    .iter()
                    .find(|(id, item)| {
                        id.starts_with("ammunition_laser_")
                            && !id.contains("sab-")
                            && !id.contains("rsb-")
                            && item.available
                            && !item.blocked
                            && item.quantity.is_some_and(|n| n > 0.0)
                    })
                    .map(|(id, _)| id.clone())
            };
            let mut laser = configured_laser.clone();
            if no_sab
                && (configured_laser.as_ref().is_some_and(|id| id.contains("sab-"))
                    || hero
                        .items
                        .iter()
                        .any(|(id, item)| id.contains("sab-") && item.selected))
            {
                laser = configured_laser.clone()
                    .filter(|id| !id.contains("sab-"))
                    .or_else(fallback);
            }
            let sab = hotkey_item(snapshot, self.engine.config().npc_killer.sab_key.as_deref())
                .unwrap_or_else(|| "ammunition_laser_sab-50".into());
            let sab_enabled = self
                .settings
                .get("sab_enabled")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let want_sab = !no_sab
                && sab_enabled
                && crate::condition_matches_snapshot(
                    self.settings.get("sab_condition").unwrap_or(&json!({})),
                    snapshot,
                    now_ms,
                )
                && snapshot
                    .entities
                    .iter()
                    .find(|entity| entity.category == EntityCategory::Player)
                    .and_then(|hero| hero.shield.zip(hero.max_shield))
                    .is_some_and(|(shield, max)| {
                        max > 0.0
                            && shield / max * 100.0
                                < self
                                    .settings
                                    .get("sab_shield_percent")
                                    .and_then(Value::as_f64)
                                    .unwrap_or(50.0)
                    })
                && npc_entity.and_then(|npc| npc.shield).is_some_and(|shield| {
                    shield
                        > self
                            .settings
                            .get("sab_target_shield_min")
                            .and_then(Value::as_f64)
                            .unwrap_or(1000.0)
                })
                && hero.items.get(&sab).is_some_and(|item| {
                    item.available && !item.blocked && item.quantity.is_some_and(|n| n > 0.0)
                });
            if want_sab {
                laser = Some(sab.clone());
            } else if laser.is_none()
                && ((sab_enabled && hero.items.get(&sab).is_some_and(|item| item.selected))
                    || plan.controls.npc_flags.contains(&NpcFlag::UseRsb)
                    || self.engine.config().npc_killer.rsb_enabled)
            {
                laser = fallback();
            }
            if let Some(laser) = laser {
                items.push((laser, false));
            }
            for key in ["rocket_ammo", "offensive_ability"] {
                if let Some(id) = self
                    .settings
                    .get(key)
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                {
                    items.push((id.into(), key == "offensive_ability"));
                }
            }
            if (plan.controls.npc_flags.contains(&NpcFlag::UseRsb)
                || self.engine.config().npc_killer.rsb_enabled)
                && let rsb = hotkey_item(snapshot, self.engine.config().npc_killer.rsb_key.as_deref())
                    .unwrap_or_else(|| "ammunition_laser_rsb-75".into())
                && let Some(item) = hero.items.get(&rsb).filter(|item| {
                    item.available && !item.blocked && item.quantity.is_some_and(|n| n > 0.0)
                })
            {
                items.push((rsb, item.activatable));
            }
            if let (Some(key), Some(npc)) = (self.engine.config().npc_killer.ship_ability_key.as_deref(), npc_entity) {
                let distance = snapshot.entities.iter()
                    .find(|entity| entity.category == EntityCategory::Player)
                    .map(|player| (player.position.x - npc.position.x).hypot(player.position.y - npc.position.y));
                if npc.health.is_some_and(|health| health >= self.engine.config().npc_killer.ship_ability_min)
                    && let Some(id) = hotkey_item(snapshot, Some(key))
                    && ability_ready(snapshot, &id, distance)
                    && hero.items.get(&id).is_some_and(|item| item.available && item.activatable && !item.blocked)
                {
                    items.push((id, true));
                }
            }
        } else if plan.suspended
            && plan.controls.travel_to_map.is_none()
            && self.safety_config.enabled
            && let Some(id) = self
                .settings
                .get("defensive_ability")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
        {
            items.push((id.into(), true));
        }
        if plan.suspended
            && !hero.killed
            && self.safety_config.enabled
            && plan.controls.ship_mode == Some(ShipMode::Run)
            && let Some(key) = tree_value_from(
                self.settings.get("config_tree"),
                "general.running.ship_ability",
            )
            .and_then(string_or_null)
            && let Some(destination) = plan.actions.iter().find_map(|action| match action {
                ActionIntent::MoveTo { destination } => Some(*destination),
                _ => None,
            })
            && let Some(distance) = plan.player.as_ref().map(|player| {
                (destination.x - player.position.x).hypot(destination.y - player.position.y)
            })
            && distance
                >= tree_value_from(
                    self.settings.get("config_tree"),
                    "general.running.ship_ability_min",
                )
                .and_then(Value::as_f64)
                .unwrap_or(1500.0)
            && let Some(id) = hotkey_item(snapshot, Some(&key))
            && ability_ready(snapshot, &id, Some(distance))
        {
            items.push((id, true));
        }

        if repairing
            && self.engine.config().instant_repair_above > 0
            && let Some(inventory) = snapshot
                .navigation
                .features
                .as_ref()
                .and_then(|features| features.repair_inventory.as_ref())
            && inventory
                .instant_repair
                .is_some_and(|count| count > self.engine.config().instant_repair_above as f64)
            && let Some(id) = inventory.instant_repair_item_id.as_deref()
            && hero.items.get(id).is_some_and(|item| {
                item.available && item.activatable && !item.blocked && !item.selected
            })
        {
            items.push((id.to_owned(), true));
        }

        if !plan.suspended
            && self.engine.config().auto_refine
            && plan.selected_npc.is_none()
            && self.last_refine_at.is_none_or(|last| now_ms.saturating_sub(last) >= 3_000)
            && let Some((ore_type, amount)) = snapshot
                .navigation
                .features
                .as_ref()
                .and_then(|features| features.economy.as_ref())
                .and_then(|economy| {
                    economy.ores.iter().find_map(|(ore_type, count)| {
                        let amount = *count;
                        (amount.is_finite() && amount >= 1.0)
                            .then(|| u64::try_from(amount.floor() as u128).ok())
                            .flatten()
                            .filter(|amount| {
                                self.last_refine.get(ore_type).is_none_or(|(previous, at)| {
                                    *previous != *amount || now_ms.saturating_sub(*at) >= 60_000
                                })
                            })
                            .map(|amount| (*ore_type, amount))
                    })
                })
        {
            plan.controls
                .commands
                .push(GameCommand::RefineOre { ore_type, amount });
            self.last_refine.insert(ore_type, (amount, now_ms));
            self.last_refine_at = Some(now_ms);
        }

        if !hero.killed
            && self
                .last_drone_repair
                .is_none_or(|last| now_ms.saturating_sub(last) >= 30_000)
            && snapshot
                .navigation
                .features
                .as_ref()
                .and_then(|features| features.drones.as_ref())
                .is_some_and(|drones| {
                    drones.observed
                        && drones.damaged.is_some_and(|count| count > 0)
                        && drones.health_percent.is_some_and(|health| {
                            health.is_finite()
                                && health < self.engine.config().drone_repair_percent * 100.0
                        })
                })
        {
            plan.controls.commands.push(GameCommand::RepairDrones);
            self.last_drone_repair = Some(now_ms);
        }

        self.group_commands(snapshot, plan, now_ms);
        for (id, activate) in items {
            if hero.items.get(&id).is_some_and(|item| {
                item.available
                    && !item.blocked
                    && (!activate || item.activatable)
                    && (activate || !item.selected)
            }) && self
                .last_item
                .get(&id)
                .is_none_or(|last| now_ms.saturating_sub(*last) >= 5_000)
            {
                plan.controls.commands.push(GameCommand::UseItem {
                    item_id: id,
                    activate,
                });
            }
        }
    }

    fn group_commands(
        &mut self,
        snapshot: &xenomit_game_api::GameSnapshot,
        plan: &mut BehaviorDecision,
        now_ms: u64,
    ) {
        use xenomit_game_api::GameCommand;
        let Some(group) = snapshot
            .navigation
            .features
            .as_ref()
            .and_then(|features| features.group.as_ref())
            .or_else(|| Some(&snapshot.navigation.group))
        else {
            return;
        };
        let tree = self.settings.get("config_tree");
        let Some(settings) = tree.and_then(|root| root.get("group")) else {
            return;
        };
        let player_tags: BTreeMap<i32, String> = self
            .settings
            .get("player_tags")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let tag_for = |id: i32| player_tags.get(&id).map(String::as_str);
        let whitelist = settings.get("whitelist_tag").and_then(string_or_null);
        let invite_tag = settings.get("invite_tag").and_then(string_or_null);
        let self_id = snapshot
            .entities
            .iter()
            .find(|entity| entity.category == EntityCategory::Player)
            .and_then(|entity| entity.id.as_ref())
            .and_then(value_i32);
        let queue = |key: String, command: GameCommand, plan: &mut BehaviorDecision, last: &mut BTreeMap<String, u64>| {
            if last
                .get(&key)
                .is_none_or(|at| now_ms.saturating_sub(*at) >= 15_000)
            {
                plan.controls.commands.push(command);
                last.insert(key, now_ms);
            }
        };

        if settings.get("accept_invites").and_then(Value::as_bool) == Some(true) {
            for invite in &group.pending_invites {
                let accepted = whitelist.as_deref().is_none_or(|wanted| {
                    tag_for(invite.player_id).is_some_and(|tag| tag.eq_ignore_ascii_case(wanted))
                });
                if accepted {
                    queue(
                        format!("accept:{}", invite.player_id),
                        GameCommand::AcceptGroupInvite {
                            inviter_id: invite.player_id,
                        },
                        plan,
                        &mut self.last_group_command,
                    );
                }
            }
        }

        if settings.get("block_invites").and_then(Value::as_bool) == Some(true) {
            if group.invites_open != Some(false) {
                queue(
                    "invites:false".into(),
                    GameCommand::SetGroupInvites { open: false },
                    plan,
                    &mut self.last_group_command,
                );
            }
        } else if settings.get("open_invites").and_then(Value::as_bool) == Some(true)
            && self_id.is_some()
            && group.leader_id == self_id
            && group.invites_open != Some(true)
        {
            queue(
                "invites:true".into(),
                GameCommand::SetGroupInvites { open: true },
                plan,
                &mut self.last_group_command,
            );
        }

        if settings.get("leave_no_whitelisted").and_then(Value::as_bool) == Some(true)
            && group.active == Some(true)
            && let Some(wanted) = whitelist.as_deref()
            && self_id.is_some()
            && !group.members.iter().any(|member| {
                Some(member.id) != self_id
                    && tag_for(member.id).is_some_and(|tag| tag.eq_ignore_ascii_case(wanted))
            })
        {
            queue(
                "leave".into(),
                GameCommand::LeaveGroup,
                plan,
                &mut self.last_group_command,
            );
        }

        if settings.get("kick_no_invited").and_then(Value::as_bool) == Some(true)
            && let Some(wanted) = invite_tag.as_deref()
        {
            for member in &group.members {
                if Some(member.id) != self_id
                    && !tag_for(member.id).is_some_and(|tag| tag.eq_ignore_ascii_case(wanted))
                {
                    queue(
                        format!("kick:{}", member.id),
                        GameCommand::KickGroupMember {
                            player_id: member.id,
                        },
                        plan,
                        &mut self.last_group_command,
                    );
                }
            }
        }

        if let Some(wanted) = invite_tag.as_deref() {
            for entity in snapshot
                .entities
                .iter()
                .filter(|entity| entity.category == EntityCategory::Player)
            {
                let Some(id) = entity.id.as_ref().and_then(value_i32) else {
                    continue;
                };
                if Some(id) == self_id
                    || group.members.iter().any(|member| member.id == id)
                    || group.pending_invites.iter().any(|invite| invite.player_id == id)
                    || !tag_for(id).is_some_and(|tag| tag.eq_ignore_ascii_case(wanted))
                {
                    continue;
                }
                queue(
                    format!("invite:{id}"),
                    GameCommand::InviteGroupMember { player_id: id },
                    plan,
                    &mut self.last_group_command,
                );
            }
        }
    }
}

fn value_i32(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .or_else(|| value.as_u64().and_then(|id| i32::try_from(id).ok()))
        .or_else(|| value.as_str().and_then(|id| id.parse().ok()))
}

/// Register custom modules on this builder before freezing it for the host.
pub fn builtin_registry_builder() -> ModuleRegistryBuilder {
    let mut builder = ModuleRegistryBuilder::new();
    for (id, name, mode) in [
        (
            "kill_collect",
            "Kill & Collect",
            BehaviorMode::KillAndCollect,
        ),
        ("npc_killer", "NPC killer", BehaviorMode::NpcKiller),
        ("collect", "Collect", BehaviorMode::Collector),
        ("palladium", "Palladium", BehaviorMode::Collector),
    ] {
        let mut settings = vec![
            SettingDescriptor::new("working_map", "Working map", SettingKind::Map, Value::Null)
                .group("Navigation").description("Select a map from the game's catalog. The bot follows the fewest portal jumps; current map keeps work local."),
            SettingDescriptor::new(
                "tick_ms",
                "Tick interval (ms)",
                SettingKind::Integer {
                    min: 10,
                    max: 60_000,
                },
                json!(150),
            )
            .group("Scheduling"),
        ];
        if matches!(id, "kill_collect" | "npc_killer") {
            settings.push(
                SettingDescriptor::new(
                    "npc_preset",
                    "NPC group preset",
                    SettingKind::Choice {
                        options: crate::modules::presets::npc_preset_choices(),
                    },
                    json!("none"),
                )
                .group("Combat")
                .description("Load per-map NPC radii and priority defaults. Species checkboxes can still disable individual entries."),
            );
        }
        if matches!(id, "kill_collect" | "collect") {
            settings.push(
                SettingDescriptor::new(
                    "box_preset",
                    "Box group preset",
                    SettingKind::Choice {
                        options: crate::modules::presets::box_preset_choices(),
                    },
                    json!("all"),
                )
                .group("Collection")
                .description("Limit collection to the selected box group and use its default waits and priorities."),
            );
        }
        settings.extend([
            SettingDescriptor::new(
                "features.pet_active",
                "Enable pet",
                SettingKind::Boolean,
                json!(true),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "pet_locator_target",
                "Locator NPC/resource type ID (0 = default)",
                SettingKind::Integer {
                    min: 0,
                    max: i64::from(i32::MAX),
                },
                json!(0),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "pet_gear",
                "Pet behavior",
                SettingKind::Choice {
                    options: [
                        ("Keep current", Value::Null),
                        ("Passive", json!(1)),
                        ("Guard", json!(2)),
                        ("Auto loot", json!(4)),
                        ("Resource collection", json!(5)),
                        ("Enemy locator", json!(6)),
                        ("Resource locator", json!(7)),
                        ("Ship repair", json!(11)),
                    ]
                    .into_iter()
                    .map(|(label, value)| SettingChoice {
                        label: label.into(),
                        value,
                    })
                    .collect(),
                },
                Value::Null,
            )
            .group("Ship"),
            SettingDescriptor::new(
                "features.switch_configs",
                "Switch ship configurations",
                SettingKind::Boolean,
                json!(true),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "offensive_config",
                "Attack configuration",
                SettingKind::Integer { min: 1, max: 2 },
                json!(1),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "roam_config",
                "Roam configuration",
                SettingKind::Integer { min: 1, max: 2 },
                json!(2),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "run_config",
                "Escape/travel configuration",
                SettingKind::Integer { min: 1, max: 2 },
                json!(2),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "repair_config",
                "Repair configuration",
                SettingKind::Integer { min: 1, max: 2 },
                json!(1),
            )
            .group("Ship"),
            SettingDescriptor::new(
                "safety.enabled",
                "Enable safety and repair",
                SettingKind::Boolean,
                json!(true),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.min_health_percent",
                "Retreat below health (%)",
                SettingKind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                json!(40),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.max_health_percent",
                "Resume at health (%)",
                SettingKind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                json!(95),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.roaming_repair_health_percent",
                "Repair while roaming below (%)",
                SettingKind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                json!(50),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.repair_shield_percent",
                "Resume at shield (%)",
                SettingKind::Number {
                    min: 0.0,
                    max: 100.0,
                },
                json!(100),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.run_from_enemy",
                "Retreat when attacked by enemies",
                SettingKind::Boolean,
                json!(true),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.run_in_sight",
                "Retreat from visible enemies",
                SettingKind::Boolean,
                json!(false),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.sight_distance",
                "Enemy sight distance",
                SettingKind::Number {
                    min: 0.0,
                    max: 100_000.0,
                },
                json!(2500),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.max_deaths",
                "Stop after deaths (0 = unlimited)",
                SettingKind::Integer { min: 0, max: 1000 },
                json!(10),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.auto_revive",
                "Use offered free base repair",
                SettingKind::Boolean,
                json!(true),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "safety.revive_wait_seconds",
                "Revive delay (seconds)",
                SettingKind::Integer { min: 0, max: 3600 },
                json!(5),
            )
            .group("Safety"),
            SettingDescriptor::new(
                "defensive_ability",
                "Defensive item ID (empty disables)",
                SettingKind::Text,
                json!(""),
            )
            .group("Safety"),
        ]);
        if mode != BehaviorMode::NpcKiller {
            if id != "palladium" {
                settings.extend([
                    SettingDescriptor::new(
                        "features.collect_boxes",
                        "Collect boxes",
                        SettingKind::Boolean,
                        json!(true),
                    )
                    .group("Collection"),
                    SettingDescriptor::new(
                        "features.collect_cargo",
                        "Collect cargo",
                        SettingKind::Boolean,
                        json!(true),
                    )
                    .group("Collection"),
                ]);
            }
            settings.extend([
                SettingDescriptor::new(
                    "auto_cloak",
                    "Use available cloak charges",
                    SettingKind::Boolean,
                    json!(false),
                )
                .group("Collection"),
                SettingDescriptor::new(
                    "stay_away_from_enemies",
                    "Keep collection away from enemies",
                    SettingKind::Boolean,
                    json!(false),
                )
                .group("Collection"),
                SettingDescriptor::new(
                    "max_box_distance",
                    "Maximum box distance",
                    SettingKind::Number {
                        min: 0.0,
                        max: 100_000.0,
                    },
                    json!(5000.0),
                )
                .group("Collection"),
                SettingDescriptor::new(
                    "pickup_radius",
                    "Pickup radius",
                    SettingKind::Number {
                        min: 1.0,
                        max: 1000.0,
                    },
                    json!(250.0),
                )
                .group("Collection"),
                SettingDescriptor::new(
                    "wait_ms",
                    "Pickup wait (ms)",
                    SettingKind::Integer {
                        min: 0,
                        max: 60_000,
                    },
                    json!(1500),
                )
                .group("Collection"),
                SettingDescriptor::new(
                    "ignore_contested_boxes",
                    "Ignore contested boxes",
                    SettingKind::Boolean,
                    json!(false),
                )
                .group("Collection"),
            ]);
        }
        if mode != BehaviorMode::Collector {
            settings.extend([
                SettingDescriptor::new(
                    "laser_ammo",
                    "Laser ammunition item ID",
                    SettingKind::Text,
                    json!(""),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "sab_enabled",
                    "Select SAB when shields are low",
                    SettingKind::Boolean,
                    json!(false),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "sab_shield_percent",
                    "Use SAB below hero shield (%)",
                    SettingKind::Number {
                        min: 0.0,
                        max: 100.0,
                    },
                    json!(50),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "sab_target_shield_min",
                    "SAB minimum target shield",
                    SettingKind::Number {
                        min: 0.0,
                        max: 10_000_000.0,
                    },
                    json!(1000),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "rocket_ammo",
                    "Rocket ammunition item ID",
                    SettingKind::Text,
                    json!(""),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "offensive_ability",
                    "Offensive item ID (empty disables)",
                    SettingKind::Text,
                    json!(""),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "offensive_ability_range",
                    "Offensive ability approach radius",
                    SettingKind::Number {
                        min: 0.0,
                        max: 5000.0,
                    },
                    json!(600),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "stationary_radius",
                    "Maximum stationary target radius",
                    SettingKind::Number {
                        min: 0.0,
                        max: 5000.0,
                    },
                    json!(800),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "npc_radius",
                    "Combat radius",
                    SettingKind::Number {
                        min: 0.0,
                        max: 5000.0,
                    },
                    json!(560.0),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "max_npc_distance",
                    "Maximum NPC distance",
                    SettingKind::Number {
                        min: 0.0,
                        max: 100_000.0,
                    },
                    json!(5000.0),
                )
                .group("Combat"),
                SettingDescriptor::new(
                    "only_kill_preferred",
                    "Only target preferred zones",
                    SettingKind::Boolean,
                    json!(false),
                )
                .group("Combat"),
                SettingDescriptor::new("npc_flags", "NPC flags", SettingKind::Json, json!([]))
                    .group("Combat"),
            ]);
        }
        if mode == BehaviorMode::KillAndCollect {
            settings.push(
                SettingDescriptor::new(
                    "collect_radius",
                    "Collection radius during combat",
                    SettingKind::Number {
                        min: 0.0,
                        max: 100_000.0,
                    },
                    json!(500.0),
                )
                .group("Collection"),
            );
        }
        settings.push(
            SettingDescriptor::new(
                "behavior_config",
                "Advanced behavior configuration",
                SettingKind::Json,
                Value::Null,
            )
            .group("Advanced")
            .description("A non-null object overrides the planner settings above."),
        );
        let descriptor = ModuleDescriptor {
            id: id.into(),
            name: name.into(),
            description: match id {
                "kill_collect" => "Fight NPCs and collect without interrupting pickups.",
                "npc_killer" => "Fight configured NPCs and roam between targets.",
                "palladium" => "Collect palladium resources.",
                _ => "Collect boxes and resources.",
            }
            .into(),
            settings,
            required_services: vec![],
        };
        builder
            .register(descriptor, move |settings| {
                let config = builtin_behavior_config(&ModuleConfig {
                    module_id: id.into(),
                    settings: settings.clone(),
                })?;
                Ok(Box::new(BuiltinModule {
                    id: id.into(),
                    engine: BehaviorEngine::new(config).map_err(|e| e.to_string())?,
                    settings: settings.clone(),
                    safety_config: safety_config(settings)?,
                    safety: SafetyManager::default(),
                    traveler: MapTraveler::default(),
                    traveling_to: None,
                    last_configuration: None,
                    last_pet: None,
                    last_pet_gear: None,
                    requested_pet_gear: None,
                    current_map: None,
                    combat_progress: None,
                    last_item: BTreeMap::new(),
                    last_refine: BTreeMap::new(),
                    last_refine_at: None,
                    last_group_command: BTreeMap::new(),
                    last_drone_repair: None,
                }))
            })
            .expect("built-in descriptor is valid");
    }
    crate::modules::game_modes::register(&mut builder);
    crate::modules::cubikon::register(&mut builder);
    crate::modules::item_restock::register(&mut builder);
    builder
}
pub fn builtin_registry() -> ModuleRegistry {
    builtin_registry_builder().build()
}

/// NPC allowlist from the `npcs` module setting — a `species → enabled` map
/// the GUI auto-populates from live frames the first time each species is
/// seen. Each enabled entry is matched against the entity `name` and its
/// class name in both stored and short (last `::`/`.`/`/` segment) forms:
/// the name surface can flip between a display name and the qualified class
/// name depending on which bindables the game populated, so both are tried.
/// An absent or empty map means every NPC is a valid target; a populated
/// map with every entry disabled targets nothing.
fn configured_npc_rules(module: &ModuleConfig) -> Vec<NpcRule> {
    let all_npcs = || NpcRule {
        selector: EntitySelector {
            category: Some(EntityCategory::Npc),
            ..EntitySelector::default()
        },
        ..NpcRule::default()
    };
    let species = module.settings.get("npcs").and_then(Value::as_object);
    let preset_id = module
        .settings
        .get("npc_preset")
        .and_then(Value::as_str)
        .unwrap_or("none");
    if let Some(mut rules) = crate::modules::presets::npc_preset_rules(preset_id) {
        if let Some(species) = species {
            for rule in &mut rules {
                let Some(name) = rule.selector.name.as_deref() else {
                    continue;
                };
                if let Some((_, enabled)) = species.iter().find(|(configured, _)| {
                    configured.eq_ignore_ascii_case(name)
                        || configured
                            .rsplit([':', '.', '/'])
                            .next()
                            .is_some_and(|short| short.eq_ignore_ascii_case(name))
                }) {
                    rule.kill = enabled.as_bool().unwrap_or(false);
                }
            }
            for (name, _) in species
                .iter()
                .filter(|(_, enabled)| enabled.as_bool() == Some(true))
            {
                if rules.iter().any(|rule| {
                    rule.selector
                        .name
                        .as_deref()
                        .is_some_and(|configured| configured.eq_ignore_ascii_case(name))
                }) {
                    continue;
                }
                rules.push(NpcRule {
                    selector: EntitySelector {
                        category: Some(EntityCategory::Npc),
                        name: Some(name.clone()),
                        ..EntitySelector::default()
                    },
                    ..NpcRule::default()
                });
            }
        }
        return rules.into_iter().filter(|rule| rule.kill).collect();
    }
    if species.and_then(|entries| entries.get("*")).and_then(Value::as_bool) == Some(true) {
        let mut rules = vec![all_npcs()];
        for (name, _enabled) in species.into_iter().flat_map(|entries| entries.iter())
            .filter(|(name, enabled)| name.as_str() != "*" && enabled.as_bool() == Some(true))
        {
            let short = name.rsplit([':', '.', '/']).next().unwrap_or(name).to_owned();
            for selector_name in [name.clone(), short] {
                rules.push(NpcRule {
                    selector: EntitySelector { category: Some(EntityCategory::Npc), name_contains: Some(selector_name), ..EntitySelector::default() },
                    ..NpcRule::default()
                });
            }
        }
        return rules;
    }
    let Some(species) = species.filter(|species| !species.is_empty()) else {
        return vec![all_npcs()];
    };
    species
        .iter()
        .filter(|(_, enabled)| enabled.as_bool().unwrap_or(false))
        .flat_map(|(name, _)| {
            let short = name
                .rsplit([':', '.', '/'])
                .find(|segment| !segment.is_empty())
                .unwrap_or(name.as_str())
                .to_owned();
            [name.clone(), short]
                .into_iter()
                .flat_map(|name| {
                    [
                        EntitySelector {
                            category: Some(EntityCategory::Npc),
                            name_contains: Some(name.clone()),
                            ..EntitySelector::default()
                        },
                        EntitySelector {
                            category: Some(EntityCategory::Npc),
                            class_name_contains: Some(name),
                            ..EntitySelector::default()
                        },
                    ]
                })
                .collect::<Vec<_>>()
        })
        .map(|selector| NpcRule {
            selector,
            ..NpcRule::default()
        })
        .collect()
}

/// A populated shared list becomes an explicit allowlist. Keeping the
/// previous selector when the list is empty preserves the historical
/// collect-everything behavior until the first live discovery.
fn configured_box_rules(module: &ModuleConfig, key: &str, defaults: Vec<BoxRule>) -> Vec<BoxRule> {
    let preset_id = if key == "boxes" {
        module
            .settings
            .get("box_preset")
            .and_then(Value::as_str)
            .unwrap_or("all")
    } else {
        "all"
    };
    let preset_rows = crate::modules::presets::box_preset_entries(preset_id);
    let Some(entries) = module
        .settings
        .get(key)
        .and_then(Value::as_object)
        .filter(|entries| !entries.is_empty())
    else {
        if let Some(rules) = crate::modules::presets::box_preset_rules(preset_id) {
            return rules;
        }
        return defaults;
    };
    let mut rules = Vec::new();
    for (name, enabled) in entries {
        if enabled.as_bool() != Some(true) {
            continue;
        }
        for template in &defaults {
            for match_name in [true, false] {
                let mut rule = template.clone();
                if let Some(row) = preset_rows
                    .as_ref()
                    .and_then(|rows| rows.iter().find(|row| row.name.eq_ignore_ascii_case(name)))
                {
                    rule.wait_ms = row.wait_ms;
                    rule.priority = row.priority;
                }
                if match_name {
                    rule.selector.name_contains = Some(name.clone());
                } else {
                    rule.selector.class_name_contains = Some(name.clone());
                }
                rules.push(rule);
            }
        }
    }
    rules
}

/// Retained state of the shared collector/NPC objective wrapper. Engine,
/// safety and traveler sub-checkpoints carry the detailed planner state.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct BuiltinCheckpoint {
    engine: crate::EngineCheckpoint,
    safety: crate::SafetyCheckpoint,
    traveler: crate::TravelerCheckpoint,
    traveling_to: Option<i32>,
    last_configuration_age_ms: Option<u64>,
    last_pet_age_ms: Option<u64>,
    last_pet_gear_age_ms: Option<u64>,
    /// (gear_type, target, map) of the gear selection in flight.
    requested_pet_gear: Option<(i32, i32, Option<i32>)>,
    current_map: Option<i32>,
    /// (target key, last hp, last shield, age of the observation).
    combat_progress: Option<(String, f64, f64, u64)>,
    /// Item id -> age since last use.
    last_item_ages: BTreeMap<String, u64>,
}

impl BuiltinModule {
    fn checkpoint(&self, now_ms: u64) -> BuiltinCheckpoint {
        BuiltinCheckpoint {
            engine: self.engine.export_checkpoint(now_ms),
            safety: self.safety.export_checkpoint(now_ms),
            traveler: self.traveler.export_checkpoint(now_ms),
            traveling_to: self.traveling_to,
            last_configuration_age_ms: crate::age_ms(self.last_configuration, now_ms),
            last_pet_age_ms: crate::age_ms(self.last_pet, now_ms),
            last_pet_gear_age_ms: crate::age_ms(self.last_pet_gear, now_ms),
            requested_pet_gear: self.requested_pet_gear,
            current_map: self.current_map,
            combat_progress: self.combat_progress.as_ref().map(|(key, hp, shield, at)| {
                (key.clone(), *hp, *shield, now_ms.saturating_sub(*at))
            }),
            last_item_ages: self
                .last_item
                .iter()
                .map(|(item, at)| (item.clone(), now_ms.saturating_sub(*at)))
                .collect(),
        }
    }
}
