//! Neutral NPC and loot-box presets translated from the supplied add-on data.
use crate::{BoxRule, EntitySelector, NpcFlag, NpcRule};
use serde::Deserialize;
use serde_json::json;
use std::{collections::BTreeMap, sync::OnceLock};
use xenomit_game_api::{EntityCategory, SettingChoice};

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct NpcPresetEntry {
    pub name: String,
    pub radius: f64,
    pub priority: i32,
    pub map_ids: Vec<i32>,
    pub flags: Vec<NpcFlag>,
}

impl Default for NpcPresetEntry {
    fn default() -> Self {
        Self {
            name: String::new(),
            radius: 560.0,
            priority: 100,
            map_ids: Vec::new(),
            flags: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoxPresetEntry {
    pub name: &'static str,
    pub wait_ms: u64,
    pub priority: i32,
}

static NPC_PRESETS: OnceLock<BTreeMap<String, Vec<NpcPresetEntry>>> = OnceLock::new();

fn npc_presets() -> &'static BTreeMap<String, Vec<NpcPresetEntry>> {
    NPC_PRESETS.get_or_init(|| {
        serde_json::from_str(include_str!("npc_presets.json"))
            .expect("bundled NPC preset data is valid")
    })
}

pub fn npc_preset_choices() -> Vec<SettingChoice> {
    [
        ("None", "none"),
        ("MMO maps", "mmo"),
        ("EIC maps", "eic"),
        ("VRU maps", "vru"),
        ("Pirate maps", "pirates"),
        ("Uber map", "uber"),
        ("Blacklight maps", "blacklight"),
        ("Special NPCs", "special"),
    ]
    .into_iter()
    .map(|(label, value)| SettingChoice {
        label: label.into(),
        value: value.into(),
    })
    .collect()
}

pub fn box_preset_choices() -> Vec<SettingChoice> {
    [
        ("All box types", "all"),
        ("Ore boxes", "ore"),
        ("Bonus boxes", "bonus"),
        ("Booty boxes", "booty"),
        ("Event boxes", "event"),
    ]
    .into_iter()
    .map(|(label, value)| SettingChoice {
        label: label.into(),
        value: value.into(),
    })
    .collect()
}

pub fn npc_preset_rules(id: &str) -> Option<Vec<NpcRule>> {
    let rows = npc_presets().get(&id.to_ascii_lowercase())?;
    Some(
        rows.iter()
            .map(|row| NpcRule {
                selector: EntitySelector {
                    category: Some(EntityCategory::Npc),
                    name: Some(row.name.clone()),
                    ..EntitySelector::default()
                },
                priority: row.priority,
                combat_radius: row.radius,
                map_ids: row.map_ids.clone(),
                flags: row.flags.clone(),
                ..NpcRule::default()
            })
            .collect(),
    )
}

pub fn box_preset_entries(id: &str) -> Option<Vec<BoxPresetEntry>> {
    let rows: &[(&str, u64, i32)] = match id.to_ascii_lowercase().as_str() {
        "ore" => &[
            ("SCRAPIUM", 0, 0),
            ("BOLTRUM", 0, 0),
            ("MUCOSUM", 0, 0),
            ("PRISMATIUM", 0, 0),
            ("ore_0", 0, 0),
            ("ore_1", 0, 0),
            ("ore_2", 0, 0),
            ("ore_8", 750, 0),
        ],
        "bonus" => &[
            ("BONUS_BOX", 0, 0),
            ("GIFT_BOXES", 0, 0),
            ("CANDY_CARGO", 0, 0),
            ("MINI_PUMPKIN", 0, 0),
            ("ALIEN_EGG", 0, 0),
            ("BIRTHDAY", 0, 0),
            ("FLOWER", 0, 0),
        ],
        "booty" => &[
            ("PIRATE_BOOTY", 5500, 0),
            ("PIRATE_BOOTY_RED", 5500, 0),
            ("PIRATE_BOOTY_GOLD", 5500, 0),
            ("PIRATE_BOOTY_BLUE", 5500, 0),
            ("POISON_PUSAT_BOX_BLACK", 5500, 0),
            ("MASQUE_BOOTY_BOX", 5500, 0),
            ("BLACK_BOOTY_BOX", 5500, 0),
        ],
        "event" => &[
            ("CHROMIN_BOX_1", 0, -10),
            ("CHROMIN_BOX_2", 0, -10),
            ("CHROMIN_BOX_3", 0, -10),
            ("TREASURE", 100, 0),
            ("STYXUS_BOX", 750, 0),
            ("AGATUS_BOX", 0, -100),
            ("FROZEN_LABYRINTH_BOX", 0, 0),
        ],
        _ => return None,
    };
    Some(
        rows.iter()
            .map(|(name, wait_ms, priority)| BoxPresetEntry {
                name,
                wait_ms: *wait_ms,
                priority: *priority,
            })
            .collect(),
    )
}

pub fn box_preset_rules(id: &str) -> Option<Vec<BoxRule>> {
    let rows = box_preset_entries(id)?;
    Some(
        rows.into_iter()
            .flat_map(|row| {
                let fields = BTreeMap::from([("entityKind".into(), json!("box"))]);
                [
                    EntitySelector {
                        name_contains: Some(row.name.into()),
                        fields: fields.clone(),
                        ..EntitySelector::default()
                    },
                    EntitySelector {
                        class_name_contains: Some(row.name.into()),
                        fields,
                        ..EntitySelector::default()
                    },
                ]
                .into_iter()
                .map(move |selector| BoxRule {
                    selector,
                    priority: row.priority,
                    wait_ms: row.wait_ms,
                    ..BoxRule::default()
                })
            })
            .collect(),
    )
}
