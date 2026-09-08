# Milestone 1.0.1 — Security / Durability Review

## Hardened issues

### HIGH — WAL crash-tail append hazard
Milestone 1.0 could ignore an incomplete suffix during recovery but reopen the writer at physical EOF.
New commits could therefore be appended behind the damaged suffix and become invisible to the next recovery.

1.0.1 scans to the last complete valid frame and truncates only an incomplete crash suffix before append.
Fully framed corruption (bad magic/version/CRC) remains a hard error and is never silently discarded.

### HIGH — attacker/corruption-controlled WAL allocation
The original reader trusted the persisted `payload_len` and allocated `Vec(payload_len)`.
A corrupted length could trigger a multi-gigabyte allocation and process OOM.

1.0.1 enforces `MAX_WAL_RECORD_BYTES = 16 MiB` before allocation and before append.

### MEDIUM — unchecked frame-length narrowing
The original writer cast payload length directly to `u32`.
1.0.1 uses checked conversion.

## Properties preserved

The original WAL-before-visibility ordering remains unchanged:

1. validate transaction,
2. append BEGIN/mutations/COMMIT,
3. `flush + sync_data`,
4. apply in-memory stores,
5. publish CommitTs.

Recovery still applies only transactions that have a COMMIT record.

## Residual risks intentionally not solved in 1.0.1

- WAL is one unbounded file; no segmentation/rotation.
- Recovery still materializes the complete valid WAL in memory before logical replay.
- There is no checkpoint; restart cost grows with WAL size.
- Mid-file corruption fails database open; there is no redundant WAL copy or repair.
- Storage is in-memory after recovery; persistent page/current store belongs to Milestone 1.5.
- No authentication/encryption exists because Milestone 1 is an embedded storage-engine milestone, not a network server.

These limits are architectural scope boundaries, not hidden claims of production readiness.
