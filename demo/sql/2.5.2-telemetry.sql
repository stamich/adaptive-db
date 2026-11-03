-- Adaptive DB 2.5.2: telemetry phase of the feature tour (see DemoMain.telemetryPhase).
-- Runs after the 2.5.1 phase. \telemetry shows the counters, \telemetry shapes the busiest shapes.

SET telemetry = off;                                                   -- the default: no work at all
SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;
\telemetry                                                             -- all counters zero

SET telemetry = full;                                                  -- capture every query (diagnostics)
SELECT id, amount FROM ledger WHERE account = 1;
SELECT id, amount FROM ledger WHERE account = 2;                       -- same shape, other literal
SELECT id, amount FROM ledger WHERE account = 3;
\telemetry shapes                                                      -- one shape, 3 executions, rows read vs returned

SET telemetry = sampled;                                               -- production: skip known, fast queries
\telemetry                                                             -- skipped grows far faster than captured
SET telemetry = off;
