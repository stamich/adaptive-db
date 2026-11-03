-- Adaptive DB 2.5.4: actuator phase of the feature tour (see DemoMain.actuatorPhase).
-- Runs after the 2.5.3 phase. In auto_safe mode the controller creates the index the advisor
-- recommends, measures it and keeps or undoes it. The default mode is off.

SET adaptive = advisory;                                               -- watch only: no statement is ever run
-- Run this query 40 times, then \actions; repeat. Nothing is created in advisory mode.
SELECT id, amount FROM ledger WHERE currency = 'PLN';
SHOW INDEXES ON ledger;

SET adaptive = auto_safe;                                              -- telemetry goes to `full` while this is on
-- Keep running the query; the controller ticks every 10 s. After three evaluations in a row the
-- policy gate approves CREATE INDEX adv_ledger_currency ON ledger (currency) and the actuator runs it.
SELECT id, amount FROM ledger WHERE currency = 'PLN';
SHOW INDEXES ON ledger;                                                -- adv_ledger_currency, owned by the advisor
\actions                                                               -- state: applied -> kept after the verify windows
\actions log                                                           -- applied, kept (or rolled_back / rejected)

SET adaptive = off;                                                    -- the controller stops; telemetry mode is restored
DROP INDEX adv_ledger_currency;
