//! Cubikon farming objective using the shared combat, navigation, and safety planners.
use crate::{
    ActionIntent, BehaviorConfig, BehaviorDecision, BehaviorEngine, BehaviorMode, BehaviorModule,
    BoxRule, ControlRequests, EntityRef, EntitySelector, InterruptionContext, InterruptionReason,
    InterruptionStatus, MapTraveler, ModuleOutput, ModuleRegistryBuilder, NpcFlag, NpcRule,
    ObstacleNavigation, Point, RunState, SafetyConfig, SafetyManager, ShipMode, TickContext,
    TravelState,
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xenomit_game_api::{
    EntityCategory, GameCommand, GameSnapshot, ModuleDescriptor, SettingDescriptor, SettingKind,
};

const CUBIKON_NAME: &str = "-=[ Cubikon ]=-";
const KRISTALLIN_NAMES: &[&str] = &["-=[ Kristallin ]=-", "..::{ Boss Kristallin }::.."];
const PATROL_POINTS: [Point; 4] = [
    Point {
        x: 7500.0,
        y: 8000.0,
    },
    Point {
        x: 13500.0,
        y: 8000.0,
    },
    Point {
        x: 13500.0,
        y: 4000.0,
    },
    Point {
        x: 7500.0,
        y: 4000.0,
    },
];

pub fn register(builder: &mut ModuleRegistryBuilder) {
    builder
        .register(descriptor(), |settings| {
            Ok(Box::new(CubikonModule::new(settings)?))
        })
        .expect("Cubikon module descriptor is valid");
}

fn descriptor() -> ModuleDescriptor {
    let settings = vec![
        SettingDescriptor::new(
            "working_map",
            "Working map (blank uses faction X-6)",
            SettingKind::Map,
            Value::Null,
        )
        .group("Navigation"),
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
        SettingDescriptor::new(
            "npc_radius",
            "Combat radius",
            SettingKind::Number {
                min: 0.0,
                max: 5000.0,
            },
            json!(550.0),
        )
        .group("Combat"),
        SettingDescriptor::new(
            "kill_kristallins_while_waiting",
            "Fight Kristallins attacking the hero",
            SettingKind::Boolean,
            json!(true),
        )
        .group("Combat"),
        SettingDescriptor::new(
            "features.pet_active",
            "Enable pet",
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
            "repair_config",
            "Repair configuration",
            SettingKind::Integer { min: 1, max: 2 },
            json!(2),
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
            json!(40.0),
        )
        .group("Safety"),
        SettingDescriptor::new(
            "safety.max_health_percent",
            "Resume at health (%)",
            SettingKind::Number {
                min: 0.0,
                max: 100.0,
            },
            json!(95.0),
        )
        .group("Safety"),
    ];
    ModuleDescriptor {
        id: "cubikon_hunter".into(),
        name: "Cubikon hunter".into(),
        description: "Patrol the faction X-6 map, attack Cubikon when it appears, and protect the hero from nearby Kristallins.".into(),
        settings,
        required_services: vec![],
    }
}

struct CubikonModule {
    settings: Value,
    engine: BehaviorEngine,
    safety_config: SafetyConfig,
    safety: SafetyManager,
    traveler: MapTraveler,
    travel_active: bool,
    last_command: Option<u64>,
    patrol_last_visit: [Option<u64>; 4],
    patrol_target: Option<usize>,
}

impl CubikonModule {
    fn new(settings: &Value) -> Result<Self, String> {
        Ok(Self {
            settings: settings.clone(),
            engine: BehaviorEngine::new(behavior_config(settings)?)
                .map_err(|error| error.to_string())?,
            safety_config: safety_config(settings)?,
            safety: SafetyManager::default(),
            traveler: MapTraveler::default(),
            travel_active: false,
            last_command: None,
            patrol_last_visit: [None; 4],
            patrol_target: None,
        })
    }

    fn target_map(&self, snapshot: &GameSnapshot) -> Option<i32> {
        if let Some(map) = self.settings.get("working_map").and_then(map_value) {
            return Some(map);
        }
        let faction = snapshot.navigation.hero.faction?;
        let name = format!("{faction}-6");
        snapshot
            .navigation
            .star_system
            .maps
            .iter()
            .find(|map| map.name.eq_ignore_ascii_case(&name))
            .map(|map| map.id)
            .or_else(|| {
                xenomit_game_api::star_map::MAPS
                    .iter()
                    .find(|(_, map_name, _)| map_name.eq_ignore_ascii_case(&name))
                    .map(|(id, _, _)| *id)
            })
    }

    fn patrol(&mut self, snapshot: &GameSnapshot, now_ms: u64) -> Option<(Point, usize)> {
        let player = snapshot
            .entities
            .iter()
            .find(|entity| entity.category == EntityCategory::Player)?;
        let transform = self.engine.config().coordinates;
        let (x, y) = transform.apply(player.position.x, player.position.y);
        let position = Point { x, y };

        if let Some(index) = self.patrol_target
            && distance(position, transform_point(PATROL_POINTS[index], transform)) <= 250.0
        {
            self.patrol_last_visit[index] = Some(now_ms);
            self.patrol_target = None;
        }
        let index = *self.patrol_target.get_or_insert_with(|| {
            (0..PATROL_POINTS.len())
                .min_by_key(|index| self.patrol_last_visit[*index].unwrap_or(0))
                .unwrap_or(0)
        });
        Some((transform_point(PATROL_POINTS[index], transform), index))
    }
}

impl BehaviorModule for CubikonModule {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        if self.travel_active {
            return InterruptionStatus::Deferred("Waiting for travel to the farming map".into());
        }
        if self
            .last_command
            .is_some_and(|at| context.now_ms.saturating_sub(at) < 2_500)
        {
            return InterruptionStatus::Deferred(
                "Waiting for the last game command to settle".into(),
            );
        }
        crate::interruption::safe_idle_status(context)
    }
    fn export_state(&self, now_ms: u64, _best_effort: bool) -> Option<crate::ModuleState> {
        let checkpoint = CubikonCheckpoint {
            engine: self.engine.export_checkpoint(now_ms),
            safety: self.safety.export_checkpoint(now_ms),
            traveler: self.traveler.export_checkpoint(now_ms),
            travel_active: self.travel_active,
            last_command_age_ms: crate::age_ms(self.last_command, now_ms),
            patrol_last_visit_ages: self
                .patrol_last_visit
                .map(|visit| crate::age_ms(visit, now_ms)),
            patrol_target: self.patrol_target,
        };
        let data = serde_json::to_value(&checkpoint).ok()?;
        Some(crate::ModuleState::clean(String::new(), data))
    }
    fn restore_state(
        &mut self,
        state: &crate::ModuleState,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        state.check_schema()?;
        let checkpoint: CubikonCheckpoint =
            serde_json::from_value(state.data.clone()).map_err(|error| {
                crate::RestoreError::Corrupt(format!("cubikon checkpoint: {error}"))
            })?;
        self.engine.restore_checkpoint(&checkpoint.engine, now_ms)?;
        self.safety.restore_checkpoint(&checkpoint.safety, now_ms)?;
        self.traveler
            .restore_checkpoint(&checkpoint.traveler, now_ms)?;
        self.travel_active = checkpoint.travel_active;
        self.last_command = crate::rebase_age(now_ms, checkpoint.last_command_age_ms);
        self.patrol_last_visit = checkpoint
            .patrol_last_visit_ages
            .map(|age| crate::rebase_age(now_ms, age));
        self.patrol_target = checkpoint.patrol_target;
        Ok(())
    }

    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
            return ModuleOutput::running("Waiting for the game bridge");
        };
        let Some(target_map) = self.target_map(snapshot) else {
            return ModuleOutput::running("Waiting for faction or map catalog data");
        };
        let safety = self.safety.tick(
            snapshot,
            context.now_ms,
            &self.safety_config,
            self.engine.config().coordinates,
        );
        if let Some(error) = safety.failure {
            return ModuleOutput::failed(error);
        }

        let current_map = snapshot
            .navigation
            .map_id
            .or_else(|| snapshot.map.as_ref().and_then(|map| map.id));
        if current_map != Some(target_map) && safety.context.safe_to_work {
            let travel = self.traveler.tick(snapshot, target_map, context.now_ms);
            self.travel_active = travel.state == TravelState::Traveling;
            match travel.state {
                TravelState::Failed(error) => return ModuleOutput::failed(error),
                TravelState::Traveling => {
                    let mut plan =
                        hold(snapshot, travel.status, travel.command, travel.destination);
                    if travel.disable_pet {
                        plan.controls.pet_enabled = Some(false);
                    }
                    plan.controls.commands.extend(safety.commands);
                    self.note_command(&plan, context.now_ms);
                    return ModuleOutput {
                        state: RunState::Running,
                        status: plan
                            .note
                            .clone()
                            .unwrap_or_else(|| "Traveling to the farming map".into()),
                        plan: Some(plan),
                    };
                }
                TravelState::Arrived => self.travel_active = false,
            }
        }

        let navigation =
            ObstacleNavigation::from_snapshot(snapshot, self.engine.config().coordinates);
        let mut plan =
            self.engine
                .step_with_context(snapshot, context.now_ms, &safety.context, &navigation);
        if plan.selected_npc.is_some() {
            self.patrol_target = None;
        } else if plan.selected_box.is_none()
            && !plan.suspended
            && safety.context.safe_to_work
            && !plan
                .actions
                .iter()
                .any(|action| matches!(action, ActionIntent::WaitForCollection { .. }))
        {
            if let Some((destination, index)) = self.patrol(snapshot, context.now_ms) {
                plan.actions
                    .retain(|action| !matches!(action, ActionIntent::MoveTo { .. }));
                plan.actions.push(ActionIntent::MoveTo { destination });
                plan.note = Some(format!(
                    "Patrolling Cubikon position {}/{}",
                    index + 1,
                    PATROL_POINTS.len()
                ));
                plan.controls.ship_mode = Some(ShipMode::Roam);
            }
        }
        plan.controls.commands.extend(safety.commands);
        self.note_command(&plan, context.now_ms);
        let status = plan
            .note
            .clone()
            .unwrap_or_else(|| "Hunting Cubikon".into());
        ModuleOutput {
            state: RunState::Running,
            status,
            plan: Some(plan),
        }
    }

    fn update_settings(&mut self, settings: &Value) -> Result<(), String> {
        self.safety_config = safety_config(settings)?;
        self.engine
            .replace_config(behavior_config(settings)?)
            .map_err(|error| error.to_string())?;
        self.settings = settings.clone();
        self.traveler.reset();
        self.travel_active = false;
        self.patrol_target = None;
        self.patrol_last_visit = [None; 4];
        Ok(())
    }

    fn command_succeeded(&mut self, command: &GameCommand, now_ms: u64) {
        if matches!(
            command,
            GameCommand::JumpPortal { .. } | GameCommand::UseItem { .. }
        ) {
            self.last_command = Some(now_ms);
        }
    }

    fn plan_failed(&mut self, plan: &BehaviorDecision) {
        if let Some(target) = &plan.selected_box {
            self.engine.collection_failed(target);
        }
    }
}

impl CubikonModule {
    fn note_command(&mut self, plan: &BehaviorDecision, now_ms: u64) {
        if plan.controls.commands.iter().any(|command| {
            matches!(
                command,
                GameCommand::JumpPortal { .. } | GameCommand::UseItem { .. }
            )
        }) {
            self.last_command = Some(now_ms);
        }
    }
}

fn behavior_config(settings: &Value) -> Result<BehaviorConfig, String> {
    let radius = settings
        .get("npc_radius")
        .and_then(Value::as_f64)
        .unwrap_or(550.0);
    let mut npcs = vec![NpcRule {
        selector: EntitySelector {
            category: Some(EntityCategory::Npc),
            name: Some(CUBIKON_NAME.into()),
            ..EntitySelector::default()
        },
        priority: -100,
        combat_radius: radius,
        flags: vec![NpcFlag::IgnoreOwnership, NpcFlag::IgnoreAttacked],
        ..NpcRule::default()
    }];
    if settings
        .get("kill_kristallins_while_waiting")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        for name in KRISTALLIN_NAMES {
            npcs.push(NpcRule {
                selector: EntitySelector {
                    category: Some(EntityCategory::Npc),
                    name: Some((*name).into()),
                    fields: BTreeMap::from([("attackingHero".into(), json!(true))]),
                    ..EntitySelector::default()
                },
                priority: 0,
                combat_radius: radius,
                flags: vec![NpcFlag::IgnoreOwnership, NpcFlag::IgnoreAttacked],
                ..NpcRule::default()
            });
        }
    }
    let config = BehaviorConfig {
        mode: BehaviorMode::KillAndCollect,
        player: Some(EntitySelector {
            category: Some(EntityCategory::Player),
            ..EntitySelector::default()
        }),
        pet_enabled: settings
            .get("features")
            .and_then(|features| features.get("pet_active"))
            .and_then(Value::as_bool)
            .unwrap_or(true),
        npc_killer: crate::NpcKillerConfig {
            npcs,
            ..crate::NpcKillerConfig::default()
        },
        collector: crate::CollectorConfig {
            boxes: vec![BoxRule {
                selector: EntitySelector {
                    fields: BTreeMap::from([("entityKind".into(), json!("box"))]),
                    ..EntitySelector::default()
                },
                wait_ms: 1_500,
                ..BoxRule::default()
            }],
            ..crate::CollectorConfig::default()
        },
        ..BehaviorConfig::default()
    };
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

fn safety_config(settings: &Value) -> Result<SafetyConfig, String> {
    let config: SafetyConfig =
        serde_json::from_value(settings.get("safety").cloned().unwrap_or_else(|| json!({})))
            .map_err(|error| format!("Invalid safety settings: {error}"))?;
    config.validate()?;
    Ok(config)
}

fn map_value(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .or_else(|| value.as_str()?.trim().parse::<i32>().ok())
        .or_else(|| {
            let name = value.as_str()?.trim();
            xenomit_game_api::star_map::MAPS
                .iter()
                .find(|(_, map_name, _)| map_name.eq_ignore_ascii_case(name))
                .map(|(id, _, _)| *id)
        })
}

fn transform_point(point: Point, transform: crate::CoordinateTransform) -> Point {
    let (x, y) = transform.apply(point.x, point.y);
    Point { x, y }
}

fn distance(left: Point, right: Point) -> f64 {
    (left.x - right.x).hypot(left.y - right.y)
}

fn hold(
    snapshot: &GameSnapshot,
    status: impl Into<String>,
    command: Option<GameCommand>,
    destination: Option<Point>,
) -> BehaviorDecision {
    let player = snapshot
        .entities
        .iter()
        .find(|entity| entity.category == EntityCategory::Player)
        .map(|entity| EntityRef {
            path: entity.path.clone(),
            id: entity.id.clone(),
            name: entity.name.clone(),
            class_name: entity.class_name.clone(),
            position: Point {
                x: entity.position.x,
                y: entity.position.y,
            },
        });
    BehaviorDecision {
        mode: BehaviorMode::KillAndCollect,
        player,
        selected_npc: None,
        selected_box: None,
        actions: destination
            .map(|destination| vec![ActionIntent::MoveTo { destination }])
            .unwrap_or_default(),
        note: Some(status.into()),
        controls: ControlRequests {
            commands: command.into_iter().collect(),
            ship_mode: Some(ShipMode::Run),
            ..Default::default()
        },
        suspended: true,
        can_refresh: false,
    }
}

/// Retained cubikon-hunter state. Patrol timestamps and command cooldowns are
/// stored as ages relative to the export tick.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct CubikonCheckpoint {
    engine: crate::EngineCheckpoint,
    safety: crate::SafetyCheckpoint,
    traveler: crate::TravelerCheckpoint,
    travel_active: bool,
    last_command_age_ms: Option<u64>,
    patrol_last_visit_ages: [Option<u64>; 4],
    patrol_target: Option<usize>,
}
