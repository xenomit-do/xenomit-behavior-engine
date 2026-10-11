//! SpaceBall objective: observed ball, directed travel, cargo pickups and manual modes.
use crate::*;
use serde_json::json;
use xenomit_game_api::{EntityCategory, GameEntity, GameSnapshot};

pub(crate) fn register(builder: &mut ModuleRegistryBuilder) -> Result<(), String> {
    let descriptor = ModuleDescriptor::new("shared_spaceball", "SpaceBall")
        .description("Attack or follow SpaceBall on 4-4 and collect its cargo. Start and stop the event manually.")
        .setting(SettingDescriptor::new("mode", "Ball mode", SettingKind::Choice { options: ["attack", "follow"].into_iter().map(|mode| SettingChoice { label: mode.into(), value: json!(mode) }).collect() }, json!("attack")))
        .setting(SettingDescriptor::new("collect_cargo", "Collect cargo", SettingKind::Boolean, json!(true)))
        .setting(SettingDescriptor::new("target_delay_seconds", "Collect after losing the ball (seconds)", SettingKind::Integer { min: 1, max: 60 }, json!(20)))
        .setting(SettingDescriptor::new("combat_radius", "Ball distance", SettingKind::Number { min: 100.0, max: 1000.0 }, json!(500.0)))
        .setting(SettingDescriptor::new("tick_ms", "Tick interval (ms)", SettingKind::Integer { min: 10, max: 60000 }, json!(150)))
        .setting(SettingDescriptor::new("safety", "Safety settings", SettingKind::Json, serde_json::to_value(SafetyConfig { run_from_enemy: false, ..Default::default() }).unwrap()));
    builder.register(descriptor, |settings| {
        let safety_config: SafetyConfig = serde_json::from_value(settings["safety"].clone())
            .map_err(|e| format!("Invalid SpaceBall safety: {e}"))?;
        safety_config.validate()?;
        let mut config = BehaviorConfig {
            mode: BehaviorMode::KillAndCollect,
            ..Default::default()
        };
        config.npc_killer.only_kill_preferred = true;
        config.npc_killer.max_target_distance = 30000.0;
        config.npc_killer.npcs.push(NpcRule {
            selector: EntitySelector::name("SpaceBall"),
            combat_radius: settings["combat_radius"].as_f64().unwrap_or(500.0),
            flags: vec![
                NpcFlag::IgnoreOwnership,
                NpcFlag::IgnoreAttacked,
                NpcFlag::NoCircle,
            ],
            ..Default::default()
        });
        if settings["collect_cargo"].as_bool() == Some(true) {
            for name in ["FROM_SHIP", "CANDY_CARGO", "TURKEY_CARGO"] {
                config.collector.boxes.push(BoxRule {
                    selector: EntitySelector::name(name),
                    wait_ms: 100,
                    resource: true,
                    ..Default::default()
                });
            }
        }
        config.collector.max_distance = 30000.0;
        config.kill_and_collect.collect_during_combat = false;
        Ok(Box::new(Spaceball {
            planner: BehaviorEngine::new(config).map_err(|e| e.to_string())?,
            safety: SafetyManager::default(),
            safety_config,
            traveler: MapTraveler::default(),
            follow: settings["mode"] == "follow",
            target_delay_ms: settings["target_delay_seconds"].as_u64().unwrap_or(20) * 1000,
            previous_ball: false,
            collect_until: 0,
            current_map: None,
        }))
    })?;
    Ok(())
}
struct Spaceball {
    planner: BehaviorEngine,
    safety: SafetyManager,
    safety_config: SafetyConfig,
    traveler: MapTraveler,
    follow: bool,
    target_delay_ms: u64,
    previous_ball: bool,
    collect_until: u64,
    current_map: Option<i32>,
}
impl BehaviorModule for Spaceball {
    fn interruption_status(
        &self,
        context: &InterruptionContext<'_>,
        _reason: InterruptionReason,
    ) -> InterruptionStatus {
        crate::interruption::safe_idle_status(context)
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
            return ModuleOutput::running("Waiting for SpaceBall map");
        };
        if self.current_map != snapshot.navigation.map_id {
            self.current_map = snapshot.navigation.map_id;
            self.previous_ball = false;
            self.collect_until = 0;
        }
        let safety = self.safety.tick(
            snapshot,
            context.now_ms,
            &self.safety_config,
            CoordinateTransform::default(),
        );
        if let Some(error) = safety.failure {
            return ModuleOutput::failed(error);
        }
        let navigation =
            ObstacleNavigation::from_snapshot(snapshot, CoordinateTransform::default());
        if !safety.context.safe_to_work {
            let mut plan = self.planner.step_with_context(
                snapshot,
                context.now_ms,
                &safety.context,
                &navigation,
            );
            plan.controls.commands.extend(safety.commands);
            return planned(plan, "SpaceBall safety");
        }
        let on_map = snapshot.map.as_ref().and_then(|m| m.name.as_deref()) == Some("4-4");
        let target_map = snapshot
            .navigation
            .star_system
            .maps
            .iter()
            .find(|map| map.name == "4-4")
            .map(|map| map.id);
        if !on_map && (target_map.is_none() || target_map != snapshot.navigation.map_id) {
            let Some(target) = target_map else {
                return planned(wait(snapshot), "Waiting for 4-4 in the map catalog");
            };
            let travel = self.traveler.tick(snapshot, target, context.now_ms);
            if let TravelState::Failed(error) = travel.state {
                return ModuleOutput::failed(error);
            }
            if travel.state != TravelState::Arrived {
                let mut plan = wait(snapshot);
                plan.controls.ship_mode = Some(ShipMode::Run);
                if travel.disable_pet {
                    plan.controls.pet_enabled = Some(false);
                }
                if let Some(destination) = travel.destination {
                    plan.actions.push(ActionIntent::MoveTo { destination });
                }
                plan.controls.commands.extend(travel.command);
                return planned(plan, travel.status);
            }
        }
        // Match the upstream's exact names, not a broad NPC or cargo fallback.
        let ball = snapshot.entities.iter().find(|entity| {
            entity.category == EntityCategory::Npc && entity.name.eq_ignore_ascii_case("SpaceBall")
        });
        if self.previous_ball && ball.is_none() {
            self.collect_until = context.now_ms.saturating_add(self.target_delay_ms);
        }
        self.previous_ball = ball.is_some();
        let mut observed = snapshot.clone();
        observed.entities.retain(|entity| {
            entity.category != EntityCategory::Npc
                || (ball.is_some()
                    && context.now_ms >= self.collect_until
                    && !self.follow
                    && entity.name.eq_ignore_ascii_case("SpaceBall"))
        });
        let mut plan =
            self.planner
                .step_with_context(&observed, context.now_ms, &safety.context, &navigation);
        plan.controls.commands.extend(safety.commands);
        if let Some(ball) = ball.filter(|_| {
            self.follow && context.now_ms >= self.collect_until && plan.selected_box.is_none()
        }) {
            plan = wait(snapshot);
            let hero = snapshot
                .entities
                .iter()
                .find(|e| e.category == EntityCategory::Player);
            if hero.is_some_and(|hero| {
                (hero.position.x - ball.position.x).hypot(hero.position.y - ball.position.y)
                    > self.planner.config().npc_killer.npcs[0].combat_radius
            }) {
                plan.actions.push(ActionIntent::MoveTo {
                    destination: Point {
                        x: ball.position.x,
                        y: ball.position.y,
                    },
                });
            }
            return planned(plan, "Following SpaceBall");
        }
        if plan.selected_npc.is_none() && plan.selected_box.is_none() && plan.actions.is_empty() {
            plan.suspended = true;
        }
        let status = if context.now_ms < self.collect_until {
            "Collecting SpaceBall cargo after goal"
        } else if plan.selected_npc.is_some() {
            "Attacking SpaceBall"
        } else if plan.selected_box.is_some() {
            "Collecting SpaceBall cargo"
        } else {
            "Waiting for SpaceBall or cargo"
        };
        planned(plan, status)
    }
    fn plan_failed(&mut self, plan: &BehaviorDecision) {
        if let Some(target) = &plan.selected_box {
            self.planner.collection_failed(target);
        }
    }
}
fn planned(plan: BehaviorDecision, status: impl Into<String>) -> ModuleOutput {
    ModuleOutput {
        state: RunState::Running,
        status: status.into(),
        plan: Some(plan),
    }
}
fn entity_ref(entity: &GameEntity) -> EntityRef {
    EntityRef {
        path: entity.path.clone(),
        id: entity.id.clone(),
        name: entity.name.clone(),
        class_name: entity.class_name.clone(),
        position: Point {
            x: entity.position.x,
            y: entity.position.y,
        },
    }
}
fn wait(snapshot: &GameSnapshot) -> BehaviorDecision {
    BehaviorDecision {
        mode: BehaviorMode::KillAndCollect,
        player: snapshot
            .entities
            .iter()
            .find(|e| e.category == EntityCategory::Player)
            .map(entity_ref),
        selected_npc: None,
        selected_box: None,
        actions: vec![],
        note: None,
        controls: ControlRequests::default(),
        suspended: true,
        can_refresh: false,
    }
}
