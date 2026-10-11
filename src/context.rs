//! Optional manager observations absent from basic bridge frames. All points,
//! speeds and distances here use the engine's transformed coordinate units.
use std::collections::HashMap;

use serde_json::Value;
use xenomit_game_api::{GameEntity, GameSnapshot};

use crate::{CoordinateTransform, Point};

/// Navigation services used by the original MovementAPI. Implement this for
/// obstacle geometry/pathfinding; the default only knows map bounds.
pub trait Navigation {
    fn can_move(&self, point: Point) -> bool {
        point.x.is_finite() && point.y.is_finite()
    }
    /// Distance from the entity to the closest reachable point, NOT distance
    /// from the hero. Unknown obstacle information defaults to reachable.
    fn closest_distance(&self, _entity: &GameEntity, _point: Point) -> f64 {
        0.0
    }
    fn in_preferred_zone(&self, _point: Point) -> bool {
        true
    }
}

/// Facts supplied by entities/attack/stats APIs. Unknown facts stay `None`;
/// absence of ownership/HP data is not interpreted as an invalid target.
#[derive(Clone, Debug, Default)]
pub struct EntityObservation {
    pub valid: Option<bool>,
    pub collected: bool,
    pub collectable: Option<bool>,
    pub owned: Option<bool>,
    pub blacklisted: bool,
    pub attacked_by_others: bool,
    pub attacking_hero: bool,
    pub enemy: bool,
    pub invisible: bool,
    pub bugged: bool,
    pub moving: Option<bool>,
    pub shield_increased: bool,
    pub speed: Option<f64>,
    pub destination: Option<Point>,
    pub closest_distance: Option<f64>,
    pub preferred: Option<bool>,
    /// AttackAPI's modified radius, e.g. ability-based adjustments.
    pub radius: Option<f64>,
    /// Attempts already made before this planning step (Box.getRetries).
    pub collection_retries: Option<u64>,
}

/// The module's dependencies, separated from raw frame extraction. Adapters
/// may fill these directly or use the documented optional raw field names.
#[derive(Clone, Debug)]
pub struct ModuleContext {
    /// Key is EntityRef::key(): `id:<JSON id>` or `path:<display path>`.
    pub entities: HashMap<String, EntityObservation>,
    pub hero_speed: Option<f64>,
    pub destination: Option<Point>,
    pub hero_attacking: Option<String>,
    pub hero_local_target: Option<String>,
    pub hero_locked_target: Option<String>,
    pub energy_leech: bool,
    pub casting_ability: bool,
    pub cargo: Option<f64>,
    pub max_cargo: Option<f64>,
    pub invisible: bool,
    /// Result of SafetyFinder.tick(). False suspends ordinary module work.
    pub safe_to_work: bool,
    pub safety_destination: Option<Point>,
    pub safety_status: Option<String>,
    pub safety_waiting: bool,
}

impl Default for ModuleContext {
    fn default() -> Self {
        Self {
            entities: HashMap::new(),
            hero_speed: None,
            destination: None,
            hero_attacking: None,
            hero_local_target: None,
            hero_locked_target: None,
            energy_leech: false,
            casting_ability: false,
            cargo: None,
            max_cargo: None,
            invisible: false,
            safe_to_work: true,
            safety_destination: None,
            safety_status: None,
            safety_waiting: false,
        }
    }
}

impl ModuleContext {
    pub fn from_snapshot(
        snapshot: &GameSnapshot,
        hero: &GameEntity,
        transform: CoordinateTransform,
    ) -> Self {
        let mut context = Self::default();
        for entity in &snapshot.entities {
            context.entities.insert(
                entity_key(entity),
                EntityObservation {
                    valid: boolean(entity, "isValid"),
                    collected: flag(entity, "isCollected"),
                    collectable: boolean(entity, "isCollectable"),
                    owned: boolean(entity, "isOwned"),
                    blacklisted: flag(entity, "isBlacklisted"),
                    attacked_by_others: flag(entity, "attackedByOthers"),
                    attacking_hero: flag(entity, "attackingHero"),
                    enemy: flag(entity, "isEnemy"),
                    invisible: flag(entity, "isInvisible"),
                    bugged: flag(entity, "isBugged"),
                    moving: boolean(entity, "isMoving"),
                    shield_increased: flag(entity, "shieldIncreased"),
                    speed: number(entity, "speed"),
                    destination: destination(entity, transform),
                    closest_distance: number(entity, "closestDistance"),
                    preferred: boolean(entity, "inPreferredZone"),
                    radius: number(entity, "modifiedRadius"),
                    collection_retries: entity
                        .fields
                        .get("collectionRetries")
                        .and_then(Value::as_u64),
                },
            );
        }
        context.hero_speed = number(hero, "speed");
        context.destination = destination(hero, transform);
        context.hero_attacking = id_key(hero, "attackingTargetId");
        context.hero_local_target = id_key(hero, "localTargetId");
        context.hero_locked_target = id_key(hero, "lockedTargetId");
        context.energy_leech = flag(hero, "energyLeech");
        context.casting_ability = flag(hero, "castingAbility");
        context.cargo = number(hero, "cargo");
        context.max_cargo = number(hero, "maxCargo");
        context.invisible = flag(hero, "isInvisible");
        context
    }

    pub(crate) fn observation(&self, entity: &GameEntity) -> EntityObservation {
        self.entities
            .get(&entity_key(entity))
            .cloned()
            .unwrap_or_default()
    }
}

pub(crate) fn entity_key(entity: &GameEntity) -> String {
    entity
        .id
        .as_ref()
        .map(|id| format!("id:{id}"))
        .unwrap_or_else(|| format!("path:{}", entity.path))
}

fn boolean(entity: &GameEntity, field: &str) -> Option<bool> {
    entity.fields.get(field).and_then(Value::as_bool)
}
fn flag(entity: &GameEntity, field: &str) -> bool {
    boolean(entity, field).unwrap_or(false)
}
fn number(entity: &GameEntity, field: &str) -> Option<f64> {
    entity
        .fields
        .get(field)
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|v| v.is_finite() && *v >= 0.0)
}
fn destination(entity: &GameEntity, transform: CoordinateTransform) -> Option<Point> {
    let x = entity.fields.get("destinationX")?.as_f64()?;
    let y = entity.fields.get("destinationY")?.as_f64()?;
    let (x, y) = transform.apply(x, y);
    (x.is_finite() && y.is_finite()).then_some(Point { x, y })
}
fn id_key(entity: &GameEntity, field: &str) -> Option<String> {
    entity
        .fields
        .get(field)
        .filter(|v| !v.is_null())
        .map(|id| format!("id:{id}"))
}
