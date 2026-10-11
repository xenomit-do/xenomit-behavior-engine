//! Shortest directed map routes and a retained, nonblocking portal traveler.
use crate::Point;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use xenomit_game_api::{
    EntityCategory, GameCommand, GameEntity, GameSnapshot, MapDescriptor, MapLink, PortalInfo,
    StarSystem,
};

/// Resolve map selectors from IDs, numeric strings, live map names, or the
/// bundled catalog used by clients that do not expose map metadata.
pub fn resolve_map_id(value: &Value, system: &StarSystem) -> Option<i32> {
    value
        .as_i64()
        .and_then(|id| i32::try_from(id).ok())
        .or_else(|| {
            let name = value.as_str()?.trim();
            name.parse::<i32>().ok().or_else(|| {
                system
                    .maps
                    .iter()
                    .find(|map| map.name.eq_ignore_ascii_case(name))
                    .map(|map| map.id)
            })
        })
        .or_else(|| {
            let name = value.as_str()?.trim();
            xenomit_game_api::star_map::MAPS
                .iter()
                .find(|(_, map_name, _)| map_name.eq_ignore_ascii_case(name))
                .map(|(id, _, _)| *id)
        })
}

/// Fewest portal jumps among known, level-accessible links. No reverse links
/// or unknown destination maps are guessed. Excluded links support recovery.
pub fn shortest_map_route(
    system: &StarSystem,
    from: i32,
    to: i32,
    level: Option<u32>,
    excluded: &BTreeSet<(i32, i32)>,
) -> Option<Vec<MapLink>> {
    if from == to {
        return Some(vec![]);
    }
    let mut queue = VecDeque::from([from]);
    let mut previous = BTreeMap::<i32, MapLink>::new();
    let mut seen = BTreeSet::from([from]);
    while let Some(map) = queue.pop_front() {
        for link in system
            .links
            .iter()
            .filter(|l| l.from == map && !excluded.contains(&(l.from, l.portal_id)))
        {
            let Some(target) = system.map(link.to) else {
                continue;
            };
            if level.is_some_and(|level| level < target.min_level) || !seen.insert(link.to) {
                continue;
            }
            previous.insert(link.to, link.clone());
            if link.to == to {
                let mut route = vec![];
                let mut current = to;
                while current != from {
                    let edge = previous.get(&current)?.clone();
                    current = edge.from;
                    route.push(edge);
                }
                route.reverse();
                return Some(route);
            }
            queue.push_back(link.to);
        }
    }
    None
}

fn static_star_system(snapshot: &GameSnapshot, current_map: i32) -> StarSystem {
    let maps = xenomit_game_api::star_map::MAPS
        .iter()
        .map(|(id, name, _)| MapDescriptor {
            id: *id,
            name: (*name).into(),
            min_level: 0,
        })
        .collect();
    let links = xenomit_game_api::star_map::PORTALS
        .iter()
        .enumerate()
        .map(|(index, (from, x, y, to))| {
            let portal_id = if *from == current_map {
                let from_world = |portal: &PortalInfo| {
                    let distance = (portal.position.x - f64::from(*x))
                        .hypot(portal.position.y - f64::from(*y));
                    (distance <= 500.0 && portal.target_map.is_none_or(|target| target == *to))
                        .then_some((portal.id, distance))
                };
                snapshot
                    .navigation
                    .portals
                    .iter()
                    .filter_map(from_world)
                    .min_by(|left, right| left.1.total_cmp(&right.1))
                    .map(|(id, _)| id)
                    .or_else(|| {
                        snapshot
                            .entities
                            .iter()
                            .filter(|entity| entity.category == EntityCategory::Portal)
                            .filter_map(|entity| {
                                let (px, py) = entity_world_position(entity)?;
                                let distance = (px - f64::from(*x)).hypot(py - f64::from(*y));
                                (distance <= 500.0
                                    && xenomit_game_api::star_map::portal_target(
                                        *from,
                                        px as f32,
                                        py as f32,
                                        field_i32(entity, "typeID").unwrap_or(0),
                                    ) == Some(*to))
                                .then_some((entity_id(entity)?, distance))
                            })
                            .min_by(|left, right| left.1.total_cmp(&right.1))
                            .map(|(id, _)| id)
                    })
                    .unwrap_or(-i32::try_from(index + 1).unwrap_or(i32::MAX))
            } else {
                -i32::try_from(index + 1).unwrap_or(i32::MAX)
            };
            MapLink {
                from: *from,
                to: *to,
                portal_id,
            }
        })
        .collect();
    StarSystem { maps, links }
}

fn current_map_portals(snapshot: &GameSnapshot, map: i32) -> Vec<PortalInfo> {
    let mut portals = snapshot.navigation.portals.clone();
    for portal in &mut portals {
        if portal.target_map.is_none() {
            portal.target_map = xenomit_game_api::star_map::portal_target(
                map,
                portal.position.x as f32,
                portal.position.y as f32,
                portal.type_id,
            );
        }
    }
    for entity in snapshot
        .entities
        .iter()
        .filter(|entity| entity.category == EntityCategory::Portal)
    {
        let Some(id) = entity_id(entity) else {
            continue;
        };
        let type_id = field_i32(entity, "typeID").unwrap_or(0);
        let target = entity_world_position(entity).and_then(|(x, y)| {
            xenomit_game_api::star_map::portal_target(map, x as f32, y as f32, type_id)
        });
        if let Some(portal) = portals.iter_mut().find(|portal| portal.id == id) {
            if portal.target_map.is_none() {
                portal.target_map = target;
            }
            continue;
        }
        portals.push(PortalInfo {
            id,
            position: entity.position,
            type_id,
            target_map: target,
            jumping: entity
                .fields
                .get("jumping")
                .or_else(|| entity.fields.get("isJumping"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        });
    }
    portals
}

fn entity_id(entity: &GameEntity) -> Option<i32> {
    entity.id.as_ref().and_then(|id| {
        id.as_i64()
            .and_then(|id| i32::try_from(id).ok())
            .or_else(|| id.as_u64().and_then(|id| i32::try_from(id).ok()))
            .or_else(|| id.as_str().and_then(|id| id.parse().ok()))
    })
}

fn field_i32(entity: &GameEntity, key: &str) -> Option<i32> {
    entity.fields.get(key).and_then(|value| {
        value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
    })
}

fn entity_world_position(entity: &GameEntity) -> Option<(f64, f64)> {
    let number = |key: &str| {
        entity.fields.get(key).and_then(|value| {
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
        })
    };
    Some((number("worldX")?, number("worldY")?))
}
#[derive(Clone, Debug, PartialEq)]
pub enum TravelState {
    Traveling,
    Arrived,
    Failed(String),
}
#[derive(Clone, Debug)]
pub struct TravelDecision {
    pub state: TravelState,
    pub status: String,
    pub destination: Option<Point>,
    pub command: Option<GameCommand>,
    pub disable_pet: bool,
}
impl TravelDecision {
    fn waiting(status: impl Into<String>) -> Self {
        Self {
            state: TravelState::Traveling,
            status: status.into(),
            destination: None,
            command: None,
            disable_pet: false,
        }
    }
    fn failed(error: impl Into<String>) -> Self {
        let error = error.into();
        Self {
            state: TravelState::Failed(error.clone()),
            ..Self::waiting(error)
        }
    }
}
#[derive(Default)]
pub struct MapTraveler {
    target: Option<i32>,
    map: Option<i32>,
    entered_at: u64,
    settled_at: u64,
    portal: Option<i32>,
    attempt_started: Option<u64>,
    last_jump: Option<u64>,
    attempts: u32,
    excluded: BTreeSet<(i32, i32)>,
    failure: Option<String>,
}
impl MapTraveler {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn command_failed(&mut self, error: String) {
        if self.attempts >= 12 {
            self.failure = Some(format!("Portal jump failed: {error}"));
        }
    }
    /// A portal with no walkable approach cannot be used by this route.
    pub fn route_failed(&mut self) {
        if let (Some(map), Some(portal)) = (self.map, self.portal.take()) {
            self.excluded.insert((map, portal));
        }
        self.attempt_started = None;
        self.last_jump = None;
        self.attempts = 0;
    }
    pub fn tick(&mut self, snapshot: &GameSnapshot, target: i32, now_ms: u64) -> TravelDecision {
        if self.target != Some(target) {
            self.reset();
            self.target = Some(target);
        }
        if let Some(error) = &self.failure {
            return TravelDecision::failed(error.clone());
        }
        let Some(map) = snapshot
            .map
            .as_ref()
            .and_then(|m| m.id)
            .filter(|map| *map != -1)
            .or_else(|| snapshot.navigation.map_id.filter(|map| *map != -1))
        else {
            return TravelDecision::waiting("Waiting for the map to load");
        };
        if self.map != Some(map) {
            self.map = Some(map);
            self.entered_at = now_ms;
            self.settled_at = now_ms.saturating_add(2_000);
            self.portal = None;
            self.attempt_started = None;
            self.last_jump = None;
            self.attempts = 0;
        }
        if now_ms < self.settled_at {
            return TravelDecision::waiting("Waiting for teleport/map loading to settle");
        }
        if map == target {
            return TravelDecision {
                state: TravelState::Arrived,
                ..TravelDecision::waiting("Arrived at the working map")
            };
        }
        let nav = &snapshot.navigation;
        if nav
            .map_id
            .is_some_and(|navigation_map| navigation_map != map)
            || (nav.map_id == Some(map) && !nav.geometry_ready)
        {
            return TravelDecision::waiting("Waiting for current-map navigation data");
        }
        if self
            .attempt_started
            .is_some_and(|at| now_ms.saturating_sub(at) >= 90_000)
            || self.attempts >= 12
        {
            if let Some(portal) = self.portal.take() {
                self.excluded.insert((map, portal));
            }
            self.attempt_started = None;
            self.last_jump = None;
            self.attempts = 0;
        }
        // The bundled static catalog substitutes for *missing* live map
        // metadata only (backends that never ship `star_system`). Once the
        // live catalog is loaded it is authoritative: an exhausted route
        // is terminal — synthesizing links it does not contain would send
        // the bot hunting portals on the wrong map forever.
        let route = shortest_map_route(
            &nav.star_system,
            map,
            target,
            nav.hero.level,
            &self.excluded,
        )
        .or_else(|| {
            if nav.star_system.links.is_empty() {
                shortest_map_route(
                    &static_star_system(snapshot, map),
                    map,
                    target,
                    nav.hero.level,
                    &self.excluded,
                )
            } else {
                None
            }
        });
        let Some(route) = route else {
            let error = format!("No accessible portal route from map {map} to {target}");
            self.failure = Some(error.clone());
            return TravelDecision::failed(error);
        };
        let edge = &route[0];
        let portals = current_map_portals(snapshot, map);
        let portal = portals
            .iter()
            .filter(|portal| {
                portal.target_map == Some(edge.to) && !self.excluded.contains(&(map, portal.id))
            })
            .min_by_key(|portal| {
                (
                    usize::from(edge.portal_id > 0 && portal.id != edge.portal_id),
                    portal.id,
                )
            });
        let Some(portal) = portal else {
            if now_ms.saturating_sub(self.entered_at) > 15_000 {
                self.excluded.insert((map, edge.portal_id));
                return TravelDecision::waiting(
                    "Portal unavailable; recalculating an alternative route",
                );
            }
            return TravelDecision::waiting("Waiting for route portals to appear");
        };
        if self.portal != Some(portal.id) {
            self.portal = Some(portal.id);
            self.attempt_started = None;
            self.last_jump = None;
            self.attempts = 0;
        }
        let Some(player) = snapshot
            .map
            .as_ref()
            .and_then(|m| m.player_position)
            .or_else(|| {
                snapshot
                    .entities
                    .iter()
                    .find(|e| e.category == xenomit_game_api::EntityCategory::Player)
                    .map(|e| e.position)
            })
        else {
            return TravelDecision::waiting("Waiting for hero position");
        };
        let distance = (player.x - portal.position.x).hypot(player.y - portal.position.y);
        let mut decision = TravelDecision::waiting(format!(
            "Traveling {map} → {} → {target} ({} jumps remaining)",
            edge.to,
            route.len()
        ));
        decision.disable_pet = distance > 1500.0;
        if portal.jumping {
            return TravelDecision::waiting("Portal jump is in progress");
        }
        let point = Point {
            x: portal.position.x,
            y: portal.position.y,
        };
        let approach = if nav.map_id == Some(map) && nav.geometry_ready {
            let navigation = crate::ObstacleNavigation::from_snapshot(
                snapshot,
                crate::CoordinateTransform::default(),
            );
            let Some(approach) = navigation.nearest_point(point) else {
                self.route_failed();
                return TravelDecision::waiting("Portal is obstructed; recalculating route");
            };
            if (approach.x - point.x).hypot(approach.y - point.y) > 550.0
                || navigation
                    .find_path(
                        Point {
                            x: player.x,
                            y: player.y,
                        },
                        approach,
                    )
                    .is_err()
            {
                self.route_failed();
                return TravelDecision::waiting(
                    "Portal has no reachable approach; recalculating route",
                );
            }
            approach
        } else {
            // Legacy and packet snapshots may not include NOA geometry. Their
            // live portal entities still provide a usable world position, so
            // let the game client route directly to its portal.
            point
        };
        let leniency = (200.0 + (approach.x - point.x).hypot(approach.y - point.y)).min(600.0);
        if distance > leniency {
            decision.destination = Some(approach);
        } else if self
            .last_jump
            .is_none_or(|last| now_ms.saturating_sub(last) >= 2_500)
        {
            self.attempt_started.get_or_insert(now_ms);
            self.last_jump = Some(now_ms);
            self.attempts += 1;
            decision.command = Some(GameCommand::JumpPortal {
                portal_id: portal.id,
            });
            decision.status = format!(
                "Jumping portal {} to map {} (attempt {})",
                portal.id, edge.to, self.attempts
            );
        }
        decision
    }
}

/// Retained travel state for checkpoint export. Stamps become ages relative
/// to `now_ms`; the excluded-portal set and attempt count carry over so a
/// successor resumes the same route instead of re-walking failed portals.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(default)]
pub struct TravelerCheckpoint {
    /// Destination map the traveler was routing toward.
    pub target: Option<i32>,
    /// Last observed map.
    pub map: Option<i32>,
    pub entered_age_ms: Option<u64>,
    pub settled_age_ms: Option<u64>,
    /// Portal currently being approached or jumped.
    pub portal: Option<i32>,
    pub attempt_started_age_ms: Option<u64>,
    pub last_jump_age_ms: Option<u64>,
    /// Portal jump attempts for the current target.
    pub attempts: u32,
    /// (from, portal) pairs that already failed for this route.
    pub excluded: Vec<(i32, i32)>,
    pub failure: Option<String>,
}

impl MapTraveler {
    /// Export travel state relative to `now_ms`.
    pub fn export_checkpoint(&self, now_ms: u64) -> TravelerCheckpoint {
        TravelerCheckpoint {
            target: self.target,
            map: self.map,
            entered_age_ms: crate::checkpoint::age_ms(Some(self.entered_at), now_ms),
            settled_age_ms: crate::checkpoint::age_ms(Some(self.settled_at), now_ms),
            portal: self.portal,
            attempt_started_age_ms: crate::checkpoint::age_ms(self.attempt_started, now_ms),
            last_jump_age_ms: crate::checkpoint::age_ms(self.last_jump, now_ms),
            attempts: self.attempts,
            excluded: self.excluded.iter().copied().collect(),
            failure: self.failure.clone(),
        }
    }

    /// Restore travel state exported by [`MapTraveler::export_checkpoint`].
    /// Restoring mid-travel resumes the route at its next decision tick —
    /// the in-flight jump command is never replayed.
    pub fn restore_checkpoint(
        &mut self,
        checkpoint: &TravelerCheckpoint,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        self.target = checkpoint.target;
        self.map = checkpoint.map;
        self.entered_at = crate::checkpoint::rebase_age(now_ms, checkpoint.entered_age_ms)
            .unwrap_or(0);
        self.settled_at = crate::checkpoint::rebase_age(now_ms, checkpoint.settled_age_ms)
            .unwrap_or(0);
        self.portal = checkpoint.portal;
        self.attempt_started =
            crate::checkpoint::rebase_age(now_ms, checkpoint.attempt_started_age_ms);
        self.last_jump = crate::checkpoint::rebase_age(now_ms, checkpoint.last_jump_age_ms);
        self.attempts = checkpoint.attempts;
        self.excluded = checkpoint.excluded.iter().copied().collect();
        self.failure = checkpoint.failure.clone();
        Ok(())
    }
}
