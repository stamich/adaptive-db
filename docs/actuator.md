# The AUTO_SAFE actuator (Milestone 2.5.4)

2.5.3 recommended; 2.5.4 can **act on the recommendation, measure the result and take it back**.
It is a JVM module, `adb-actuator` (package `io.adb.actuator`), wired into the gateway. The engine
did not change: C ABI 9, plan wire v4 and data format 4 are as in 2.5.3.

The loop is **observe → recommend → decide → act → measure → keep or undo**, and it acts through
exactly three statements: `CREATE INDEX`, `ANALYZE` and `DROP INDEX`. Nothing else is ever run.

## Modes

`SET adaptive = off | advisory | auto_safe`

| Mode | What the controller does |
|---|---|
| `off` (default) | nothing: no thread, no evaluation, no state change on any query |
| `advisory` | evaluates the advisor on its timer, as 2.5.3 did; never runs a statement |
| `auto_safe` | everything below |

`SET advisor = on | off` still works and means `advisory` | `off`. `auto_safe` switches telemetry to
`full` (in `sampled` mode a fast query is mostly skipped, so the runs after an index was built would
be undercounted and could not be verified) and restores the earlier mode when `auto_safe` is left,
unless a person changed the telemetry mode in between. Leaving `auto_safe` stops new actions and
rollbacks at once; a statement already running finishes or fails atomically in the engine.

## One tick

The advisor's timer (10 s by default) runs `AdaptiveController.tick(design)`. In `auto_safe` a tick
does, in this order:

1. **Evaluate** the advisor (the same hysteresis as 2.5.3: three evaluations in a row).
2. **Adopt, then reconcile.** `adv_*` indexes found in the design after a restart are re-adopted
   (no baseline, never verified, subject to the idle rule only); an owned index that is no longer in
   the design was dropped by someone else and is recorded as `externally_dropped`.
3. **Verify** every pending action (below): keep it, undo it, or wait.
4. **Drop idle indexes** the advisor owns and no query wants any more.
5. **Approve at most one** new action, chosen from the advice that passes the policy gate.

At most one action per tick, and at most one build at a time, are deliberate: every action changes
what the next one must be measured against.

## The policy gate

`PolicyGate` is a pure function of a `GateContext` (plain values: the recommendation, the workload,
the physical design, the owned indexes, the history, the policy, the budget). It returns approve or
one of **eleven reasons**, checked in a fixed order so the same input always gives the same reason:

`not_actionable` · `already_exists` · `too_few_executions` · `below_ratio` · `externally_dropped` ·
`cooldown` · `rollback_cooldown` · `too_many_indexes` · `table_too_large` · `too_many_bytes` ·
`build_in_flight`

A rejection is logged once per (target, reason), not once per tick.

### Defaults

| `Policy` | Default | Meaning |
|---|---|---|
| `minExecutions` | 50 | evidence needed before acting, and before a before/after figure counts |
| `minRatio` | 4.0 | benefit-to-cost ratio of the recommendation |
| `cooldown` | 30 min | between two actions on the same target |
| `rollbackCooldown` | 6 h | after a rollback of the target (doubled per rollback, up to 8x: `FlapGuard`) |
| `verifyWindows` | 6 | windows observed after an action before the verdict |
| `minRowsReduction` | 2.0 | rows read per execution must fall at least this many times |
| `timeTolerance` | 2.0 | time per execution may not rise beyond this factor |
| `maxWriteSlowdown` | 0.25 | write slowdown that counts against the index |
| `idleWindows` | 360 | windows an owned index may go unwanted before it is dropped (one hour at 10 s) |
| `adoptByPrefix` | true | re-adopt `adv_*` indexes after a restart |

| `Budget` | Default | Meaning |
|---|---|---|
| `maxAdvisorIndexes` | 8 | indexes the advisor may own |
| `maxIndexBytes` | 1 GiB | estimated size of what it owns (`bytesPerEntry` 48) |
| `maxConcurrentBuilds` | 1 | builds in flight |
| `maxBuildRows` | 50 000 | largest table it will index by itself |

`maxBuildRows` comes from a measurement: `CREATE INDEX` stalls the database, reads and writes, for
the whole build, about 30 µs per row. 50 000 rows is about 1.5 s. Larger tables still get advice;
a person builds the index at a time of their choosing.

## Measuring: the effectiveness verifier

Before an action, `BaselineCapture` records, from the workload windows, the figures of the shapes
the recommendation named (and the writes of the table). After it, `AfterPeriod` counts **only the
windows that started at or after the moment the action was applied**, so a window that straddles
the build is never used.

`EffectivenessVerifier` compares per-execution figures, weighted by the executions after:

- **Primary evidence: rows read per execution.** It is the engine's exact counter
  (`adb_query_telemetry`), so it does not depend on the load of the machine.
- **Time is secondary.** It can only veto, and only beyond `timeTolerance` (2x): timings on a shared
  machine are too noisy to prove an improvement, but a doubling is not noise.
- Verdicts: `NotYet` (fewer than `verifyWindows`), `Inconclusive` (too little evidence: kept, not
  vouched for), `Keep`, `Undo`.

`WriteCostCheck` adds the cost an index puts on writes: a write slowdown above `maxWriteSlowdown`
is a regression only if the writes' lost time exceeds the reads' saved time, so a read-heavy table
may keep an index that slows its few writes a little.

An undo is a `DROP INDEX` planned by `RollbackPlanner`; the target is then in its rollback cooldown.

## Ownership: the only drop authority

The controller never drops an index it did not create. `OwnershipRegistry.mayDrop` is the single
authority; `OwnedOnlyActuator` is a second guard that refuses a `DROP INDEX`, or an undo, of an
index the registry does not own, before the call reaches the engine. A user's index named like an
advisor index is protected by one more rule: only `adv_` indexes can be adopted, and only when they
are in the physical design. An index a person dropped by hand is remembered
(`externally_dropped`) and the gate refuses to build that target again: the person's decision stands.

## Idle indexes

`IdleDetector` counts windows in which something ran but no shape *wants* the index. A shape wants
an index when it filters on, or joins through, the index's leading column. Quiet windows (nothing
ran) neither count nor reset the count, so a weekend does not retire an index. After `idleWindows`
the index is dropped and logged. Adopted indexes have no baseline, so this is the only rule that
applies to them.

## Concurrency

The gateway has a `ddlLock` (a `ReentrantLock`) that serialises user DDL and actuator statements.
The actuator has its own session, so it never shares a transaction with a user. `BuildGuard` limits
concurrent builds and refuses new ones after shutdown.

## What you can see

- `\actions` (`AdaptiveDatabase.showActions()`): `id | target | action | state | sql | reason`.
  States: `proposed`, `rejected`, `applied`, `failed`, `kept`, `rolled_back`.
- `\actions log` (`showActionsLog()`): the bounded, sequence-numbered `ActionLog`. Events: `applied`,
  `kept`, `rolled_back`, `rejected`, `failed`, `externally_dropped`, `adopted`, `unverified`.
- `\advice` and `\advice log` as in 2.5.3.

## Write counters

The write cost needs data the 2.5.3 model did not have. While telemetry is on, the gateway counts
`INSERT`, `UPDATE` and `DELETE` per entity (statements, rows changed, wall-clock nanoseconds:
`WriteCounters`); the advisor's windows carry them as `WriteDelta` and the physical design as
`EntityDesign.writes`. The record allocates nothing for an entity already seen (checked by
`check-benchmarks.py`). See [telemetry.md](telemetry.md).

## Tests and checks

- Unit tests per piece: gate, policy, ownership, build guard, baseline, verifier, write cost, idle
  detector, rollback planner, flap guard, action log.
- `AdaptiveControllerTest` runs the whole loop against `Rig`, a scripted pretend database.
- `examples/jvm-actuator-smoke` runs it against the real engine: nothing in advisory mode, a kept
  index, a rolled-back index, the refusal during the cooldown, switching modes, a user index left
  alone, the idle rule, an external drop that is not undone and a budget of one index. Adoption after
  a restart is covered by `OwnershipRegistryTest` and `AdaptiveControllerTest`.
- The `actuator` section of the JVM workload report, checked exactly by `scripts/check-benchmarks.py`.
- The feature tour, phase 2.5.4.

## Not done

- **Reference-machine timing.** The effect is judged on rows read, which is exact; the time figures
  need the reference machine.
- **Persisting** the ownership registry and the action log. After a restart the `adv_` prefix is the
  only memory; the log starts empty.
- **Other actions**: join projections, partitions, dropping a user's index, changing settings. They
  stay informational until their own actuators exist.
- **Building large tables.** `CREATE INDEX` blocks the database for the build; an online build is an
  engine feature for a later milestone. Until then `maxBuildRows` keeps the stall short.
- **Learning the thresholds.** The defaults are conservative constants, not tuned on real workloads.
