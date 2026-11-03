# Security and robustness review — Milestone 2.5.0

Scope: everything 2.5.0 adds that reads untrusted bytes or accepts untrusted numbers — the change
log files, the projection checkpoint, the C ABI for change cursors and projections, the Java
binding and the session token. Findings are listed with the control that handles them and the
test that proves it.

## Inputs and the limits that bound them

| Input | Risk | Control | Test |
|---|---|---|---|
| Change-log segment files (disk) | torn write, bit rot, planted file | every frame has a length and a CRC-32; the header is checked; damage in the last segment's tail is truncated, damage elsewhere is an error; the engine rebuilds a damaged log from the WAL | `store.rs` (`torn_tail_is_truncated`, `mid_log_damage_is_an_error`), `change_log_recovery.rs`, `fuzz.rs` |
| Envelope length prefix | huge allocation | `MAX_ENVELOPE_BYTES` (64 MiB) is checked before any allocation, on append and on read; decoding is bounded by the same cap | `oversized_envelope_is_refused`, `fuzz.rs` |
| Arbitrary envelope bytes | panic | decoding returns an error for every input | `fuzz.rs::decoding_arbitrary_bytes_is_safe` |
| Segment file names | confusion about order | the first sequence number in the name must match the first frame | `store.rs` |
| `projections.meta` | torn or edited checkpoint | atomic replace through a temporary file; CRC over the whole file; frontier consistency checked on load; any single-bit flip and any truncation is rejected | `checkpoint.rs::damage_is_detected` |
| Projection rate reports | NaN or infinity poisoning lag estimates | the smoothed rate rejects NaN, infinity and negatives | `registry.rs::rate_is_smoothed_and_validated` |
| `adb_change_cursor_next_batch` | unbounded batch | `max_commits` is 1..=10000 and `max_bytes` 1..=64 MiB; a larger request is `INVALID_ARGUMENT`; one commit larger than the byte budget is still delivered whole, never split | `change_cursor.rs::argument_validation` |
| `adb_projection_wait_until` | thread parked forever | the wait is capped at 60 s (`MAX_WAIT`) in the runtime and in the Java binding; the wait runs outside the object monitor so writers are not blocked | `wait.rs::the_wait_cap_is_bounded`, `AdaptiveSessionTest` |
| Change cursor handles | null or foreign pointer | every entry point null-checks and runs inside the panic guard; a handle must be closed exactly once (documented in `include/adb.h`, and enforced by the Java wrapper, which clears its pointer on close) | `change_cursor.rs::argument_validation` |
| Projection names and JSON | injection, huge documents | names are non-empty and unique; registration JSON is limited to 4 KiB; the Java decoder rejects wrong types, negative numbers and unknown enum names | `projection.rs::validation`, `ProjectionJsonCodecTest` |
| Session token | forged or stale token | a token is only a number; one beyond the head is refused (`SESSION TOKEN ... beyond this database's head`) instead of silently returning old data; it never grants access to anything, only widens what a read waits for | `FreshnessResolverTest`, `AdaptiveSessionTest` |
| `SET SESSION CONSISTENCY` | overflow, negative bounds | bounds are parsed as non-negative 63-bit integers; larger or fractional values are parse errors | `SqlParserTest::parsesSessionConsistency` |

## Consistency properties that matter for safety

* A projection that is `Building`, `NeedsRebuild` or `Failed` is never read; the planner falls back
  to the canonical row path, which always exists.
* A wait that times out is not an error: the read is planned again without the lagging index, so a
  slow projection delays a read at most once, by the session's wait limit (default 2 s, at most 60 s).
* Retention can never delete a change-log segment a registered consumer still needs unless a
  configured limit (bytes, age or lag) says so; a consumer that loses commits that way is marked
  `NeedsRebuild` and cannot serve reads until rebuilt.
* A transaction that has uncommitted writes must read its own writes from the canonical row path:
  projections only ever contain committed data, so they cannot serve such a read. In 2.5.0 every
  index is still maintained inside the commit, so this is not reachable through SQL; the rule is
  recorded in `docs/consistency.md` for the asynchronous projections of later milestones.
* The change log is derived data. Losing or damaging it never loses committed data: the WAL
  rebuilds it, and the engine does so when it opens a database.

## Residual risks

* The change log is not encrypted or access-controlled beyond the data directory's permissions; it
  holds the same row images as the WAL.
* An external consumer that never advances keeps its retention frontier low; the byte, age and lag
  limits of `ChangeRetentionPolicy` are what bound disk use, and they are off by default.
* Time-bounded staleness (`BOUNDED n MS`) is an estimate from the observed consumption rate, not a
  measurement of wall-clock age; a projection with no observed rate cannot prove a time bound.
