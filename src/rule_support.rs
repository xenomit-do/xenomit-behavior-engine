//! Canonical ability IDs and priority groups audited against DarkBot's
//! `SupportedAbilities` and `SelectableItem.Ability` definitions.

pub(crate) const HEALTH_WITH_TARGET: &[&str] = &[
    "ability_aegis_hp-repair",
    "ability_hammerclaw-plus_reallocate",
];

pub(crate) const HEALTH_WITHOUT_TARGET: &[&str] = &[
    "ability_aegis_repair-pod",
    "ability_liberator-plus_self-repair",
    "ability_solace",
    "ability_solace-plus_nano-cluster-repairer-plus",
];

pub(crate) const SHIELD: &[&str] = &["ability_aegis_shield-repair", "ability_sentinel"];

pub(crate) const SPEED: &[&str] = &[
    "ability_tartarus_speed-boost",
    "ability_tartarus-plus_speed-boost-plus",
    "ability_citadel_travel",
    "ability_lightning",
    "ability_keres_sle",
    "ability_mimesis_phase-out",
    "ability_zephyr_mmt",
    "ability_pusat-plus_speed-sap",
    "ability_retiarus_spc",
];

pub(crate) const EVADE: &[&str] = &[
    "ability_spearhead_jam-x",
    "ability_spearhead_ultimate-cloak",
    "ability_berserker_rvg",
    "ability_mimesis_scramble",
    "ability_disruptor_ddol",
    "ability_spearhead-plus_neutralizing-marker",
];

pub(crate) const LAST_CHANCE: &[&str] = &[
    "ability_citadel-plus_prismatic-endurance",
    "ability_citadel_fortify",
    "ability_disruptor_redirect",
    "ability_spectrum",
    "ability_spectrum-plus_prismatic-reflecting",
    "ability_sentinel",
    "ability_berserker_bsk",
];

pub(crate) const LAST_CHANCE_ORCUS: &[&str] = &["ability_orcus_assimilate"];

pub(crate) const TANK: &[&str] = &["ability_citadel_draw-fire", "ability_citadel_protection"];

pub(crate) const DAMAGE_RANGE: &[&str] = &[
    "ability_solaris_inc",
    "ability_solaris-plus_incinerate-plus",
    "ability_basilisk_noxious-nebula",
    "ability_tempest_voltage-link",
];

pub(crate) const DAMAGE: &[&str] = &[
    "ability_spearhead_target-marker",
    "ability_diminisher",
    "ability_goliath-x_frozen-claw",
    "ability_venom",
    "ability_tartarus_rapid-fire",
    "ability_disruptor_shield-disarray",
    "ability_hecate_particle-beam",
    "ability_keres_spr",
    "ability_zephyr_tbr",
    "ability_holo_enemy-reversal",
    "ability_tartarus-plus_rapid-fire-plus",
    "ability_basilisk_heightened-valour",
    "ability_retiarus_chs",
    "ability_tempest_volt-discharge",
    "ability_hecate-plus_particle-beam-plus",
    "ability_spearhead-plus_jamx-creed",
    "ability_spearhead-plus_neutralizing-marker",
];

pub(crate) const ALWAYS: &[&str] = &["ability_spearhead_double-minimap"];
pub(crate) const EMERGENCY: &[&str] = &["ability_tempest_volt-backup"];

/// Every configurable ability exposed by DMPlugin's best-ability behavior.
pub(crate) const SUPPORTED: &[&str] = &[
    "ability_aegis_hp-repair",
    "ability_aegis_shield-repair",
    "ability_aegis_repair-pod",
    "ability_basilisk_noxious-nebula",
    "ability_basilisk_heightened-valour",
    "ability_berserker_bsk",
    "ability_berserker_rvg",
    "ability_citadel_draw-fire",
    "ability_citadel_travel",
    "ability_citadel_protection",
    "ability_citadel_fortify",
    "ability_citadel-plus_prismatic-endurance",
    "ability_diminisher",
    "ability_disruptor_ddol",
    "ability_disruptor_shield-disarray",
    "ability_disruptor_redirect",
    "ability_goliath-x_frozen-claw",
    "ability_hammerclaw-plus_reallocate",
    "ability_hecate_particle-beam",
    "ability_hecate-plus_particle-beam-plus",
    "ability_holo_enemy-reversal",
    "ability_keres_spr",
    "ability_keres_sle",
    "ability_liberator-plus_self-repair",
    "ability_lightning",
    "ability_mimesis_phase-out",
    "ability_mimesis_scramble",
    "ability_orcus_assimilate",
    "ability_pusat-plus_speed-sap",
    "ability_retiarus_spc",
    "ability_retiarus_chs",
    "ability_sentinel",
    "ability_solace",
    "ability_solace-plus_nano-cluster-repairer-plus",
    "ability_solaris_inc",
    "ability_solaris-plus_incinerate-plus",
    "ability_spearhead_double-minimap",
    "ability_spearhead_jam-x",
    "ability_spearhead_target-marker",
    "ability_spearhead_ultimate-cloak",
    "ability_spearhead-plus_jamx-creed",
    "ability_spearhead-plus_neutralizing-marker",
    "ability_spectrum",
    "ability_spectrum-plus_prismatic-reflecting",
    "ability_tartarus-plus_rapid-fire-plus",
    "ability_tartarus-plus_speed-boost-plus",
    "ability_tartarus_rapid-fire",
    "ability_tartarus_speed-boost",
    "ability_tempest_voltage-link",
    "ability_tempest_volt-discharge",
    "ability_tempest_volt-backup",
    "ability_venom",
    "ability_zephyr_mmt",
    "ability_zephyr_tbr",
];
