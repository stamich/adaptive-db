-- Adaptive DB 2.5.1: plan cache phase of the feature tour (see DemoMain.planCachePhase).
-- Runs after the 2.5.0 phase. \cache shows the counters, \cache clear empties the cache.

EXPLAIN SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;   -- "Plan cache:" line
SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;            -- first sighting: remembered
SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;            -- second: plan stored
SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;            -- third: cache hit
\cache                                                                        -- hits, inserts and deferred show the admission

ANALYZE ledger;                                                               -- statistics generation moves
SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;            -- stale: planned again
\cache                                                                        -- stale grew by one

SET plan_cache = off;
SET plan_cache = on;
