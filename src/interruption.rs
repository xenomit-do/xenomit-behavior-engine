//! Shared conservative checks for host-requested module interruptions.
use crate::{InterruptionContext, InterruptionStatus};
use xenomit_game_api::{EntityCategory, GameSnapshot};

/// A common safe point for ordinary objectives: the bridge is ready, the hero
/// is alive, no gate workflow is active, and no live combat/collection target
/// is targeted. Individual modules can apply stricter state checks first.
pub fn safe_idle_status(context: &InterruptionContext<'_>) -> InterruptionStatus {
    let Some(snapshot) = context.snapshot.filter(|_| context.game_ready) else {
        return InterruptionStatus::Deferred("Waiting for a ready game snapshot".into());
    };
    if snapshot.navigation.hero.killed {
        return InterruptionStatus::Deferred("Waiting for hero recovery".into());
    }
    let Some(hero) = snapshot
        .entities
        .iter()
        .find(|entity| entity.category == EntityCategory::Player)
    else {
        return InterruptionStatus::Deferred("Waiting for the hero snapshot".into());
    };
    if hero.health.is_some_and(|health| health <= 0.0)
        || ["isCollecting", "isBootyCollecting", "isCastingAbility"]
            .iter()
            .any(|key| hero.field(key).and_then(serde_json::Value::as_bool) == Some(true))
        || hero
            .field("effects")
            .is_some_and(contains_collection_effect)
    {
        return InterruptionStatus::Deferred(
            "Waiting for the current game action to finish".into(),
        );
    }
    if snapshot
        .navigation
        .map_id
        .or_else(|| snapshot.map.as_ref().and_then(|map| map.id))
        .is_none()
    {
        return InterruptionStatus::Deferred("Waiting for the current map to be identified".into());
    }
    if is_gate_map(snapshot) {
        return InterruptionStatus::Deferred(
            "Waiting until the current galaxy-gate run is finished".into(),
        );
    }
    if hero_has_active_target(snapshot) {
        return InterruptionStatus::Deferred(
            "Waiting for the current target activity to finish".into(),
        );
    }
    if snapshot
        .navigation
        .portals
        .iter()
        .any(|portal| portal.jumping)
    {
        return InterruptionStatus::Deferred("Waiting for map travel to finish".into());
    }
    InterruptionStatus::Ready
}

fn contains_collection_effect(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Array(values) => values.iter().any(contains_collection_effect),
        serde_json::Value::Object(fields) => {
            let effect_id = fields
                .get("id")
                .or_else(|| fields.get("_id"))
                .or_else(|| fields.get("effectId"));
            effect_id
                .and_then(serde_json::Value::as_i64)
                .is_some_and(|id| matches!(id, 84 | 85))
                || fields.values().any(contains_collection_effect)
        }
        _ => false,
    }
}

/// Detect gate-family maps from both stable gate IDs and the loaded map
/// catalog. Catalog names cover special/rotating gate maps without relying on
/// a guessed map-number convention.
pub fn is_gate_map(snapshot: &GameSnapshot) -> bool {
    if snapshot
        .entities
        .iter()
        .filter(|entity| entity.category == EntityCategory::Player)
        .any(|hero| {
            ["isGG", "is_gg", "heroIsGG"]
                .iter()
                .any(|key| hero.field(key).and_then(serde_json::Value::as_bool) == Some(true))
        })
    {
        return true;
    }
    let map_id = snapshot
        .navigation
        .map_id
        .or_else(|| snapshot.map.as_ref().and_then(|map| map.id));
    let catalog_name = snapshot
        .navigation
        .star_system
        .maps
        .iter()
        .find(|map| Some(map.id) == map_id)
        .map(|map| map.name.as_str())
        .or_else(|| map_id.and_then(xenomit_game_api::star_map::map_name));
    let name = catalog_name.or_else(|| snapshot.map.as_ref().and_then(|map| map.name.as_deref()));
    name.is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        [
            "gate",
            "gg ",
            "eternal blacklight",
            "ebg",
            "eternal",
            "hades",
            "treacherous domain",
            "group gate",
            "gates of",
            "gop",
            "lords of war",
            "low gate",
        ]
        .iter()
        .any(|marker| name.contains(marker))
    })
}

fn hero_has_active_target(snapshot: &GameSnapshot) -> bool {
    let Some(hero) = snapshot
        .entities
        .iter()
        .find(|entity| entity.category == EntityCategory::Player)
    else {
        return false;
    };
    let target = hero
        .field("localTargetId")
        .or_else(|| hero.field("lockedTargetId"))
        .or_else(|| {
            hero.id
                .as_ref()
                .and_then(serde_json::Value::as_i64)
                .and_then(|id| i32::try_from(id).ok())
                .and_then(|id| snapshot.navigation.entity_fields.get(&id))
                .and_then(|fields| fields.get("localTargetId"))
        });
    let Some(target) = target.filter(|value| !value.is_null() && value.as_i64() != Some(0)) else {
        return false;
    };
    snapshot.entities.iter().any(|entity| {
        matches!(
            entity.category,
            EntityCategory::Npc | EntityCategory::Player | EntityCategory::Ship
        ) && entity.id.as_ref().is_some_and(|id| {
            id == target
                || id
                    .as_i64()
                    .zip(target.as_i64())
                    .is_some_and(|(a, b)| a == b)
        }) && entity.health.is_none_or(|health| health > 0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use xenomit_game_api::{Bounds, GameEntity, MapDescriptor, Point};

    fn snapshot(map_id: i32) -> GameSnapshot {
        let mut snapshot = GameSnapshot {
            navigation: Default::default(),
            observed_at_unix_ms: 0,
            stage: Default::default(),
            map: None,
            entities: vec![GameEntity {
                path: "hero".into(),
                class_name: "Hero".into(),
                name: "hero".into(),
                category: EntityCategory::Player,
                position: Point::default(),
                bounds: Bounds::default(),
                id: Some(serde_json::json!(1)),
                health: Some(100.0),
                max_health: Some(100.0),
                shield: None,
                max_shield: None,
                fields: Default::default(),
            }],
            visited: 0,
            truncated: false,
        };
        snapshot.navigation.map_id = Some(map_id);
        snapshot
    }
    fn context(snapshot: Option<&GameSnapshot>) -> InterruptionContext<'_> {
        InterruptionContext {
            now_ms: 10,
            snapshot,
            game_ready: snapshot.is_some(),
        }
    }

    #[test]
    fn idle_requires_ready_live_snapshot() {
        assert!(matches!(
            safe_idle_status(&context(None)),
            InterruptionStatus::Deferred(_)
        ));
        let idle = snapshot(1);
        assert_eq!(
            safe_idle_status(&context(Some(&idle))),
            InterruptionStatus::Ready
        );
    }
    #[test]
    fn gate_map_id_and_catalog_name_defer_even_without_npcs() {
        let ebg = snapshot(-4);
        assert!(matches!(
            safe_idle_status(&context(Some(&ebg))),
            InterruptionStatus::Deferred(_)
        ));
        let mut named = snapshot(999);
        named.navigation.star_system.maps.push(MapDescriptor {
            id: 999,
            name: "Eternal Blacklight Gate 4".into(),
            min_level: 0,
        });
        assert!(matches!(
            safe_idle_status(&context(Some(&named))),
            InterruptionStatus::Deferred(_)
        ));
    }

    #[test]
    fn an_unselected_npc_does_not_block_an_idle_refresh() {
        let mut idle = snapshot(1);
        idle.entities.push(GameEntity {
            path: "npc".into(),
            class_name: "Npc".into(),
            name: "npc".into(),
            category: EntityCategory::Npc,
            position: Point::default(),
            bounds: Bounds::default(),
            id: Some(serde_json::json!(2)),
            health: Some(100.0),
            max_health: Some(100.0),
            shield: None,
            max_shield: None,
            fields: Default::default(),
        });
        assert_eq!(
            safe_idle_status(&context(Some(&idle))),
            InterruptionStatus::Ready
        );
    }
}
