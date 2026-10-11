use std::collections::HashMap;

use serde_json::Value;
use xenomit_game_api::{EntityCategory, GameEntity, GameSnapshot};

use crate::context::entity_key;
use crate::{BehaviorConfig, BehaviorMode, EntitySelector, ModuleContext, Navigation, NpcFlag};

/// A coordinate in the behavior engine's configured coordinate system.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    fn distance(self, other: Self) -> f64 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

/// Snapshot entity identity and display information needed by an executor.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityRef {
    pub path: String,
    pub id: Option<Value>,
    pub name: String,
    pub class_name: String,
    /// Position transformed according to `BehaviorConfig::coordinates`.
    pub position: Point,
}

impl EntityRef {
    /// Use the normalized id where available and the display-list path as a
    /// best-effort fallback. Paths are only guaranteed within one snapshot.
    pub fn key(&self) -> String {
        self.id
            .as_ref()
            .map(|id| format!("id:{id}"))
            .unwrap_or_else(|| format!("path:{}", self.path))
    }
}

/// Declarative action requested by the behavior engine.
#[derive(Clone, Debug, PartialEq)]
pub enum ActionIntent {
    MoveTo {
        destination: Point,
    },
    AttackNpc {
        target: EntityRef,
    },
    CollectBox {
        target: EntityRef,
        /// Configured delay before this same box should be requested again.
        wait_ms: u64,
    },
    /// Preserve collection movement instead of roaming or circling away.
    WaitForCollection {
        until_ms: u64,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShipMode {
    Roam,
    Run,
    Attack,
}

/// Requests for manager operations. The adapter implements these through its
/// pet/items/ship-mode/map-traveler APIs; they are not reflected method names.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControlRequests {
    pub npc_flags: Vec<NpcFlag>,
    /// Typed commands supported by the host's game-client implementation.
    pub commands: Vec<xenomit_game_api::GameCommand>,
    pub pet_enabled: Option<bool>,
    pub ship_mode: Option<ShipMode>,
    pub cloak: bool,
    /// Start a temporal MapModule, retaining this engine/mode as its back module.
    /// The caller ticks its MapTraveler until arrival or route failure.
    pub travel_to_map: Option<i32>,
}

/// Result of one planning step. The caller translates intents to its own
/// `GameClient::call_function` bindings and handles invocation failures.
#[derive(Clone, Debug, PartialEq)]
pub struct BehaviorDecision {
    pub mode: BehaviorMode,
    pub player: Option<EntityRef>,
    pub selected_npc: Option<EntityRef>,
    pub selected_box: Option<EntityRef>,
    pub actions: Vec<ActionIntent>,
    pub note: Option<String>,
    pub controls: ControlRequests,
    /// Prevent an executor's idle/roaming fallback during safety, travel or wait.
    pub suspended: bool,
    pub can_refresh: bool,
}

impl BehaviorDecision {
    /// Convert planner output once before passing it to a world-coordinate
    /// executor such as the desktop runtime. Typed commands contain no points.
    pub fn into_world_coordinates(mut self, transform: crate::CoordinateTransform) -> Self {
        let convert = |point: &mut Point| {
            let (x, y) = transform.inverse_apply(point.x, point.y);
            *point = Point { x, y };
        };
        for entity in [
            &mut self.player,
            &mut self.selected_npc,
            &mut self.selected_box,
        ]
        .into_iter()
        .flatten()
        {
            convert(&mut entity.position);
        }
        for action in &mut self.actions {
            match action {
                ActionIntent::MoveTo { destination } => convert(destination),
                ActionIntent::AttackNpc { target } | ActionIntent::CollectBox { target, .. } => {
                    convert(&mut target.position)
                }
                ActionIntent::WaitForCollection { .. } => {}
            }
        }
        self
    }
}

/// Stateful target selection and action planner.
pub struct BehaviorEngine {
    config: BehaviorConfig,
    current_npc_key: Option<String>,
    current_box_key: Option<String>,
    collection_wait: Option<(String, u64)>,
    collection_retries: HashMap<String, u64>,
    npc_blacklist: HashMap<String, u64>,
    backwards: bool,
    last_cloak: Option<u64>,
    last_map: Option<i32>,
    random_state: u64,
}

impl BehaviorEngine {
    pub fn new(config: BehaviorConfig) -> Result<Self, StepError> {
        config.validate()?;
        Ok(Self {
            config,
            current_npc_key: None,
            current_box_key: None,
            collection_wait: None,
            collection_retries: HashMap::new(),
            npc_blacklist: HashMap::new(),
            backwards: false,
            last_cloak: None,
            last_map: None,
            random_state: 0x9e3779b97f4a7c15,
        })
    }

    pub fn from_json(json: &str) -> Result<Self, StepError> {
        Self::new(BehaviorConfig::from_json(json)?)
    }

    pub fn config(&self) -> &BehaviorConfig {
        &self.config
    }
    /// Highest-priority matching NPC rule, useful for adapter attack policy.
    pub fn npc_rule(&self, entity: &GameEntity) -> Option<&crate::NpcRule> {
        self.config
            .npc_killer
            .npcs
            .iter()
            .filter(|rule| matches_selector(entity, &rule.selector))
            .min_by_key(|rule| rule.priority)
    }

    /// Highest-priority matching NPC rule that is active on `map_id`.
    pub fn npc_rule_on_map(
        &self,
        entity: &GameEntity,
        map_id: Option<i32>,
    ) -> Option<&crate::NpcRule> {
        self.config
            .npc_killer
            .npcs
            .iter()
            .filter(|rule| rule_matches_map(rule, map_id))
            .filter(|rule| matches_selector(entity, &rule.selector))
            .min_by_key(|rule| rule.priority)
    }

    /// Replace configuration and clear target/wait state after validation.
    pub fn replace_config(&mut self, config: BehaviorConfig) -> Result<(), StepError> {
        config.validate()?;
        self.config = config;
        self.reset_state();
        Ok(())
    }

    pub fn reset_state(&mut self) {
        self.current_npc_key = None;
        self.current_box_key = None;
        self.collection_wait = None;
        self.collection_retries.clear();
        self.npc_blacklist.clear();
        self.backwards = false;
        self.last_cloak = None;
        self.last_map = None;
    }

    /// Plan one behavior step from a fresh API snapshot. `now_ms` is a
    /// nondecreasing millisecond counter supplied by the caller; the API
    /// snapshot timestamp can be used when steps come from the same client.
    pub fn step(&mut self, snapshot: &GameSnapshot, now_ms: u64) -> BehaviorDecision {
        let context = self
            .find_player(snapshot)
            .map(|hero| ModuleContext::from_snapshot(snapshot, hero, self.config.coordinates))
            .unwrap_or_default();
        let navigation =
            crate::ObstacleNavigation::from_snapshot(snapshot, self.config.coordinates);
        self.step_with_context(snapshot, now_ms, &context, &navigation)
    }

    /// Use richer AttackAPI/MovementAPI/StatsAPI observations when available.
    /// Unknown manager state is permissive rather than invented from HP/IDs.
    pub fn step_with_context(
        &mut self,
        snapshot: &GameSnapshot,
        now_ms: u64,
        context: &ModuleContext,
        navigation: &dyn Navigation,
    ) -> BehaviorDecision {
        let map = snapshot.map.as_ref().and_then(|map| map.id);
        if self.last_map.is_some() && map != self.last_map {
            self.current_npc_key = None;
            self.current_box_key = None;
            self.collection_wait = None;
            self.collection_retries.clear();
            self.npc_blacklist.clear();
        }
        self.last_map = map;
        self.npc_blacklist.retain(|_, until| *until > now_ms);
        self.collection_retries
            .retain(|key, _| snapshot.entities.iter().any(|e| entity_key(e) == *key));
        let Some(player_entity) = self.find_player(snapshot) else {
            self.current_npc_key = None;
            self.current_box_key = None;
            return BehaviorDecision {
                mode: self.config.mode,
                player: None,
                selected_npc: None,
                selected_box: None,
                actions: Vec::new(),
                note: Some("player entity was not present in the snapshot".into()),
                controls: ControlRequests::default(),
                suspended: true,
                can_refresh: false,
            };
        };
        let player_pos = self.point(player_entity);
        let player = self.entity_ref(player_entity);
        let waiting = self.waiting_box(snapshot, context, now_ms);
        // Collector's wait precedes safety; mixed mode checks safety first.
        if self.config.mode == BehaviorMode::Collector {
            if let Some(waiting) = waiting {
                return self.wait_decision(player, waiting, None);
            }
        }
        if !context.safe_to_work {
            self.current_npc_key = None;
            return BehaviorDecision {
                mode: self.config.mode,
                player: Some(player),
                selected_npc: None,
                selected_box: None,
                actions: context
                    .safety_destination
                    .filter(|p| navigation.can_move(*p))
                    .map(|destination| vec![ActionIntent::MoveTo { destination }])
                    .unwrap_or_default(),
                note: Some(
                    context
                        .safety_status
                        .clone()
                        .unwrap_or_else(|| "Waiting for safety".into()),
                ),
                controls: ControlRequests {
                    ship_mode: Some(ShipMode::Run),
                    ..Default::default()
                },
                suspended: true,
                can_refresh: waiting.is_none() && context.safety_waiting,
            };
        }
        if let Some(target_map) = self
            .config
            .working_map
            .filter(|target| map.is_some_and(|id| id != *target))
        {
            self.current_npc_key = None;
            self.current_box_key = None;
            return BehaviorDecision {
                mode: self.config.mode,
                player: Some(player),
                selected_npc: None,
                selected_box: None,
                actions: Vec::new(),
                note: Some(format!("Traveling to working map {target_map}")),
                controls: ControlRequests {
                    travel_to_map: Some(target_map),
                    ship_mode: Some(ShipMode::Run),
                    ..Default::default()
                },
                suspended: true,
                can_refresh: false,
            };
        }
        let npc = if self.uses_npcs() {
            self.choose_npc(snapshot, player_pos, context, navigation, now_ms)
        } else {
            None
        };
        if self.uses_boxes() {
            if let Some(waiting) = waiting {
                return self.wait_decision(player, waiting, npc.as_ref());
            }
        }
        let box_candidate = if self.uses_boxes() {
            self.choose_box(snapshot, player_pos, context, navigation)
        } else {
            None
        };

        let mut actions = Vec::new();
        let selected_npc = npc
            .as_ref()
            .map(|candidate| self.entity_ref(candidate.entity));
        let remembered_box = box_candidate.as_ref().map(|b| entity_key(b.entity));
        let box_candidate =
            box_candidate.filter(|b| npc.as_ref().is_none_or(|n| self.collect_in_combat(b, n)));
        let selected_box = box_candidate
            .as_ref()
            .map(|candidate| self.entity_ref(candidate.entity));
        let mut note = None;
        let mut controls = ControlRequests {
            npc_flags: npc
                .as_ref()
                .map(|n| n.rule.flags.clone())
                .unwrap_or_default(),
            pet_enabled: Some(self.config.pet_enabled),
            ship_mode: Some(ShipMode::Roam),
            ..Default::default()
        };
        if self.config.mode == BehaviorMode::Collector {
            controls.cloak = self.config.collector.auto_cloak
                && !context.invisible
                && self
                    .last_cloak
                    .is_none_or(|last| now_ms.saturating_sub(last) > 30_000);
            // Keep danger avoidance from being overwritten by a box movement.
            if self.config.collector.stay_away_from_enemies {
                if let Some(enemy) = snapshot
                    .entities
                    .iter()
                    .filter(|e| entity_key(e) != entity_key(player_entity))
                    .filter(|e| {
                        let o = context.observation(e);
                        o.enemy
                            && !o.invisible
                            && finite_position(e)
                            && player_pos.distance(self.point(e))
                                < self.config.collector.danger_distance
                    })
                    .min_by(|a, b| {
                        player_pos
                            .distance(self.point(a))
                            .total_cmp(&player_pos.distance(self.point(b)))
                    })
                {
                    let center = self.point(enemy);
                    let angle = (player_pos.y - center.y).atan2(player_pos.x - center.x);
                    let destination = self.valid_location(
                        center,
                        angle,
                        self.config.collector.danger_distance + 100.0,
                        navigation,
                    );
                    let reachable = navigation.can_move(destination);
                    if reachable {
                        actions.push(ActionIntent::MoveTo { destination });
                    }
                    controls.ship_mode = Some(ShipMode::Run);
                    self.current_box_key = None;
                    return BehaviorDecision {
                        mode: self.config.mode,
                        player: Some(player),
                        selected_npc: None,
                        selected_box: None,
                        actions,
                        note: Some(
                            if reachable {
                                "Staying away from enemy"
                            } else {
                                "Enemy nearby; no reachable avoidance position"
                            }
                            .into(),
                        ),
                        controls,
                        suspended: !reachable,
                        can_refresh: true,
                    };
                }
            }
        }

        match self.config.mode {
            BehaviorMode::Collector => {
                if let Some(candidate) = box_candidate {
                    self.plan_collection(candidate, now_ms, context, &mut actions);
                } else {
                    note = Some("no configured collectible box is in range".into());
                }
            }
            BehaviorMode::NpcKiller => {
                if let Some(candidate) = npc {
                    self.plan_combat(
                        candidate,
                        player_pos,
                        snapshot,
                        context,
                        navigation,
                        &mut actions,
                        &mut controls,
                    );
                } else {
                    note = Some(self.npc_idle_note(snapshot, player_pos));
                }
            }
            BehaviorMode::KillAndCollect => match (npc, box_candidate) {
                (Some(npc), Some(box_candidate)) => {
                    // LootCollector gives the collector sole movement control.
                    // Attack continues, but orbiting must not cancel the pickup.
                    controls.ship_mode =
                        Some(self.ship_mode(&npc, player_pos, self.point(box_candidate.entity)));
                    self.plan_collection(box_candidate, now_ms, context, &mut actions);
                    actions.push(ActionIntent::AttackNpc {
                        target: self.entity_ref(npc.entity),
                    });
                }
                (Some(npc), None) => self.plan_combat(
                    npc,
                    player_pos,
                    snapshot,
                    context,
                    navigation,
                    &mut actions,
                    &mut controls,
                ),
                (None, Some(box_candidate)) => {
                    self.plan_collection(box_candidate, now_ms, context, &mut actions)
                }
                (None, None) => {
                    note = Some("no configured NPC or box is in range".into());
                }
            },
        }

        self.current_npc_key = selected_npc.as_ref().map(EntityRef::key);
        self.current_box_key = remembered_box;

        BehaviorDecision {
            mode: self.config.mode,
            player: Some(player),
            selected_npc,
            selected_box,
            actions,
            note,
            controls,
            suspended: false,
            can_refresh: self.collection_wait.is_none()
                && (self.config.mode == BehaviorMode::Collector
                    || (self.current_npc_key.is_none() && context.safety_waiting)),
        }
    }

    /// Call only after successful cloak use. Failed item use must not start
    /// the 30-second cooldown (CollectorModule.ifSuccessful).
    pub fn cloak_used(&mut self, now_ms: u64) {
        self.last_cloak = Some(now_ms);
    }

    /// Undo optimistic collection state when the executor could not collect.
    pub fn collection_failed(&mut self, target: &EntityRef) {
        let key = target.key();
        if self
            .collection_wait
            .as_ref()
            .is_some_and(|(current, _)| current == &key)
        {
            self.collection_wait = None;
            if let Some(retries) = self.collection_retries.get_mut(&key) {
                *retries = retries.saturating_sub(1);
            }
        }
    }

    fn waiting_box<'a>(
        &mut self,
        snapshot: &'a GameSnapshot,
        context: &ModuleContext,
        now_ms: u64,
    ) -> Option<(&'a GameEntity, u64)> {
        let (key, until) = self.collection_wait.as_ref()?;
        if let Some(entity) = snapshot
            .entities
            .iter()
            .find(|e| entity_key(e) == *key && finite_position(e))
        {
            let state = context.observation(entity);
            // isCollected also means a recent attempt in Darkbot's Box API;
            // only invalidity/removal interrupts an active pickup wait.
            if state.valid != Some(false) && now_ms <= *until {
                return Some((entity, *until));
            }
        }
        self.collection_wait = None;
        None
    }

    fn wait_decision(
        &mut self,
        player: EntityRef,
        (entity, until_ms): (&GameEntity, u64),
        npc: Option<&NpcCandidate<'_>>,
    ) -> BehaviorDecision {
        let selected_box = Some(self.entity_ref(entity));
        let selected_npc = npc.map(|n| self.entity_ref(n.entity));
        self.current_box_key = selected_box.as_ref().map(EntityRef::key);
        self.current_npc_key = selected_npc.as_ref().map(EntityRef::key);
        let mut actions = vec![ActionIntent::WaitForCollection { until_ms }];
        if let Some(target) = selected_npc.clone() {
            actions.push(ActionIntent::AttackNpc { target });
        }
        BehaviorDecision {
            mode: self.config.mode,
            player: Some(player),
            selected_npc,
            selected_box,
            actions,
            note: Some("Waiting for collection".into()),
            controls: ControlRequests {
                npc_flags: npc.map(|n| n.rule.flags.clone()).unwrap_or_default(),
                pet_enabled: (self.config.mode == BehaviorMode::KillAndCollect
                    && self.config.pet_enabled)
                    .then_some(true),
                ..Default::default()
            },
            suspended: true,
            can_refresh: false,
        }
    }

    fn uses_npcs(&self) -> bool {
        matches!(
            self.config.mode,
            BehaviorMode::NpcKiller | BehaviorMode::KillAndCollect
        )
    }

    fn uses_boxes(&self) -> bool {
        matches!(
            self.config.mode,
            BehaviorMode::Collector | BehaviorMode::KillAndCollect
        )
    }

    fn find_player<'a>(&self, snapshot: &'a GameSnapshot) -> Option<&'a GameEntity> {
        if let Some(selector) = &self.config.player {
            return snapshot.entities.iter().find(|entity| {
                finite_position(entity)
                    && finite_point(self.point(entity))
                    && matches_selector(entity, selector)
            });
        }
        snapshot.entities.iter().find(|entity| {
            finite_position(entity)
                && finite_point(self.point(entity))
                && entity.category == EntityCategory::Player
        })
    }

    fn choose_npc<'a>(
        &mut self,
        snapshot: &'a GameSnapshot,
        player: Point,
        context: &ModuleContext,
        navigation: &dyn Navigation,
        now_ms: u64,
    ) -> Option<NpcCandidate<'a>> {
        let current = self.current_npc_key.clone();
        let current_map = snapshot
            .navigation
            .map_id
            .or_else(|| snapshot.map.as_ref().and_then(|map| map.id));
        snapshot
            .entities
            .iter()
            .filter_map(|entity| {
                let rule = self
                    .config
                    .npc_killer
                    .npcs
                    .iter()
                    .filter(|rule| rule_matches_map(rule, current_map))
                    .filter(|rule| matches_selector(entity, &rule.selector))
                    .min_by_key(|rule| rule.priority)
                    .cloned()?;
                let state = context.observation(entity);
                if !finite_position(entity) || dead(entity) || state.valid == Some(false) {
                    return None;
                }
                let point = self.point(entity);
                if !finite_point(point) {
                    return None;
                }
                let distance = player.distance(point);
                let key = self.entity_ref(entity).key();
                if distance > self.config.npc_killer.max_target_distance {
                    if current.as_deref() == Some(key.as_str())
                        && context.hero_locked_target.as_deref() == Some(key.as_str())
                    {
                        self.npc_blacklist.insert(key, now_ms.saturating_add(5000));
                    }
                    return None;
                }
                let is_current = current.as_deref() == Some(key.as_str());
                let locked = context.hero_locked_target.as_deref() == Some(key.as_str());
                let closest = state
                    .closest_distance
                    .unwrap_or_else(|| navigation.closest_distance(entity, point));
                let radius = state
                    .radius
                    .filter(|v| v.is_finite() && *v >= 0.0)
                    .unwrap_or(rule.combat_radius);
                let mut rule = rule;
                rule.combat_radius = radius;
                // LootModule.ignoreInvalidTarget: short blacklist before lock,
                // longer blacklist for ownership, distance and stuck targets.
                if is_current || locked {
                    let ignored = if !locked && closest > 600.0 {
                        Some(1000)
                    } else if locked
                        && (state.bugged
                            || (state.owned == Some(false)
                                && !rule.flags.contains(&NpcFlag::IgnoreOwnership))
                            || (closest > 650.0 && health_fraction(entity) > 0.9)
                            || (closest > 500.0
                                && state.moving == Some(false)
                                && (state.shield_increased || shield_fraction(entity) > 0.99)))
                    {
                        Some(5000)
                    } else if locked
                        && entity.name.contains("Invoke")
                        && rule.flags.contains(&NpcFlag::Passive)
                        && context.hero_local_target.as_deref() == Some(key.as_str())
                        && !context.casting_ability
                    {
                        Some(600_000)
                    } else {
                        None
                    };
                    if let Some(duration) = ignored {
                        self.npc_blacklist
                            .insert(key.clone(), now_ms.saturating_add(duration));
                        return None;
                    }
                }
                if self.npc_blacklist.contains_key(&key) || state.blacklisted {
                    return None;
                }
                let attacking_current =
                    is_current && context.hero_attacking.as_deref() == Some(key.as_str());
                if !attacking_current {
                    if state.attacked_by_others && !rule.flags.contains(&NpcFlag::IgnoreAttacked) {
                        self.npc_blacklist
                            .insert(key.clone(), now_ms.saturating_add(20_000));
                        return None;
                    }
                    if !rule.kill
                        || closest >= 500.0
                        || (self.config.npc_killer.only_kill_preferred
                            && !state
                                .preferred
                                .unwrap_or_else(|| navigation.in_preferred_zone(point)))
                        || (rule.flags.contains(&NpcFlag::LeechOnly) && !context.energy_leech)
                        || (rule.flags.contains(&NpcFlag::AttackSecond)
                            && !state.attacked_by_others)
                        || (rule.flags.contains(&NpcFlag::Passive)
                            && !entity.name.contains("Invoke")
                            && !state.attacking_hero)
                    {
                        return None;
                    }
                }
                let health_fraction = health_fraction(entity);
                let sticky_bonus = if is_current
                    && (context.hero_local_target.as_deref() == Some(key.as_str())
                        || distance < 600.0)
                {
                    20 - (health_fraction * 10.0) as i64
                } else {
                    0
                };
                let priority = i64::from(rule.priority)
                    - sticky_bonus
                    - if is_current {
                        i64::from(self.config.npc_killer.current_target_priority_bonus)
                    } else {
                        0
                    };
                Some(NpcCandidate {
                    entity,
                    rule,
                    priority,
                    health_fraction,
                    distance,
                })
            })
            .min_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| a.health_fraction.total_cmp(&b.health_fraction))
                    .then_with(|| a.distance.total_cmp(&b.distance))
            })
    }

    fn choose_box<'a>(
        &self,
        snapshot: &'a GameSnapshot,
        player: Point,
        context: &ModuleContext,
        navigation: &dyn Navigation,
    ) -> Option<BoxCandidate<'a>> {
        let current = self.current_box_key.as_deref();
        let candidates: Vec<_> = snapshot
            .entities
            .iter()
            .filter_map(|entity| {
                let rule = self
                    .config
                    .collector
                    .boxes
                    .iter()
                    .filter(|rule| matches_selector(entity, &rule.selector))
                    .min_by_key(|rule| rule.priority)
                    .cloned()?;
                let state = context.observation(entity);
                if !rule.collect
                    || !finite_position(entity)
                    || state.valid == Some(false)
                    || state.collected
                    || state.collectable == Some(false)
                {
                    return None;
                }
                let entity_ref = self.entity_ref(entity);
                let key = entity_ref.key();
                let point = self.point(entity);
                if !finite_point(point) {
                    return None;
                }
                let distance = player.distance(point);
                if distance > self.config.collector.max_distance {
                    return None;
                }
                if state
                    .closest_distance
                    .unwrap_or_else(|| navigation.closest_distance(entity, point))
                    >= 200.0
                {
                    return None;
                }
                let resource = rule.resource
                    || entity.name == "FROM_SHIP"
                    || entity.name == "PROSPEROUS_CARGO"
                    || entity
                        .fields
                        .get("typeName")
                        .and_then(Value::as_str)
                        .is_some_and(|s| s == "FROM_SHIP" || s == "PROSPEROUS_CARGO");
                if resource
                    && context
                        .cargo
                        .zip(context.max_cargo)
                        .is_some_and(|(cargo, max)| cargo >= max)
                {
                    return None;
                }
                if self.config.collector.ignore_contested_boxes {
                    if let Some(speed) = context.hero_speed.filter(|s| s.is_finite() && *s > 0.0) {
                        let hero_time = distance / speed * 2.0;
                        if snapshot
                            .entities
                            .iter()
                            .filter(|e| {
                                matches!(e.category, EntityCategory::Ship | EntityCategory::Player)
                            })
                            .filter(|e| {
                                self.find_player(snapshot)
                                    .is_none_or(|hero| entity_key(e) != entity_key(hero))
                                    && finite_position(e)
                            })
                            .any(|e| {
                                let s = context.observation(e);
                                s.destination
                                    .is_some_and(|p| p.distance(point) < f64::EPSILON)
                                    && s.speed.filter(|v| v.is_finite() && *v > 0.0).is_some_and(
                                        |v| self.point(e).distance(point) / v < hero_time,
                                    )
                            })
                        {
                            return None;
                        }
                    }
                }
                let priority = if current == Some(key.as_str()) {
                    i64::from(rule.priority)
                        - i64::from(self.config.collector.current_target_priority_bonus)
                } else {
                    i64::from(rule.priority)
                };
                Some(BoxCandidate {
                    entity,
                    rule,
                    priority,
                    distance,
                })
            })
            .collect();
        let best = candidates.iter().min_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| a.distance.total_cmp(&b.distance))
        })?;
        let previous = candidates
            .iter()
            .find(|b| current == Some(entity_key(b.entity).as_str()));
        // CollectorModule.isBetter: do not bounce between nearby boxes. A new
        // box must save more than 150 units and the old one must exceed 100.
        Some(
            previous
                .filter(|old| old.distance <= 100.0 || old.distance - 150.0 <= best.distance)
                .unwrap_or(best)
                .clone(),
        )
    }

    fn collect_in_combat(&self, b: &BoxCandidate<'_>, n: &NpcCandidate<'_>) -> bool {
        let config = &self.config.kill_and_collect;
        config.collect_during_combat
            && b.distance <= config.collect_radius
            && n.health_fraction >= config.collect_above_target_health
            && (!n.rule.flags.contains(&NpcFlag::IgnoreBoxes)
                || self.point(b.entity).distance(self.point(n.entity))
                    <= config
                        .max_box_distance_from_npc
                        .min(n.rule.combat_radius * 2.0))
    }

    fn plan_collection(
        &mut self,
        candidate: BoxCandidate<'_>,
        now_ms: u64,
        context: &ModuleContext,
        actions: &mut Vec<ActionIntent>,
    ) {
        let point = self.point(candidate.entity);
        if candidate.distance < self.config.collector.pickup_radius {
            let target = self.entity_ref(candidate.entity);
            let key = target.key();
            let retries = self.collection_retries.entry(key.clone()).or_default();
            *retries = (*retries).max(
                context
                    .observation(candidate.entity)
                    .collection_retries
                    .unwrap_or(0),
            );
            *retries = retries.saturating_add(1);
            let travel_ms = context
                .hero_speed
                .filter(|s| s.is_finite() && *s > 0.0)
                .map(|s| (candidate.distance / s * 1000.0) as u64)
                .unwrap_or(0);
            let wait_ms = candidate
                .rule
                .wait_ms
                .saturating_add((*retries).saturating_mul(100).min(1000))
                .saturating_add(travel_ms)
                .saturating_add(30);
            self.collection_wait = Some((key, now_ms.saturating_add(wait_ms)));
            actions.push(ActionIntent::CollectBox { target, wait_ms });
        } else {
            actions.push(ActionIntent::MoveTo { destination: point });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn plan_combat(
        &mut self,
        candidate: NpcCandidate<'_>,
        player: Point,
        snapshot: &GameSnapshot,
        context: &ModuleContext,
        navigation: &dyn Navigation,
        actions: &mut Vec<ActionIntent>,
        controls: &mut ControlRequests,
    ) {
        let mut target_point = self.point(candidate.entity);
        let state = context.observation(candidate.entity);
        let speed = state
            .speed
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0);
        // LocationInfo.destinationInTime(250), without overshooting destination.
        if let Some(destination) = state.destination {
            let remaining = target_point.distance(destination);
            if remaining > 0.0 {
                let advance = (speed * 0.25).min(remaining) / remaining;
                target_point.x += (destination.x - target_point.x) * advance;
                target_point.y += (destination.y - target_point.y) * advance;
            }
        }
        let angle = if target_point.distance(player) > f64::EPSILON {
            (player.y - target_point.y).atan2(player.x - target_point.x)
        } else {
            stable_angle(&candidate.entity.path)
        };
        let distance = player.distance(self.point(candidate.entity));
        let radius = candidate.rule.combat_radius;
        let no_circle = candidate.rule.flags.contains(&NpcFlag::NoCircle) && radius <= 750.0;
        let destination = if candidate.rule.flags.contains(&NpcFlag::AggressiveFollow)
            && state.moving == Some(true)
            && distance > radius
        {
            polar(target_point, angle, radius.min(200.0))
        } else if no_circle {
            let min_radius = (radius - 200.0).min(radius * 0.5).max(0.0);
            if let Some(destination) = context.destination.filter(|p| {
                navigation.can_move(*p) && (min_radius..=radius).contains(&p.distance(target_point))
            }) {
                controls.ship_mode = Some(self.ship_mode(&candidate, player, destination));
                actions.push(ActionIntent::AttackNpc {
                    target: self.entity_ref(candidate.entity),
                });
                return;
            }
            let distance = min_radius + self.random() * (radius - min_radius - 10.0).max(0.0);
            let angle_diff = self.random() * 0.1 - 0.05;
            self.best_direction(
                snapshot,
                &candidate,
                target_point,
                angle,
                angle_diff,
                distance,
                context,
                navigation,
            )
        } else if radius > f64::EPSILON {
            let radius_fix = (radius - distance)
                .clamp(-radius / 2.0, radius / 2.0)
                .trunc();
            let corrected_radius = radius + radius_fix;
            let hero_speed = context
                .hero_speed
                .filter(|v| v.is_finite() && *v >= 0.0)
                .unwrap_or(300.0);
            let angle_diff = ((hero_speed * 0.625) + (speed.max(200.0) * 0.625)
                - player.distance(polar(target_point, angle, corrected_radius)))
            .max(0.0)
                / corrected_radius;
            self.best_direction(
                snapshot,
                &candidate,
                target_point,
                angle,
                angle_diff,
                corrected_radius,
                context,
                navigation,
            )
        } else {
            target_point
        };
        controls.ship_mode = Some(self.ship_mode(&candidate, player, destination));
        // Circling intentionally continues at the configured radius. The old
        // planner stopped moving as soon as the desired radius was reached.
        if player.distance(destination) > self.config.npc_killer.position_tolerance
            && navigation.can_move(destination)
        {
            actions.push(ActionIntent::MoveTo { destination });
        }
        actions.push(ActionIntent::AttackNpc {
            target: self.entity_ref(candidate.entity),
        });
    }

    fn ship_mode(&self, npc: &NpcCandidate<'_>, player: Point, direction: Point) -> ShipMode {
        let distance = player.distance(direction);
        if self.config.npc_killer.run_config_in_circle
            && npc.health_fraction < 0.25
            && distance > npc.rule.combat_radius * 2.0
        {
            ShipMode::Run
        } else if distance > npc.rule.combat_radius * 3.0 {
            ShipMode::Roam
        } else {
            ShipMode::Attack
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn best_direction(
        &mut self,
        snapshot: &GameSnapshot,
        target: &NpcCandidate<'_>,
        center: Point,
        angle: f64,
        angle_diff: f64,
        distance: f64,
        context: &ModuleContext,
        navigation: &dyn Navigation,
    ) -> Point {
        let mut forward_score: f64 = 0.0;
        let mut backward_score: f64 = 0.0;
        for iteration in 1..=self.config.npc_killer.max_circle_iterations {
            forward_score += self.score(
                snapshot,
                target.entity,
                polar(center, angle + angle_diff * iteration as f64, distance),
                context,
                navigation,
            );
            backward_score += self.score(
                snapshot,
                target.entity,
                polar(center, angle - angle_diff * iteration as f64, distance),
                context,
                navigation,
            );
            if (forward_score < 0.0) != (backward_score < 0.0)
                || (forward_score - backward_score).abs() > 300.0
            {
                self.backwards = backward_score > forward_score;
                break;
            }
        }
        let selected_angle = angle + angle_diff * if self.backwards { -1.0 } else { 1.0 };
        self.valid_location(center, selected_angle, distance, navigation)
    }

    fn score(
        &self,
        snapshot: &GameSnapshot,
        target: &GameEntity,
        point: Point,
        context: &ModuleContext,
        navigation: &dyn Navigation,
    ) -> f64 {
        let penalty: f64 = snapshot
            .entities
            .iter()
            .filter(|e| {
                e.category == EntityCategory::Npc
                    && entity_key(e) != entity_key(target)
                    && finite_position(e)
            })
            .map(|e| {
                let radius = context
                    .observation(e)
                    .radius
                    .filter(|r| r.is_finite() && *r >= 0.0)
                    .or_else(|| {
                        self.config
                            .npc_killer
                            .npcs
                            .iter()
                            .filter(|r| matches_selector(e, &r.selector))
                            .min_by_key(|r| r.priority)
                            .map(|r| r.combat_radius)
                    })
                    .unwrap_or(560.0);
                (radius - self.point(e).distance(point)).max(0.0)
            })
            .sum();
        (if navigation.can_move(point) {
            0.0
        } else {
            -1000.0
        }) - penalty
    }

    fn valid_location(
        &self,
        center: Point,
        mut angle: f64,
        mut distance: f64,
        navigation: &dyn Navigation,
    ) -> Point {
        let mut point = polar(center, angle, distance);
        while !navigation.can_move(point) && distance < 10_000.0 {
            angle += if self.backwards { -0.3 } else { 0.3 };
            distance += 2.0;
            point = polar(center, angle, distance);
        }
        if distance >= 10_000.0 {
            point = polar(center, angle, 500.0);
        }
        point
    }

    fn random(&mut self) -> f64 {
        self.random_state ^= self.random_state << 13;
        self.random_state ^= self.random_state >> 7;
        self.random_state ^= self.random_state << 17;
        (self.random_state >> 11) as f64 / ((1_u64 << 53) as f64)
    }

    /// Why `choose_npc` returned no target: whether any NPCs are in the
    /// snapshot at all, whether the enabled rules match any of them, or how
    /// far away the nearest match is. The bot surfaces this verbatim so an
    /// idle module says what it is waiting for.
    fn npc_idle_note(&self, snapshot: &GameSnapshot, player: Point) -> String {
        let current_map = snapshot
            .navigation
            .map_id
            .or_else(|| snapshot.map.as_ref().and_then(|map| map.id));
        let mut seen = 0usize;
        let mut matched = 0usize;
        let mut nearest: Option<(f64, &GameEntity)> = None;
        let mut nearest_matched: Option<(f64, &GameEntity)> = None;
        for entity in &snapshot.entities {
            if entity.category != EntityCategory::Npc || !finite_position(entity) || dead(entity) {
                continue;
            }
            seen += 1;
            let distance = player.distance(self.point(entity));
            if nearest.is_none_or(|(best, _)| distance < best) {
                nearest = Some((distance, entity));
            }
            let hits = self.config.npc_killer.npcs.iter().any(|rule| {
                rule.kill
                    && rule_matches_map(rule, current_map)
                    && matches_selector(entity, &rule.selector)
            });
            if hits {
                matched += 1;
                if nearest_matched.is_none_or(|(best, _)| distance < best) {
                    nearest_matched = Some((distance, entity));
                }
            }
        }
        if seen == 0 {
            return "no NPC entities in the snapshot".into();
        }
        if matched == 0 {
            let nearest_name = nearest
                .map(|(_, entity)| entity.name.as_str())
                .unwrap_or("?");
            return format!(
                "{seen} NPCs in the snapshot, but none match the enabled kill list (nearest: {nearest_name})"
            );
        }
        let (distance, entity) = nearest_matched.unwrap();
        if distance <= self.config.npc_killer.max_target_distance {
            return "no eligible NPC target (flags, blacklist, preferred zone or reachability)"
                .into();
        }
        format!(
            "nearest enabled NPC '{}' is {distance:.0}u away (limit {:.0}u)",
            entity.name, self.config.npc_killer.max_target_distance
        )
    }

    fn point(&self, entity: &GameEntity) -> Point {
        let (x, y) = self
            .config
            .coordinates
            .apply(entity.position.x, entity.position.y);
        Point { x, y }
    }

    fn entity_ref(&self, entity: &GameEntity) -> EntityRef {
        EntityRef {
            path: entity.path.clone(),
            id: entity.id.clone(),
            name: entity.name.clone(),
            class_name: entity.class_name.clone(),
            position: self.point(entity),
        }
    }
}

impl Default for BehaviorEngine {
    fn default() -> Self {
        Self::new(BehaviorConfig::default()).expect("default behavior configuration is valid")
    }
}

#[derive(Clone, Debug, thiserror::Error)]
pub enum StepError {
    #[error("invalid behavior configuration: {0}")]
    InvalidConfig(String),
}

struct NpcCandidate<'a> {
    entity: &'a GameEntity,
    rule: crate::NpcRule,
    priority: i64,
    health_fraction: f64,
    distance: f64,
}

#[derive(Clone)]
struct BoxCandidate<'a> {
    entity: &'a GameEntity,
    rule: crate::BoxRule,
    priority: i64,
    distance: f64,
}

fn matches_selector(entity: &GameEntity, selector: &EntitySelector) -> bool {
    selector
        .category
        .is_none_or(|category| entity.category == category)
        && selector
            .name
            .as_deref()
            .is_none_or(|value| entity.name.eq_ignore_ascii_case(value))
        && selector.name_contains.as_deref().is_none_or(|value| {
            entity
                .name
                .to_ascii_lowercase()
                .contains(&value.to_ascii_lowercase())
        })
        && selector
            .class_name
            .as_deref()
            .is_none_or(|value| entity.class_name.eq_ignore_ascii_case(value))
        && selector.class_name_contains.as_deref().is_none_or(|value| {
            entity
                .class_name
                .to_ascii_lowercase()
                .contains(&value.to_ascii_lowercase())
        })
        && selector
            .path
            .as_deref()
            .is_none_or(|value| entity.path == value)
        && selector
            .id
            .as_ref()
            .is_none_or(|value| entity.id.as_ref() == Some(value))
        && selector
            .fields
            .iter()
            .all(|(key, value)| entity.fields.get(key) == Some(value))
}

fn rule_matches_map(rule: &crate::NpcRule, map_id: Option<i32>) -> bool {
    rule.map_ids.is_empty() || map_id.is_some_and(|map| rule.map_ids.contains(&map))
}

fn finite_position(entity: &GameEntity) -> bool {
    entity.position.x.is_finite() && entity.position.y.is_finite()
}

fn finite_point(point: Point) -> bool {
    point.x.is_finite() && point.y.is_finite()
}

/// An entity only counts as dead when health is readable *and* exhausted —
/// `0`/`0` or an unreadable pair means the snapshot lacks HP data, not that
/// the NPC died.
fn dead(entity: &GameEntity) -> bool {
    match (entity.health, entity.max_health) {
        (Some(health), Some(maximum)) => health <= 0.0 && maximum > 0.0,
        _ => false,
    }
}

fn health_fraction(entity: &GameEntity) -> f64 {
    match (entity.health, entity.max_health) {
        (Some(current), Some(maximum))
            if current.is_finite() && maximum.is_finite() && maximum > 0.0 =>
        {
            (current / maximum).clamp(0.0, 1.0)
        }
        _ => 1.0,
    }
}

fn shield_fraction(entity: &GameEntity) -> f64 {
    match (entity.shield, entity.max_shield) {
        (Some(current), Some(maximum))
            if current.is_finite() && maximum.is_finite() && maximum > 0.0 =>
        {
            (current / maximum).clamp(0.0, 1.0)
        }
        _ => 0.0,
    }
}

fn polar(center: Point, angle: f64, radius: f64) -> Point {
    Point {
        x: center.x + angle.cos() * radius,
        y: center.y + angle.sin() * radius,
    }
}

fn stable_angle(value: &str) -> f64 {
    let hash = value.bytes().fold(0_u64, |hash, byte| {
        hash.wrapping_mul(31).wrapping_add(u64::from(byte))
    });
    (hash % 628_319) as f64 / 100_000.0
}

/// Retained planner state for checkpoint export. Timestamps are stored as
/// ages/remaining durations relative to the export tick's `now_ms`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct EngineCheckpoint {
    pub current_npc_key: Option<String>,
    pub current_box_key: Option<String>,
    /// Box being collected and the remaining wait duration.
    pub collection_wait: Option<CollectionWait>,
    /// Observed collection failures per box key.
    pub collection_retries: HashMap<String, u64>,
    /// NPC keys blacklisted until a remaining duration elapses.
    pub npc_blacklist_ms: HashMap<String, u64>,
    pub backwards: bool,
    /// Milliseconds since the cloak item was used.
    pub last_cloak_age_ms: Option<u64>,
    pub last_map: Option<i32>,
    /// PRNG continuity so patrol/evasion choices differ after restore.
    pub random_state: u64,
}

/// A collection wait as a box key plus the remaining wait duration.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CollectionWait {
    pub key: String,
    pub remaining_ms: u64,
}

impl Default for EngineCheckpoint {
    fn default() -> Self {
        Self {
            current_npc_key: None,
            current_box_key: None,
            collection_wait: None,
            collection_retries: HashMap::new(),
            npc_blacklist_ms: HashMap::new(),
            backwards: false,
            last_cloak_age_ms: None,
            last_map: None,
            random_state: 0x9e3779b97f4a7c15,
        }
    }
}

impl BehaviorEngine {
    /// Export retained planner state relative to `now_ms`.
    pub fn export_checkpoint(&self, now_ms: u64) -> EngineCheckpoint {
        EngineCheckpoint {
            current_npc_key: self.current_npc_key.clone(),
            current_box_key: self.current_box_key.clone(),
            collection_wait: self
                .collection_wait
                .as_ref()
                .map(|(key, until)| CollectionWait {
                    key: key.clone(),
                    remaining_ms: until.saturating_sub(now_ms),
                }),
            collection_retries: self.collection_retries.clone(),
            npc_blacklist_ms: self
                .npc_blacklist
                .iter()
                .filter(|(_, until)| **until > now_ms)
                .map(|(key, until)| (key.clone(), until.saturating_sub(now_ms)))
                .collect(),
            backwards: self.backwards,
            last_cloak_age_ms: self.last_cloak.map(|at| now_ms.saturating_sub(at)),
            last_map: self.last_map,
            random_state: self.random_state,
        }
    }

    /// Restore planner state exported by [`BehaviorEngine::export_checkpoint`].
    /// Absent fields keep fresh defaults; the map is not cleared by restore.
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: &EngineCheckpoint,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        self.current_npc_key = checkpoint.current_npc_key.clone();
        self.current_box_key = checkpoint.current_box_key.clone();
        self.collection_wait = checkpoint.collection_wait.as_ref().map(|wait| {
            (wait.key.clone(), now_ms.saturating_add(wait.remaining_ms))
        });
        self.collection_retries = checkpoint.collection_retries.clone();
        self.npc_blacklist = checkpoint
            .npc_blacklist_ms
            .iter()
            .map(|(key, remaining)| (key.clone(), now_ms.saturating_add(*remaining)))
            .collect();
        self.backwards = checkpoint.backwards;
        self.last_cloak = checkpoint
            .last_cloak_age_ms
            .map(|age| now_ms.saturating_sub(age));
        self.last_map = checkpoint.last_map;
        self.random_state = checkpoint.random_state;
        Ok(())
    }
}
