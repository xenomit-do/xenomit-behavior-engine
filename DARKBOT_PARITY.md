# DarkBotAPI investigation and Ruffle integration

Reference: sibling `DarkBotAPI`, commit
`55fae907f9d590b143c455e1fc62437a166bacd5`, licensed GNU LGPL v3.
The Rust port adapts the module algorithms to a snapshot/intent boundary.

## Module behavior

Inspected `shared/src/main/java/eu/darkbot/shared/modules/` and its helpers:

| Reference | Implemented behavior |
| --- | --- |
| CollectorModule | Priority/distance selection; 100/150-unit switching hysteresis; pickup radius and reachability; cargo limits; contested-box travel comparison; retained waits and retry padding; enemy avoidance and cloak requests. |
| LootModule | Priority/HP/distance selection; current-target bonus; preferred-zone/reachability checks; ownership/attacker blacklists; 250ms prediction; corrected combat radius; obstacle-aware forward/backward orbit scoring and bounded spiral search; no-circle movement. |
| LootCollectorModule | Safety first; attacking while collecting; one movement owner; collection radius and low-target-HP limits; IGNORE_BOXES distance constraints; retained collection waits. |
| MapModule / TemporalModule | Wrong-map work suspends immediately. A retained MapTraveler follows directed shortest portal routes, waits for map loading, retries/excludes failed portals, and resumes the same module after arrival. |
| SafetyFinder / PortalJumper | Stateful enemy retreat and repair hysteresis, walkable safe-place selection, repair-item use, offered free base revival, death limits; validated native portal requests and map-transition settling. See differences below. |

The registry exposes four executable modules: `collect`, `npc_killer`,
`kill_collect`, and `palladium`. Custom modules register factories and schemas
before attachment. Discovery never constructs a module. Instances and sequences
survive ticks, game loading and snapshot outages; terminal results are cached.
The desktop rejects changing the running module ID. Settings updates retain
custom instances; built-ins deliberately reset target/travel state on edits.

## Native observations and API

Ruffle captures world coordinates, velocity/destination, HP/shield, identities,
map dimensions, safe places, no-access circle/rectangle/polygon geometry,
inverted zones, portal targets/jumping, and loaded map/link catalogs. The native
capture includes validity/collection state, NPC lock ownership, attackers,
enemy/clan/faction state, invisibility, shield regeneration, cargo, target IDs,
energy-leech effects, NPC type IDs, pet ownership/activity/equipped gears,
configuration, item availability/quantity/selection/cooldown, death state and
eligible free repair offers. Optional observations remain unknown when the
underlying game field is unavailable; active pet gear is not guessed from the
pet-window selection cursor.

`ModuleContext::from_snapshot` recognizes these optional field contracts:

| Scope | Fields |
| --- | --- |
| Entity | isValid, isCollected, isCollectable, isOwned, isBlacklisted, attackedByOthers, attackingHero, isEnemy, isInvisible, isBugged, isMoving, shieldIncreased, speed, destinationX/Y, closestDistance, inPreferredZone, modifiedRadius, collectionRetries |
| Hero | speed, destinationX/Y, attackingTargetId, localTargetId, lockedTargetId, energyLeech, castingAbility, cargo, maxCargo, isInvisible |

Native capture provides the observed manager facts above; application-specific
blacklists, preferred zones and radius adjustments can be supplied by custom
adapters. Built-ins derive a stuck-target timeout and configured ability/radius
policy. Numeric observations must be finite and nonnegative. Unknown ownership
does not blacklist, unknown HP is full HP, and unknown shield is zero. Without
observed speed, hero orbit speed defaults to 300 and NPC speed to zero; pickup
travel time is omitted.

Raw planner points use the configured affine transform. Context scalar speed,
radius and distance values must use the same behavior units. Native capture uses
world units, so custom nonidentity transforms require calibrated observations.
`BehaviorDecision::into_world_coordinates` converts plans for a world-coordinate
executor; built-ins perform this automatically. Travel geometry stays in world
coordinates.

`GameClient::game_command` now has validated Ruffle native bindings for:

- JumpPortal: checks portal identity, jumping state and hero proximity before
  constructing the game's actual JumpRequest.
- SetPet / SelectPetGear: explicit activation/deactivation, ownership and
  equipped-gear checks, locator target selection.
- SelectConfiguration: validates configuration 1/2 and uses the game's request
  path; credentials never leave the client through this API.
- UseItem: selects or activates an equipped, available, unblocked item through
  its actual menu proxy/request.
- Revive: accepts only an offered, affordable, cooldown-free base repair with
  zero cost, then follows the killscreen request/cleanup path.

Built-in policies execute pet, cloak, ship-mode/configuration, repair items,
optional offensive/defensive abilities and laser/rocket selections. NO_SAB
forces available regular ammo, USE_RSB enables throttled available RSB bursts,
PET_LOCATOR selects equipped locator gear for the observed NPC type, and
AGGRESSIVE_FOLLOW approaches distant moving NPCs. Commands are acknowledged
individually for cooldown bookkeeping even if a later movement fails. A command
acknowledgement means request submission, not server-side completion; workflows
must observe authoritative state before advancing when that matters.

## Navigation and desktop settings

ObstacleNavigation uses bounded A*, continuous segment collision checks,
clearance, nearest safe approaches and line-of-sight smoothing. Prepared
geometry and broad-phase bounds avoid repeatedly cloning polygons. A time
budget pauses difficult searches. MovementRouter retains waypoints, follows
corners, and replans after goal/map/geometry changes or lack of progress. Native
missing, malformed, truncated or stale-map geometry fails closed. It applies to
combat, collection approaches, roaming and temporary map travel. The live map
paints the same captured no-access geometry.

Working-map selection is searchable and uses map IDs from the catalog. Legacy
names resolve only through that catalog. Routes minimize portal-jump count,
respect known level restrictions and never infer reverse links. Failed or
unreachable portals are excluded so another route can be attempted. Map
transitions settle before work resumes; no route produces a reported failure
instead of hunting on the wrong map.

The desktop discovers immutable module descriptors off the UI thread during
startup, overlapping game loading. It creates and retains one page, inputs,
selects and subscriptions per module. Catalog updates change choices on the
existing map selector. NPC dictionaries retain user-disabled species. Settings
persist under `modules.<id>` in modules.json, preserving legacy defaults and
whole behavior_config overrides. Validation errors block starting a module.

The native shared-memory layout is version 2; rebuild both client and controller.
Legacy pipe and packet clients do not implement these new native command and
geometry bindings; this implementation targets Ruffle as requested.

## Execution and extension contract

Honor suspended plans before any idle-roaming fallback. Preserve movement and
requested attacks through WaitForCollection. Pickup waits include base delay,
capped retry padding, observed travel time and 30ms. Invalid/removed boxes cancel
waits; recent-attempt isCollected does not. Report plan failure to roll back
pickup waits, and report successful commands individually.

TaskSequence issues each operation once per attempt, waits for its completion,
retains previous named results, supports dependent requests and multi-hour
cooperative delays, and advances only on success. Explicitly idempotent retries
retain input and keys. ThreadedOperationHost bounds work, catches service panics
and propagates cancellation. Long service callbacks must check cancellation
and bound I/O; Rust cannot forcibly kill an uncooperative callback. State is
retained in memory, not a durable remote exactly-once guarantee. See
CUSTOM_MODULES.md and examples/sequenced_event.rs. The example simulates an
injected gate/event service; it is not a live galaxy-gate purchase integration.

## Deliberate differences and remaining limits

- AttackAPI is an interface in the reference repo. Rust ammo/ability policies
  are explicit configurable implementations, not a claim of exact parity with
  a separate Darkbot AttackAPI implementation.
- Safety implements core retreat/repair/revival. It does not implement the full
  SafetyFinder escape-through-portal-and-return policy, player-tag/friendship
  rules, or automatic client refresh/relogin. Unsupported paid repairs are
  rejected. Connection lifecycle remains with the desktop.
- Inverted no-access shapes conservatively restrict movement to the playable
  interior; they do not reproduce the client's outer damage-ring margins.
- Roaming uses the desktop's existing patrol waypoints instead of MovementAPI
  random roaming. No-circle randomness is reproducible. Rust retains explicit
  allowlists and configurable target limits.
- Modules are registered Rust implementations, not hot-loaded binaries. Session
  restart recovery requires a module-owned journal and state reconciliation.

## Validation

Engine tests cover source module decisions, sequencing across loading and hours,
completion/cancellation/retry limits, schema discovery, obstacle routes including
full-size maps, malformed/stale geometry, directed shortest travel, retained
module resumption, repair/death policies, ammo flags and command cooldowns.
Desktop tests exercise the real worker, a simulated portal transition, obstacle
execution, action ordering and dynamic settings persistence. Client smoke tests
run under Ruffle and exercise command validation, object construction, bridge
bindings and shared-memory round trips. The runnable sequence completes 100
simulated spins. These checks do not replace an authenticated live game run:
portal jumps, server item effects, revival and real-map geometry still need
live verification.
