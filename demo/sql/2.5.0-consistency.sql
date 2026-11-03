-- Adaptive DB 2.5.0: consistency phase of the feature tour (see DemoMain.consistencyPhase).
-- Runs after the 2.4.0 phase, in a new session on the reopened database. Shell meta-commands
-- (\session, \projections, \changes [after] [max]) show the same tables the demo prints.

\session                                                          -- consistency=READ_YOUR_WRITES, token=0
INSERT INTO ledger (id, account, amount) VALUES (3001, 1, 4242);   -- prints commitSeq=<n>
\session                                                          -- the token moved to that commit

SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;   -- sees the write through the index
EXPLAIN SELECT id, amount FROM ledger WHERE account = 1 AND amount = 4242;   -- "Read context" section

SET SESSION CONSISTENCY EVENTUAL;                                  -- never waits for a projection
SET SESSION CONSISTENCY BOUNDED 250 MS;                            -- a projection may trail by about 250 ms
SET SESSION CONSISTENCY BOUNDED 3 COMMITS;                         -- ... or by three commits
SET SESSION CONSISTENCY STRONG;                                    -- everything committed so far
SET SESSION CONSISTENCY READ_YOUR_WRITES;                          -- the default

\projections                                                      -- __current_row and index:<name>, with state and lag
\changes 0 5                                                      -- the first commits of the change log
