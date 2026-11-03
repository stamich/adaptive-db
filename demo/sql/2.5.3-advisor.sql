-- Adaptive DB 2.5.3: advisor phase of the feature tour (see DemoMain.advisorPhase).
-- Runs after the 2.5.2 phase. The advisor only recommends; \advice shows what it recommends,
-- \advice log what it has ever recommended.

SET telemetry = full;                                                  -- the advisor reads telemetry; without it, it sees nothing
\advice                                                                -- baseline: nothing to advise yet

-- Run this query 40 times, then \advice; repeat until the third \advice. `currency` has no index
-- and one of 2,001 rows is PLN, so every run reads the whole table to return one row.
SELECT id, amount FROM ledger WHERE currency = 'PLN';
\advice                                                                -- evaluation 1: not shown yet
\advice                                                                -- evaluation 2: not shown yet (run the query again in between)
\advice                                                                -- evaluation 3: CREATE INDEX adv_ledger_currency ON ledger (currency)

SHOW INDEXES ON ledger;                                                -- no index was created by the advisor
CREATE INDEX adv_ledger_currency ON ledger (currency);                 -- the person carries the advice out
\advice                                                                -- the advice is gone (applied)
\advice log                                                            -- confirmed, then applied

DROP INDEX adv_ledger_currency;
SET advisor = on;                                                      -- evaluate in the background every 10 s
SET advisor = off;
SET telemetry = off;
