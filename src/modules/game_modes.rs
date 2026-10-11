//! Behavior ports of specialized game modes (Galaxy, GoP, LoW, Hades,
//! Eternal Blacklight, Follow, Chaos Spawner, Skylab, Blacklight farmer,
//! Treacherous Domain). Combat and collection reuse the existing planner.
//! Gate spinning, placement, lives and group invites are typed commands the
//! native client executes through the game's request classes.
use crate::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use xenomit_game_api::{
    EntityCategory, GameCommand, GameEntity, GameSnapshot, SettingDescriptor, SettingKind,
};

const GATES: &[Gate] = &[
    Gate {
        id: 1,
        name: "Alpha",
        maps: &[51],
    },
    Gate {
        id: 2,
        name: "Beta",
        maps: &[52],
    },
    Gate {
        id: 3,
        name: "Gamma",
        maps: &[53],
    },
    Gate {
        id: 4,
        name: "Delta",
        maps: &[55],
    },
    Gate {
        id: 5,
        name: "Epsilon",
        maps: &[70],
    },
    Gate {
        id: 6,
        name: "Zeta",
        maps: &[71, 72, 73],
    },
    Gate {
        id: 7,
        name: "Kappa",
        maps: &[74],
    },
    Gate {
        id: 8,
        name: "Lambda",
        maps: &[75],
    },
    Gate {
        id: 12,
        name: "Kronos",
        maps: &[76],
    },
    Gate {
        id: 19,
        name: "Kuiper",
        maps: &[300, 301, 302, 303, 304],
    },
];
const CHAOS_CPUS: &[&str] = &[
    "ammunition_specialammo_alien-cpu-legend",
    "ammunition_specialammo_alien-cpu-rare",
    "ammunition_specialammo_alien-cpu-common",
];
const ETERNAL_CPU: &str = "ammunition_ggportal_eternal-blacklight-cpu";

struct Gate {
    id: i32,
    name: &'static str,
    maps: &'static [i32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Galaxy,
    Gop,
    Low,
    Hades,
    Eternal,
    Follow,
    Chaos,
    Skylab,
    Blacklight,
    Treacherous,
}

impl Kind {
    fn id(self) -> &'static str {
        match self {
            Self::Galaxy => "galaxy",
            Self::Gop => "gop",
            Self::Low => "low",
            Self::Hades => "hades",
            Self::Eternal => "eternal_blacklight",
            Self::Follow => "follow",
            Self::Chaos => "chaos_spawner",
            Self::Skylab => "skylab",
            Self::Blacklight => "blacklight_farmer",
            Self::Treacherous => "treacherous_domain",
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::Galaxy => "Galaxy gates",
            Self::Gop => "GoP",
            Self::Low => "LoW",
            Self::Hades => "Hades",
            Self::Eternal => "Eternal Blacklight",
            Self::Follow => "Follow",
            Self::Chaos => "Chaos spawner",
            Self::Skylab => "Skylab builder",
            Self::Blacklight => "Blacklight farmer",
            Self::Treacherous => "Treacherous Domain",
        }
    }
}

enum Step {
    Wait(String),
    Travel(i32),
    Combat,
    Hold {
        status: String,
        command: Option<GameCommand>,
        destination: Option<Point>,
    },
}

pub fn register(builder: &mut ModuleRegistryBuilder) {
    for kind in [
        Kind::Galaxy,
        Kind::Gop,
        Kind::Low,
        Kind::Hades,
        Kind::Eternal,
        Kind::Follow,
        Kind::Chaos,
        Kind::Skylab,
        Kind::Blacklight,
        Kind::Treacherous,
    ] {
        builder
            .register(descriptor(kind), move |settings| {
                Ok(Box::new(GameModeModule::new(kind, settings)?))
            })
            .expect("game mode descriptor is valid");
    }
}

fn descriptor(kind: Kind) -> ModuleDescriptor {
    let mut settings = vec![
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
    match kind {
        Kind::Galaxy => settings.extend([
            SettingDescriptor::new("gates", "Gates to run", SettingKind::Json, json!({"1": true, "2": true, "3": true}))
                .group("Galaxy")
                .description("Object of gate id to enabled. Ids: 1 Alpha, 2 Beta, 3 Gamma, 4 Delta, 5 Epsilon, 6 Zeta, 7 Kappa, 8 Lambda, 12 Kronos, 19 Kuiper."),
            SettingDescriptor::new("spin_amount", "Parts per spin", SettingKind::Integer { min: 1, max: 100 }, json!(1)).group("Galaxy"),
            SettingDescriptor::new("multiplier_at", "Use multiplier at", SettingKind::Integer { min: 0, max: 6 }, json!(5)).group("Galaxy"),
            SettingDescriptor::new("buy_life", "Buy a life when the gate has none", SettingKind::Boolean, json!(false)).group("Galaxy"),
            SettingDescriptor::new("jump_out_wave", "Jump out at wave (0 = never)", SettingKind::Integer { min: 0, max: 100 }, json!(0)).group("Galaxy"),
        ]),
        Kind::Gop => settings.extend([
            SettingDescriptor::new("easy", "Run GoP Easy", SettingKind::Boolean, json!(false)).group("GoP"),
            SettingDescriptor::new("attack_while_repair", "Attack while repairing or on a beacon", SettingKind::Boolean, json!(false)).group("GoP"),
        ]),
        Kind::Low => settings.push(
            SettingDescriptor::new("wait_for_members", "Wait until the group is on LoW", SettingKind::Boolean, json!(true)).group("LoW"),
        ),
        Kind::Hades => settings.push(
            SettingDescriptor::new("group_size", "Wait until the group has this many members", SettingKind::Integer { min: 1, max: 8 }, json!(4)).group("Hades"),
        ),
        Kind::Eternal => settings.push(
            SettingDescriptor::new("suicide_wave", "Stop at wave (0 = never)", SettingKind::Integer { min: 0, max: 999 }, json!(0)).group("Eternal"),
        ),
        Kind::Follow => settings.extend([
            SettingDescriptor::new("follow_id", "Player id to follow (0 = group leader)", SettingKind::Integer { min: 0, max: i64::from(i32::MAX) }, json!(0)).group("Follow"),
            SettingDescriptor::new("follow_name", "Player name contains", SettingKind::Text, json!("")).group("Follow"),
            SettingDescriptor::new("working_map", "Working map", SettingKind::Map, Value::Null).group("Navigation"),
        ]),
        Kind::Chaos => settings.push(
            SettingDescriptor::new("use_cpus", "Use equipped alien CPUs", SettingKind::Boolean, json!(true)).group("Chaos"),
        ),
        Kind::Skylab => settings.extend([
            SettingDescriptor::new("delay_minutes", "Minutes between checks", SettingKind::Integer { min: 1, max: 180 }, json!(5)).group("Skylab"),
            SettingDescriptor::new("buy_uri_robots", "Buy Uridium robots", SettingKind::Boolean, json!(false)).group("Skylab"),
            SettingDescriptor::new("disable_seprom", "Toggle the Seprom refinery with its resources", SettingKind::Boolean, json!(false)).group("Skylab"),
            SettingDescriptor::new("modules", "Modules to upgrade", SettingKind::Json, json!({
                "prometiumCollector": true, "enduriumCollector": true, "terbiumCollector": true
            })).group("Skylab"),
        ]),
        Kind::Blacklight => settings.push(
            SettingDescriptor::new("working_map", "Working map", SettingKind::Map, Value::Null).group("Navigation"),
        ),
        Kind::Treacherous => {}
    }
    if !matches!(kind, Kind::Skylab) {
        settings.extend([
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
        ]);
    }
    let mut required = vec![];
    if matches!(kind, Kind::Skylab) {
        required.push("backpage.skylab".into());
    }
    ModuleDescriptor {
        id: kind.id().into(),
        name: kind.name().into(),
        description: match kind {
            Kind::Galaxy => "Spin, place and clear the selected galaxy gates.",
            Kind::Gop => "Travel to GoP and clear waves, including Plutus beacons.",
            Kind::Low => "Clear the LoW gate and cover the Century Falcon.",
            Kind::Hades => "Travel to Hades, wait for the group, and clear the gate.",
            Kind::Eternal => "Open the eternal blacklight gate and clear waves.",
            Kind::Follow => "Follow a player or the group leader and fight nearby NPCs.",
            Kind::Chaos => "Use alien CPUs and fight the Chaos NPCs they spawn.",
            Kind::Skylab => "Upgrade checked skylab modules and restock collector robots.",
            Kind::Blacklight => "Kill and collect on the selected blacklight map.",
            Kind::Treacherous => "Enter the Treacherous Domain from the faction 1 or 8 map.",
        }
        .into(),
        settings,
        required_services: required,
    }
}

struct GameModeModule {
    kind: Kind,
    settings: Value,
    engine: BehaviorEngine,
    safety_config: SafetyConfig,
    safety: SafetyManager,
    traveler: MapTraveler,
    traveling_to: Option<i32>,
    last_command: Option<u64>,
    skylab_handle: Option<OperationHandle>,
    skylab_next_ms: u64,
    /// True while the last observed tick placed the hero inside a gate map.
    /// Tracked across ticks so `non_interruptible` still reports a gate when
    /// the snapshot is briefly unavailable.
    in_gate: bool,
}

impl GameModeModule {
    fn new(kind: Kind, settings: &Value) -> Result<Self, String> {
        let safety_config = safety_config(settings)?;
        Ok(Self {
            kind,
            settings: settings.clone(),
            engine: BehaviorEngine::new(behavior_config(kind, settings)?)
                .map_err(|e| e.to_string())?,
            safety_config,
            safety: SafetyManager::default(),
            traveler: MapTraveler::default(),
            traveling_to: None,
            last_command: None,
            skylab_handle: None,
            skylab_next_ms: 0,
            in_gate: false,
        })
    }
}

impl BehaviorModule for GameModeModule {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        if self.kind == Kind::Skylab && self.skylab_handle.is_some() {
            return InterruptionStatus::Deferred("Waiting for the current skylab operation".into());
        }
        if self.traveling_to.is_some() {
            return InterruptionStatus::Deferred("Waiting for map travel to finish".into());
        }
        if self
            .last_command
            .is_some_and(|at| context.now_ms.saturating_sub(at) < 2_500)
        {
            return InterruptionStatus::Deferred(
                "Waiting for the last game command to settle".into(),
            );
        }
        if self.kind != Kind::Skylab {
            if let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) {
                if crate::interruption::is_gate_map(snapshot) {
                    return InterruptionStatus::Deferred(
                        "Waiting until the current gate is exited".into(),
                    );
                }
            }
        }
        crate::interruption::safe_idle_status(context)
    }
    /// Account operations (skylab requests, purchases) and active gate
    /// sessions cannot resume after a kill: the countdown pauses on them.
    fn non_interruptible(&self) -> bool {
        self.skylab_handle.is_some() || self.in_gate
    }
    fn export_state(&self, now_ms: u64, _best_effort: bool) -> Option<crate::ModuleState> {
        let mut issues = Vec::new();
        if self.skylab_handle.is_some() {
            issues.push("in-flight skylab operation discarded; the step re-runs".into());
        }
        let checkpoint = GameModeCheckpoint {
            engine: self.engine.export_checkpoint(now_ms),
            safety: self.safety.export_checkpoint(now_ms),
            traveler: self.traveler.export_checkpoint(now_ms),
            traveling_to: self.traveling_to,
            last_command_age_ms: crate::age_ms(self.last_command, now_ms),
            skylab_next_in_ms: crate::remaining_ms(Some(self.skylab_next_ms), now_ms),
        };
        let data = serde_json::to_value(&checkpoint).ok()?;
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
        let checkpoint: GameModeCheckpoint =
            serde_json::from_value(state.data.clone()).map_err(|error| {
                crate::RestoreError::Corrupt(format!("game mode checkpoint: {error}"))
            })?;
        self.engine.restore_checkpoint(&checkpoint.engine, now_ms)?;
        self.safety.restore_checkpoint(&checkpoint.safety, now_ms)?;
        self.traveler
            .restore_checkpoint(&checkpoint.traveler, now_ms)?;
        self.traveling_to = checkpoint.traveling_to;
        self.last_command = crate::rebase_age(now_ms, checkpoint.last_command_age_ms);
        self.skylab_next_ms =
            crate::rebase_deadline(now_ms, checkpoint.skylab_next_in_ms).unwrap_or(0);
        self.skylab_handle = None;
        self.in_gate = false;
        Ok(())
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        if self.kind == Kind::Skylab {
            return self.tick_skylab(context);
        }
        let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
            return ModuleOutput::running("Waiting for the game bridge");
        };
        self.in_gate = crate::interruption::is_gate_map(snapshot);
        let step = decide(self.kind, snapshot, &self.settings);
        let mut plan = match step {
            Step::Wait(status) => hold(snapshot, status, None, None),
            Step::Hold {
                status,
                command,
                destination,
            } => {
                if command.is_some()
                    && self
                        .last_command
                        .is_some_and(|at| context.now_ms.saturating_sub(at) < 2_500)
                {
                    hold(snapshot, status, None, destination)
                } else {
                    hold(snapshot, status, command, destination)
                }
            }
            Step::Travel(_) | Step::Combat => {
                let target = match step {
                    Step::Travel(map) => Some(map),
                    _ => snapshot.navigation.map_id,
                };
                if self.engine.config().working_map != target
                    && let Err(error) = self.engine.replace_config({
                        let mut config = self.engine.config().clone();
                        config.working_map = target;
                        config
                    })
                {
                    return ModuleOutput::failed(error.to_string());
                }
                match self.combat_plan(snapshot, context.now_ms) {
                    Ok(plan) => plan,
                    Err(error) => return ModuleOutput::failed(error),
                }
            }
        };
        if let Some(command) = &plan.controls.commands.first()
            && matches!(
                command,
                GameCommand::SpinGalaxyGate { .. }
                    | GameCommand::PlaceGalaxyGate { .. }
                    | GameCommand::BuyGalaxyLife { .. }
                    | GameCommand::UseItem { .. }
                    | GameCommand::AcceptGroupInvite { .. }
            )
        {
            self.last_command = Some(context.now_ms);
        }
        self.push_support_commands(snapshot, &mut plan);
        let status = plan.note.clone().unwrap_or_else(|| self.kind.name().into());
        ModuleOutput {
            state: RunState::Running,
            status,
            plan: Some(plan),
        }
    }
    fn update_settings(&mut self, settings: &Value) -> Result<(), String> {
        self.safety_config = safety_config(settings)?;
        self.engine
            .replace_config(behavior_config(self.kind, settings)?)
            .map_err(|e| e.to_string())?;
        self.settings = settings.clone();
        self.traveler.reset();
        self.traveling_to = None;
        self.in_gate = false;
        Ok(())
    }
    fn command_succeeded(&mut self, command: &GameCommand, now_ms: u64) {
        if matches!(
            command,
            GameCommand::SpinGalaxyGate { .. }
                | GameCommand::PlaceGalaxyGate { .. }
                | GameCommand::BuyGalaxyLife { .. }
                | GameCommand::UseItem { .. }
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

impl GameModeModule {
    fn combat_plan(
        &mut self,
        snapshot: &GameSnapshot,
        now_ms: u64,
    ) -> Result<BehaviorDecision, String> {
        let safety = self.safety.tick(
            snapshot,
            now_ms,
            &self.safety_config,
            self.engine.config().coordinates,
        );
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
                let result = self.traveler.tick(snapshot, target, now_ms);
                match result.state {
                    TravelState::Failed(error) => return Err(error),
                    TravelState::Arrived => self.traveling_to = None,
                    TravelState::Traveling => travel = Some(result),
                }
            }
        }
        let mut plan = if let Some(result) = &travel {
            hold(
                snapshot,
                result.status.clone(),
                result.command.clone(),
                result.destination,
            )
        } else {
            let observed = snapshot
                .entities
                .iter()
                .find(|entity| entity.category == EntityCategory::Player)
                .map(|hero| {
                    ModuleContext::from_snapshot(snapshot, hero, self.engine.config().coordinates)
                })
                .unwrap_or_else(|| safety.context.clone());
            self.engine
                .step_with_context(snapshot, now_ms, &observed, &navigation)
                .into_world_coordinates(self.engine.config().coordinates)
        };
        if let Some(target) = plan.controls.travel_to_map {
            self.traveling_to = Some(target);
            let travel = travel.unwrap_or_else(|| self.traveler.tick(snapshot, target, now_ms));
            plan.actions = travel
                .destination
                .map(|destination| vec![ActionIntent::MoveTo { destination }])
                .unwrap_or_default();
            plan.controls.commands.extend(travel.command);
            plan.note = Some(travel.status);
            plan.suspended = true;
            if let TravelState::Failed(error) = travel.state {
                return Err(error);
            }
        }
        if !snapshot.navigation.hero.killed {
            plan.controls.commands.extend(safety.commands);
        }
        Ok(plan)
    }

    fn push_support_commands(&self, snapshot: &GameSnapshot, plan: &mut BehaviorDecision) {
        let hero = &snapshot.navigation.hero;
        let config = if plan.selected_npc.is_some() {
            number(&self.settings, "offensive_config", 1.0)
        } else {
            number(&self.settings, "repair_config", 2.0)
        } as u8;
        if (1..=2).contains(&config) && hero.configuration.is_some_and(|current| current != config)
        {
            plan.controls
                .commands
                .push(GameCommand::SelectConfiguration {
                    configuration: config,
                });
        }
        if bool_setting(&self.settings, "features.pet_active", true)
            && hero.owns_pet
            && !hero.pet_destroyed
            && !hero.pet_active
        {
            plan.controls
                .commands
                .push(GameCommand::SetPet { enabled: true });
        }
        if self.kind == Kind::Low
            && plan
                .selected_npc
                .as_ref()
                .is_some_and(|npc| npc.name.contains("Century Falcon"))
        {
            let hp = hero_health_ratio(snapshot);
            if hp.is_some_and(|hp| hp < 0.35) {
                for item in [
                    "ability_orcus_assimilate",
                    "equipment_extra_cpu_ish-01",
                    "ammunition_specialammo_emp-01",
                ] {
                    if item_ready(hero, item) {
                        plan.controls.commands.push(GameCommand::UseItem {
                            item_id: item.into(),
                            activate: true,
                        });
                        break;
                    }
                }
            }
        }
        if self.kind == Kind::Chaos && bool_setting(&self.settings, "use_cpus", true) {
            if let Some(item) = CHAOS_CPUS.iter().find(|item| item_ready(hero, item)) {
                plan.controls.commands.push(GameCommand::UseItem {
                    item_id: (*item).into(),
                    activate: true,
                });
            }
        }
    }

    fn tick_skylab(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        if let Some(handle) = self.skylab_handle {
            return match context.operations.poll(handle) {
                OperationPoll::Pending => ModuleOutput::running("Checking the skylab"),
                OperationPoll::Succeeded(value) => {
                    context.operations.release(handle);
                    self.skylab_handle = None;
                    let minutes = number(&self.settings, "delay_minutes", 5.0).max(1.0) as u64;
                    self.skylab_next_ms = context.now_ms.saturating_add(minutes * 60_000);
                    ModuleOutput::running(
                        value
                            .get("status")
                            .and_then(Value::as_str)
                            .unwrap_or("Skylab check finished"),
                    )
                }
                OperationPoll::Failed(error) => {
                    context.operations.release(handle);
                    self.skylab_handle = None;
                    ModuleOutput::failed(error)
                }
            };
        }
        if context.now_ms < self.skylab_next_ms {
            return ModuleOutput::running("Waiting for the next skylab check");
        }
        match context.operations.start(OperationRequest::service(
            "backpage.skylab",
            json!({
                "modules": self.settings.get("modules").cloned().unwrap_or_else(|| json!({})),
                "buy_uri_robots": bool_setting(&self.settings, "buy_uri_robots", false),
                "disable_seprom": bool_setting(&self.settings, "disable_seprom", false),
            }),
        )) {
            Ok(handle) => {
                self.skylab_handle = Some(handle);
                ModuleOutput::running("Checking the skylab")
            }
            Err(error) => ModuleOutput::failed(error),
        }
    }
}

fn decide(kind: Kind, snapshot: &GameSnapshot, settings: &Value) -> Step {
    let map = snapshot
        .navigation
        .map_id
        .or_else(|| snapshot.map.as_ref().and_then(|map| map.id))
        .filter(|map| *map != -1);
    match kind {
        Kind::Galaxy => galaxy_step(snapshot, settings, map),
        Kind::Gop => named_gate_step(
            snapshot,
            if bool_setting(settings, "easy", false) {
                "GoP Easy 1"
            } else {
                "GoP Normal 1"
            },
            "Final",
        ),
        Kind::Low => fixed_gate_step(
            snapshot,
            200,
            bool_setting(settings, "wait_for_members", true),
        ),
        Kind::Hades => hades_step(snapshot, settings, map),
        Kind::Eternal => eternal_step(snapshot, settings, map),
        Kind::Follow => follow_step(snapshot, settings, map),
        Kind::Chaos => Step::Combat,
        Kind::Blacklight => map_setting_step(snapshot, settings),
        Kind::Treacherous => treacherous_step(snapshot, map),
        Kind::Skylab => Step::Wait("Skylab runs off the game map".into()),
    }
}

fn galaxy_step(snapshot: &GameSnapshot, settings: &Value, map: Option<i32>) -> Step {
    let enabled = enabled_gates(settings);
    if let Some(map) = map
        && let Some(gate) = enabled.iter().find(|gate| gate.maps.contains(&map))
    {
        let wave = snapshot
            .navigation
            .galaxy_gates
            .iter()
            .find(|info| info.id == gate.id)
            .map(|info| info.current_wave)
            .unwrap_or(0);
        let jump_out = number(settings, "jump_out_wave", 0.0) as i32;
        if jump_out > 0 && wave >= jump_out {
            return jump_portal(snapshot, "Jumping out of the gate", |portal| {
                portal
                    .target_map
                    .is_some_and(|target| !gate.maps.contains(&target))
            });
        }
        return Step::Combat;
    }
    let Some(home) = home_map(snapshot, "-1") else {
        return Step::Wait("Waiting for the faction home map".into());
    };
    if map != Some(home) {
        return Step::Travel(home);
    }
    let Some(gate) = enabled.first() else {
        return Step::Wait("No galaxy gate is enabled".into());
    };
    let Some(info) = snapshot
        .navigation
        .galaxy_gates
        .iter()
        .find(|info| info.id == gate.id)
    else {
        return Step::Wait("Waiting for the galaxy gate builder".into());
    };
    if info.lives == 0 && bool_setting(settings, "buy_life", false) {
        return command(
            format!("Buying a life for {}", gate.name),
            GameCommand::BuyGalaxyLife { gate_id: gate.id },
        );
    }
    if info.total_parts > 0 && info.parts < info.total_parts {
        let amount = match number(settings, "spin_amount", 1.0) as u8 {
            5 => 5,
            10 => 10,
            100 => 100,
            _ => 1,
        };
        let use_multiplier =
            info.multiplier >= number(settings, "multiplier_at", 5.0) as i32 && info.multiplier > 0;
        return command(
            format!(
                "Spinning {} ({}/{})",
                gate.name, info.parts, info.total_parts
            ),
            GameCommand::SpinGalaxyGate {
                gate_id: gate.id,
                spin_amount: amount,
                use_multiplier,
            },
        );
    }
    if info.total_parts > 0 && info.parts >= info.total_parts && !info.finished {
        return command(
            format!("Placing {}", gate.name),
            GameCommand::PlaceGalaxyGate { gate_id: gate.id },
        );
    }
    jump_portal(snapshot, &format!("Entering {}", gate.name), |portal| {
        portal
            .target_map
            .is_some_and(|target| gate.maps.contains(&target))
    })
}

fn named_gate_step(snapshot: &GameSnapshot, first: &str, inside_marker: &str) -> Step {
    let name = snapshot
        .map
        .as_ref()
        .and_then(|map| map.name.as_deref())
        .unwrap_or("");
    if name.contains(first) || name.contains("GoP") {
        if let Some(beacon) = snapshot
            .entities
            .iter()
            .find(|entity| entity.class_name.contains("Plutus") || entity.name.contains("Plutus"))
        {
            return Step::Hold {
                status: "Holding the Plutus beacon".into(),
                command: None,
                destination: Some(Point {
                    x: beacon.position.x,
                    y: beacon.position.y,
                }),
            };
        }
        if snapshot.navigation.portals.len() > 1 && !name.contains(inside_marker) {
            return jump_highest_portal(snapshot);
        }
        return Step::Combat;
    }
    match snapshot
        .navigation
        .star_system
        .maps
        .iter()
        .find(|map| map.name == first)
        .map(|map| map.id)
    {
        Some(id) => Step::Travel(id),
        None => Step::Wait(format!("Waiting for the {first} map catalog")),
    }
}

fn fixed_gate_step(snapshot: &GameSnapshot, map_id: i32, wait_members: bool) -> Step {
    if snapshot.navigation.map_id != Some(map_id) {
        return Step::Travel(map_id);
    }
    if wait_members
        && !snapshot.navigation.group.members.is_empty()
        && snapshot
            .navigation
            .group
            .members
            .iter()
            .any(|member| member.map_id != map_id)
    {
        return Step::Wait("Waiting for the group".into());
    }
    Step::Combat
}

fn hades_step(snapshot: &GameSnapshot, settings: &Value, map: Option<i32>) -> Step {
    if map != Some(203) {
        return if snapshot
            .navigation
            .portals
            .iter()
            .any(|portal| portal.target_map == Some(203))
        {
            jump_portal(snapshot, "Entering Hades", |portal| {
                portal.target_map == Some(203)
            })
        } else {
            Step::Travel(203)
        };
    }
    let wanted = number(settings, "group_size", 4.0) as usize;
    if snapshot.navigation.portals.len() <= 1
        && snapshot.navigation.group.members.len() + 1 < wanted
    {
        return Step::Hold {
            status: "Waiting for the Hades group".into(),
            command: None,
            destination: Some(Point {
                x: 5300.0,
                y: 3900.0,
            }),
        };
    }
    if snapshot.navigation.portals.len() > 1 {
        return jump_highest_portal(snapshot);
    }
    Step::Combat
}

fn eternal_step(snapshot: &GameSnapshot, settings: &Value, map: Option<i32>) -> Step {
    let name = snapshot
        .map
        .as_ref()
        .and_then(|map| map.name.as_deref())
        .unwrap_or("");
    if matches!(name, "1BL" | "2BL" | "3BL") {
        if snapshot
            .navigation
            .portals
            .iter()
            .any(|portal| portal.target_map == Some(-4))
        {
            return jump_portal(snapshot, "Entering eternal blacklight", |portal| {
                portal.target_map == Some(-4)
            });
        }
        return command(
            "Using the eternal blacklight CPU".into(),
            GameCommand::UseItem {
                item_id: ETERNAL_CPU.into(),
                activate: true,
            },
        );
    }
    if map == Some(-4) || name.contains("Blacklight") || name.contains("Eternal") {
        let wave = snapshot
            .navigation
            .galaxy_gates
            .iter()
            .map(|gate| gate.current_wave)
            .max()
            .unwrap_or(0);
        let stop = number(settings, "suicide_wave", 0.0) as i32;
        if stop > 0 && wave >= stop {
            return Step::Wait(format!("Reached eternal wave {wave}"));
        }
        return Step::Combat;
    }
    match snapshot
        .navigation
        .star_system
        .maps
        .iter()
        .find(|map| matches!(map.name.as_str(), "1BL" | "2BL" | "3BL"))
        .map(|map| map.id)
    {
        Some(id) => Step::Travel(id),
        None => Step::Wait("Waiting for a blacklight map in the catalog".into()),
    }
}

fn follow_step(snapshot: &GameSnapshot, settings: &Value, map: Option<i32>) -> Step {
    if let Some(target) = settings
        .get("working_map")
        .and_then(|value| resolve_map_id(value, &snapshot.navigation.star_system))
        && map != Some(target)
    {
        return Step::Travel(target);
    }
    if let Some(ship) = follow_target(snapshot, settings) {
        return Step::Hold {
            status: format!("Following {}", ship.name),
            command: None,
            destination: Some(Point {
                x: ship.position.x,
                y: ship.position.y,
            }),
        };
    }
    Step::Combat
}

fn map_setting_step(snapshot: &GameSnapshot, settings: &Value) -> Step {
    let current_map = snapshot
        .navigation
        .map_id
        .or_else(|| snapshot.map.as_ref().and_then(|map| map.id))
        .filter(|map| *map != -1);
    match settings
        .get("working_map")
        .and_then(|value| resolve_map_id(value, &snapshot.navigation.star_system))
    {
        Some(target) if current_map != Some(target) => Step::Travel(target),
        _ => Step::Combat,
    }
}

fn treacherous_step(snapshot: &GameSnapshot, map: Option<i32>) -> Step {
    if map == Some(471) {
        return Step::Combat;
    }
    let on_approach = snapshot
        .map
        .as_ref()
        .and_then(|map| map.name.as_deref())
        .is_some_and(|name| {
            snapshot.navigation.hero.faction.is_some_and(|faction| {
                name == format!("{faction}-1") || name == format!("{faction}-8")
            })
        });
    if on_approach {
        return jump_portal(snapshot, "Entering the Treacherous Domain", |portal| {
            portal.type_id == 238 || portal.target_map == Some(471)
        });
    }
    match home_map(snapshot, "-8") {
        Some(id) => Step::Travel(id),
        None => Step::Wait("Waiting for the faction x-8 map".into()),
    }
}

fn follow_target<'a>(snapshot: &'a GameSnapshot, settings: &Value) -> Option<&'a GameEntity> {
    let id = number(settings, "follow_id", 0.0) as i64;
    let name = settings
        .get("follow_name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    snapshot
        .entities
        .iter()
        .find(|entity| {
            entity.category == EntityCategory::Ship
                && ((id > 0 && entity.id.as_ref().and_then(Value::as_i64) == Some(id))
                    || (!name.is_empty() && entity.name.to_ascii_lowercase().contains(&name)))
        })
        .or_else(|| {
            let leader = snapshot
                .navigation
                .group
                .members
                .iter()
                .find(|member| member.leader)?;
            snapshot.entities.iter().find(|entity| {
                entity.id.as_ref().and_then(Value::as_i64) == Some(i64::from(leader.id))
            })
        })
}

fn jump_portal(
    snapshot: &GameSnapshot,
    status: &str,
    predicate: impl Fn(&xenomit_game_api::PortalInfo) -> bool,
) -> Step {
    let Some(portal) = snapshot
        .navigation
        .portals
        .iter()
        .find(|portal| predicate(portal))
    else {
        return Step::Wait(format!("{status}: portal is not on the map"));
    };
    let hero = snapshot.map.as_ref().and_then(|map| map.player_position);
    let close = hero
        .is_some_and(|hero| (hero.x - portal.position.x).hypot(hero.y - portal.position.y) < 550.0);
    Step::Hold {
        status: status.into(),
        command: close.then_some(GameCommand::JumpPortal {
            portal_id: portal.id,
        }),
        destination: Some(Point {
            x: portal.position.x,
            y: portal.position.y,
        }),
    }
}

fn jump_highest_portal(snapshot: &GameSnapshot) -> Step {
    let Some(portal) = snapshot
        .navigation
        .portals
        .iter()
        .max_by_key(|portal| portal.type_id)
    else {
        return Step::Wait("Waiting for the next gate portal".into());
    };
    jump_portal(snapshot, "Jumping to the next wave", |candidate| {
        candidate.id == portal.id
    })
}

fn command(status: String, command: GameCommand) -> Step {
    Step::Hold {
        status,
        command: Some(command),
        destination: None,
    }
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
        mode: BehaviorMode::NpcKiller,
        player,
        selected_npc: None,
        selected_box: None,
        actions: destination
            .map(|destination| vec![ActionIntent::MoveTo { destination }])
            .unwrap_or_default(),
        note: Some(status.into()),
        controls: ControlRequests {
            commands: command.into_iter().collect(),
            ship_mode: Some(ShipMode::Roam),
            ..Default::default()
        },
        suspended: true,
        can_refresh: false,
    }
}

fn enabled_gates(settings: &Value) -> Vec<&'static Gate> {
    let selected = settings.get("gates").and_then(Value::as_object);
    GATES
        .iter()
        .filter(|gate| {
            selected
                .and_then(|values| {
                    values
                        .get(&gate.id.to_string())
                        .or_else(|| values.get(gate.name))
                })
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect()
}

fn home_map(snapshot: &GameSnapshot, suffix: &str) -> Option<i32> {
    let faction = snapshot.navigation.hero.faction?;
    let name = format!("{faction}{suffix}");
    snapshot
        .navigation
        .star_system
        .maps
        .iter()
        .find(|map| map.name == name)
        .map(|map| map.id)
}

fn hero_health_ratio(snapshot: &GameSnapshot) -> Option<f64> {
    let hero = snapshot
        .entities
        .iter()
        .find(|entity| entity.category == EntityCategory::Player)?;
    let max = hero.max_health.filter(|value| *value > 0.0)?;
    Some((hero.health.unwrap_or(max) / max).clamp(0.0, 1.0))
}

fn item_ready(hero: &xenomit_game_api::HeroState, id: &str) -> bool {
    hero.items
        .get(id)
        .is_some_and(|item| item.available && !item.blocked && item.quantity.unwrap_or(1.0) > 0.0)
}

fn behavior_config(kind: Kind, settings: &Value) -> Result<BehaviorConfig, String> {
    let mut config = BehaviorConfig {
        player: Some(EntitySelector {
            category: Some(EntityCategory::Player),
            ..EntitySelector::default()
        }),
        mode: if matches!(kind, Kind::Blacklight | Kind::Gop | Kind::Galaxy) {
            BehaviorMode::KillAndCollect
        } else {
            BehaviorMode::NpcKiller
        },
        pet_enabled: bool_setting(settings, "features.pet_active", true),
        ..BehaviorConfig::default()
    };
    config.npc_killer.npcs = match kind {
        Kind::Chaos => vec![NpcRule {
            selector: EntitySelector {
                category: Some(EntityCategory::Npc),
                name_contains: Some("Chaos".into()),
                ..EntitySelector::default()
            },
            ..NpcRule::default()
        }],
        Kind::Low => vec![
            NpcRule {
                selector: EntitySelector {
                    category: Some(EntityCategory::Npc),
                    name_contains: Some("Century Falcon".into()),
                    ..EntitySelector::default()
                },
                priority: -10,
                ..NpcRule::default()
            },
            NpcRule {
                selector: EntitySelector {
                    category: Some(EntityCategory::Npc),
                    ..EntitySelector::default()
                },
                ..NpcRule::default()
            },
        ],
        _ => vec![NpcRule {
            selector: EntitySelector {
                category: Some(EntityCategory::Npc),
                ..EntitySelector::default()
            },
            ..NpcRule::default()
        }],
    };
    for rule in &mut config.npc_killer.npcs {
        rule.combat_radius = number(settings, "npc_radius", rule.combat_radius);
    }
    if config.mode == BehaviorMode::KillAndCollect {
        config.collector.boxes.push(BoxRule {
            selector: EntitySelector {
                fields: BTreeMap::from([("entityKind".into(), json!("box"))]),
                ..EntitySelector::default()
            },
            wait_ms: 1_500,
            ..BoxRule::default()
        });
    }
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

fn bool_setting(settings: &Value, key: &str, fallback: bool) -> bool {
    key.split('.')
        .try_fold(settings, |value, part| value.get(part))
        .and_then(Value::as_bool)
        .unwrap_or(fallback)
}
fn number(settings: &Value, key: &str, fallback: f64) -> f64 {
    settings
        .get(key)
        .and_then(Value::as_f64)
        .unwrap_or(fallback)
}

/// Retained game-mode state; sub-checkpoints carry engine/safety/travel.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct GameModeCheckpoint {
    engine: crate::EngineCheckpoint,
    safety: crate::SafetyCheckpoint,
    traveler: crate::TravelerCheckpoint,
    traveling_to: Option<i32>,
    last_command_age_ms: Option<u64>,
    /// Time until the next skylab check; the in-flight handle never persists.
    skylab_next_in_ms: Option<u64>,
}
