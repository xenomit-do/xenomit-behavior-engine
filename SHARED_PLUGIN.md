# SharedPlugin coverage

`shared::register_shared_behaviors` registers snapshot-backed SharedPlugin
behaviors with `shared_` IDs. The current port includes simple ship healing
(Solace/Aegis ability commands from observed inventory and health ratios),
crowd avoidance (NPC/player selection, exclusion of known group/clan allies,
safe-place checks, and a takeover movement plan), Solaris/Paladin ability use
for observed NPC counts, PET repair through the observed gear list and
`SelectPetGear`, and stuck PET refresh recovery. Every rule checks its own
`enabled` and `tick_stopped` settings.

`shared::register_shared_modules` registers the `shared_spaceball` objective,
also included in the event plugin catalog. It travels to
4-4, recognizes the SpaceBall NPC and its cargo names, supports attack/follow
modes, and prioritizes cargo collection for a configured delay after the ball disappears. Start and stop
the event manually; goal/cargo phase is inferred from snapshots because the
client API does not expose explicit score events.

The engine already registers its `galaxy` gate objective, which uses the same
typed spin/place/life/jump requests and planner lifecycle as the SharedPlugin
gate builder. SharedPlugin's broader per-gate NPC flags, hangar/profile
switching, server-time scheduling, and full set of gate-specific wave scripts
are not equivalent and remain unsupported.

PET repair and stuck recovery are partial ports. Repair waits for PET health,
ownership, activity, and gear IDs in `navigation.hero`; stuck recovery uses the
active PET bit and observed hero combat/collection state, then asks the host to
refresh. Missing facts suppress the action. The port does not model DarkBot's
restricted gear cooldown state or PET-window diagnostics.

Other unsupported SharedPlugin features include ore selling (inventory and
transactional Backpage results), fast travel and scheduled gates (their event
and module lifecycle orchestration), and autobuy (purchase endpoints with
confirmed results). Basic typed commands exist for selected gate-builder
actions, but do not provide every module loop.

Healing relies on `health`, `max_health`, `shield`, and `max_shield` plus
`navigation.hero.items`. Crowd avoidance relies on player/NPC fields such as
`isEnemy`, `clanId`, and `isCollecting`, along with map safe places. Solaris
counting uses entity positions and, when requested, a `hasSolarisAbility` flag.
PET repair requires an entity with class `Pet` and `isOwnPet`, `ownerId`, or
`ownerID` identifying the hero, plus gear ID 9. Stuck recovery requires PET
ownership, an explicit enabled setting from the active module or captured
`petEnabled`, and combat/collection state; a pending refresh waits until combat
ends. Kamikaze uses gear ID 10. Missing observations conservatively disable
the affected action. Solaris sticky movement is approximated by its
configured duration; the source's custom NPC extra-flag provider is unavailable
unless the adapter exposes `hasSolarisAbility` on each NPC.
