use serde_json::Value;

const SKYLAB_MODULES: &[(&str, i32)] = &[
    ("baseModule", 0),
    ("solarModule", 1),
    ("storageModule", 1),
    ("prometiumCollector", 1),
    ("enduriumCollector", 1),
    ("terbiumCollector", 1),
    ("prometidRefinery", 2),
    ("duraniumRefinery", 2),
    ("promeriumRefinery", 3),
    ("xenoModule", 3),
    ("sepromRefinery", 4),
];

/// One skylab page, in the shape the builder task acts on.
#[derive(Clone, Debug, PartialEq)]
pub struct SkylabRow {
    pub name: String,
    pub level: i32,
    pub upgrade_path: Option<String>,
    pub pending_robots: i32,
    pub level_diff: i32,
}

pub fn skylab_rows(page: &str) -> Vec<SkylabRow> {
    let upgrade = regex_lite_upgrade(page);
    let levels = regex_lite_levels(page);
    let robots = regex_lite_robots(page);
    SKYLAB_MODULES
        .iter()
        .map(|(name, diff)| SkylabRow {
            name: (*name).into(),
            level: levels
                .iter()
                .find(|(module, _)| module == name)
                .map(|(_, level)| *level)
                .unwrap_or(0),
            upgrade_path: upgrade
                .iter()
                .find(|(module, _)| module == name)
                .map(|(_, path)| path.clone()),
            pending_robots: robots
                .iter()
                .find(|(module, _)| module == name)
                .map(|(_, count)| *count)
                .unwrap_or(0),
            level_diff: *diff,
        })
        .collect()
}

fn regex_lite_upgrade(page: &str) -> Vec<(String, String)> {
    capture_pairs(page, "href=\"", "subaction=upgrade", "construction=")
}
fn regex_lite_levels(page: &str) -> Vec<(String, i32)> {
    let mut rows = vec![];
    for (index, _) in page.match_indices("skylab_font_level") {
        let before = &page[..index];
        let Some(name_at) = before.rfind('\'') else {
            continue;
        };
        let name = before[..name_at].rsplit('\'').next().unwrap_or("").trim();
        let after = &page[index..];
        let Some(start) = after.find('>') else {
            continue;
        };
        let digits: String = after[start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(level) = digits.parse()
            && !name.is_empty()
        {
            rows.push((name.into(), level));
        }
    }
    rows
}
fn regex_lite_robots(page: &str) -> Vec<(String, i32)> {
    let mut rows = vec![];
    for (index, _) in page.match_indices("_skylabPendingRobots") {
        let name = page[..index]
            .rsplit(|c: char| !c.is_ascii_alphanumeric())
            .next()
            .unwrap_or("");
        let after = &page[index..];
        let Some(start) = after.find('>') else {
            continue;
        };
        let digits: String = after[start + 1..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(count) = digits.parse()
            && !name.is_empty()
        {
            rows.push((name.into(), count));
        }
    }
    rows
}
fn capture_pairs(
    page: &str,
    href: &str,
    marker: &str,
    construction: &str,
) -> Vec<(String, String)> {
    let mut rows = vec![];
    let mut rest = page;
    while let Some(start) = rest.find(href) {
        rest = &rest[start + href.len()..];
        let Some(end) = rest.find('"') else { break };
        let path = &rest[..end];
        rest = &rest[end..];
        if !path.contains(marker) {
            continue;
        }
        let Some(name_at) = path.find(construction) else {
            continue;
        };
        let name = path[name_at + construction.len()..]
            .split(['&', '"'])
            .next()
            .unwrap_or("");
        if !name.is_empty() {
            rows.push((name.into(), path.replace("amp;", "")));
        }
    }
    rows
}

/// Choose the next skylab action. Collectors with no pending robots are
/// restocked before the lowest level-diff upgrade.
pub fn next_skylab_action(
    rows: &[SkylabRow],
    enabled: &Value,
    buy_uri: bool,
) -> Option<SkylabAction> {
    let enabled = |name: &str| enabled.get(name).and_then(Value::as_bool).unwrap_or(false);
    if let Some(row) = rows
        .iter()
        .find(|row| row.name.contains("Collector") && enabled(&row.name) && row.pending_robots == 0)
    {
        return Some(SkylabAction::BuyRobots {
            construction: row.name.clone(),
            uri: buy_uri,
        });
    }
    let floor = rows
        .iter()
        .filter(|row| row.level < 20 && enabled(&row.name))
        .map(|row| row.level_diff)
        .min()?;
    rows.iter()
        .filter(|row| {
            enabled(&row.name)
                && row.level < 20
                && row.level_diff == floor
                && row.upgrade_path.is_some()
        })
        .min_by_key(|row| row.level)
        .map(|row| SkylabAction::Upgrade {
            path: row.upgrade_path.clone().unwrap_or_default(),
        })
}

#[derive(Clone, Debug, PartialEq)]
pub enum SkylabAction {
    BuyRobots { construction: String, uri: bool },
    Upgrade { path: String },
}
