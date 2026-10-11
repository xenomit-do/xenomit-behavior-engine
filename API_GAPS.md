# Upstream plugin feature and API gap catalogue

This is a maintenance catalogue for deciding what can be implemented as a native
behavior, what depends on backend-specific captured fields, and what needs a new
typed API or host service. The machine-readable source is
[`api-gaps.json`](api-gaps.json). “Native” describes the shared contract, not
every backend: packet, legacy-pipe, Ruffle and Flash may expose different
observations and command support.

## Audited revisions

| Source | Revision |
|---|---|
| [SharedPlugin](https://github.com/Darkbot-Plugins/SharedPlugin) | `fe5e3b3a9d369b3673037410d9e4765681fdf161` (plugin version 0.12.22) |
| [DefaultPlugin](https://github.com/darkbot-reloaded/DefaultPlugin) | `1d8e3a9fe28e77b5066354da8af503d6df1171f4` |
| [DmPlugin](https://github.com/darkbot-reloaded/DmPlugin) | `d3ebcc7828c3b9eb3755dc4b6f8d47c4dc2cca36` |

The DefaultPlugin and DmPlugin general behavior port and its field/condition
limits are described in [GENERAL_BEHAVIORS.md](GENERAL_BEHAVIORS.md). That
document remains the detailed rule reference; this catalogue adds ownership
and tracks missing API families across all three upstreams.

## SharedPlugin inventory

The source `plugin.json` registers these features. “Optional captured” means a
backend or adapter may populate useful fields, but the shared contract does not
guarantee them. Generic `fields` are forward-compatible storage, not proof that
a backend captures a value or that an action can be completed safely.

| Feature | Suggested plugin group / module owner | Coverage | Main boundary |
|---|---|---|---|
| SimpleHealing | combat / `base.combat` | Partial | Generic item use and health observations exist; ship ability readiness and compatibility are backend dependent. |
| RepairPet | combat / `base.combat` | Partial | Pet activation/gear commands exist; pet owned/active/health/shield/fuel/gears/idle are typed observations where the backend exposes them. The repair workflow itself is not a complete normalized contract. |
| FixPetStuck | utility / `base.utility` | Partial | Pet commands exist; pet position, activity/idle and gear state are observed; a stuck timer is derived client-side, not server state. |
| CrowdAvoidance | combat / `base.combat` | Partial | Entity positions, safe places and combat flags can support a policy; exact enemy/safety config semantics may be absent. |
| SolarisInc | combat / `base.combat` | Partial | Generic item command exists; ship capability and authoritative cooldown state are not stable typed observations. |
| OreSeller | economy / `base.economy` | Partial | `economy.ores`/`cargo`/`trade_available` are observed and `SellOre` is a typed command on all backends, validated against observed inventory and trade area. Sale prices, config changes and transaction completion remain gaps. |
| AutoRefin | economy / `base.economy` | Partial | `RefineOre` is a typed command validated against observed recipe capacity on all backends. A dedicated transaction acknowledgement is still missing. |
| Autobuy | economy / `base.economy` | Missing API | Needs catalog/pricing, balances, authenticated purchase service and completion/error data. ServiceRegistry can host an adapter. |
| Spaceball | event / `shared_spaceball` | Partial | Manual attack/follow, directed travel and named cargo collection are implemented; spaceball score/team widgets are captured-optional observations on the graphical backends. Automatic team/direction decisions, scheduling, profile switching and refresh policy remain gaps. |
| SimpleGalaxyGate | gg / existing `galaxy` family | Partial | Existing gate modules run a typed builder/travel/combat lifecycle; `gate.builder` progression and `SelectGateDifficulty` (observed selection UI) are typed APIs on Ruffle/Flash. SharedPlugin's broader per-gate scripts, event schedules and hangar switching remain gaps. |
| FastTravel | utility / `base.utility` | Partial | Directed portal routing and JumpPortal exist; upstream module-mode detection and config thresholds need local equivalents. |
| GGAlertCloser | utility / `base.utility` | Partial | `gui` element enumeration plus `ClickGui`/`CloseGui` are typed APIs on Ruffle and Flash; identifying the alert window is module policy. The packet backend has no GUI. |
| CloseAdOffer | utility / `base.utility` | Partial | Enumerated GUI elements expose the offer window when visible; `CloseGui`/`ClickGui` act on it on Ruffle and Flash. The packet backend has no GUI. |
| LogOverlay | utility / `base.utility` | Partial | `poll_events` is a bounded log/journal stream on all backends; `SetOverlay`/`RemoveOverlay` draw host-owned text on the game stage on Ruffle and Flash. Localization parity is missing. |
| TemporalModuleDetector | utility / `base.utility` | Local equivalent | Upstream tests runtime Java module classes; Rust can use module identity/settings without importing that API. |
| CustomSafetyFinder | combat / `base.combat` | Partial | Native pathfinding, safe places and map geometry help; CBS ownership/hull, detailed safety settings and run thresholds may be optional. |
| SharedPlugin general rules | base / `base.behaviors` | Partial | Fold into the global behavior catalog where appropriate; use GENERAL_BEHAVIORS.md for individual rule gaps. |

### Shared gate catalogue

These are the actual `gate/*Gate.java` handlers in the audited checkout. The
existing engine modules are related implementations, not full ports of those
handlers. Remaining scripts and missing APIs must be tracked separately.

| Upstream handler | Bundle | Existing equivalent | Remaining coverage |
|---|---|---|---|
| AbgGate (Alpha/Beta/Gamma) | gg | `galaxy` | Shared NPC flags, completion/account policies |
| DeltaGate | gg | `galaxy` | Shared gate-specific tactics and completion policies |
| EpsilonGate | gg | `galaxy` | Shared gate-specific tactics and completion policies |
| ZetaGate | gg | `galaxy` | Shared phase/target policy |
| HadesGate | gg | `hades` | Group coordination, invitations and Shared phase policy |
| KuiperGate | gg | `galaxy` | Exact type-based branch portal policy |
| LowGate | gg | `low` | Century Falcon/group tactics and eligibility |
| EternalBlacklightGate | gg | `eternal_blacklight` | Accurate per-run waves, configured exit/suicide policies, GUI dismissal |
| GopGate | gg | `gop` | Shared reward/portal/scheduling policies |
| TreacherousDomainGate | event | `treacherous_domain` | Shared gate-specific tactics and completion policies |
| InvasionGate | event | None | Event availability/schedule; handler script and exact portal policy remain to port |
| MimesisMutinyGate | event | None | Escort/event progression observations; guard, loot and portal scripts |
| DseGate | event | None | Gate phase/target observations and its specialized combat/PET scripts |
| FiestaGate | event | None | Typed difficulty selection (`difficultySelection`) and handler script |
| TrinityTrialsGate | event | None | Typed gate/difficulty selection (`trinitytrials_difficultyselect`) and handler script |
| VoyagersAscentGate | event | None | Typed difficulty selection (`singularitydrive_difficultyselect`) and handler script |
| AnyGate | gg | None | Explicit safe opt-in routing for arbitrary gates; not a blanket fallback |

Difficulty selection/GUI state is now a typed API (`SelectGateDifficulty` on
the observed selection window) and gate builder progression is observed through
`gate.builder`; the missing normalized APIs are event schedules, authoritative
per-gate mechanics beyond the builder model, authenticated hangar switching,
and gear cooldowns. Portal/NPC scripts which can use existing
observations are remaining implementation work, rather than new API requests.

### Ownership policy

Keep reusable observations and commands in `xenomit-game-api`. Put cross-cutting
always-on policies in the engine's global behavior registry, combat tactics in
the combat behavior family, travel/GUI-adjacent conveniences in utility, and
event-specific objectives in event modules. Economy transactions should be
host services with explicit request results and authentication boundaries;
they should not be simulated with guessed `call_function` invocations.
Galaxy-gate spin/place commands are useful primitives, but a gate module owns
the scheduling, wave state, retries and objective policy. Spaceball is likewise
an event objective, not a general combat rule.

## API facts available today

Typed game commands include portal jump, pet activation/gear selection, ship
configuration selection, generic item use, revive, galaxy-gate spin/place/life
purchase, group invite acceptance, overlay set/remove, GUI click/close, ore
refine/sell, and observed gate-difficulty selection. `feature_state` carries a
fresh `observed_at_unix_ms`, a capabilities list, and optional sections for
GUI element trees (Ruffle/Flash), pet vitals and gears, gate builder
progression and difficulty offerings, event server time/team/active windows,
and economy ore/balance/cargo/trade state. `poll_events` returns a bounded,
cursor-ordered event journal on all backends. Snapshots can carry entities,
generic entity/hero fields, map bounds and safe places, navigation
catalogs/portals, group data, and optional native frames. The Ruffle native
capture provides a broader set of observations than legacy pipe or packet
adapters; absence of a field must remain “unknown.”

The engine's `ServiceRegistry` and completion-aware `TaskSequence` are suitable
extension points for authenticated backpage, shop, calendar, or event adapters.
They do not themselves implement those integrations. Important missing typed
surfaces are authenticated shop/backpage purchase services with completion
results, event schedules and authoritative per-gate mechanics, hangar switching
and profile/config manager operations, captcha support, packet-backend GUI
operations, a pet stuck timer, and ship-specific ability readiness. Host
lifecycle actions that already exist are listed in GENERAL_BEHAVIORS.md.

## Keeping this catalogue current

When updating an upstream audit, record the exact `git rev-parse HEAD` for each
checkout and review the feature registration manifest as well as implementation
classes. Update the JSON and this human-readable table together. Keep the
coverage labels narrow: `native_api` means a normalized API exists,
`captured_optional` means data may be supplied but is not guaranteed, and
`unavailable` means implementation needs an API/service or client capability.
