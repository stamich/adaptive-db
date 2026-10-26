-- Adaptive DB 2.3.0: secondary-index phase of the feature tour (see DemoMain.indexPhase).
-- The tour loads 2,000 ledger rows with the formula below in single-row INSERTs; amount is
-- unique, so ORDER BY amount has no ties and cost and rule mode return the same rows.

CREATE TABLE ledger (id BIGINT PRIMARY KEY, account BIGINT NOT NULL, amount BIGINT NOT NULL, memo STRING NOT NULL);
-- ledger i: (i, i % 200, (i * 7919) % 10007, 'memo-i')   for i in 1..2000
ANALYZE ledger;
EXPLAIN SELECT account, amount FROM ledger WHERE account = 42;

CREATE INDEX idx_ledger_account ON ledger (account) INCLUDE (amount);
SHOW INDEXES;
ANALYZE ledger;
EXPLAIN ANALYZE SELECT account, amount FROM ledger WHERE account = 42;   -- covering index scan
EXPLAIN SELECT id, memo FROM ledger WHERE account = 42;                 -- index scan + row fetch

CREATE INDEX idx_ledger_amount ON ledger (amount);
EXPLAIN SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5;     -- backward scan, no TopK
SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5;
SET optimizer = rule;
SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5;             -- same rows, TopK over a scan
SET optimizer = cost;

UPDATE ledger SET amount = 20000 WHERE id = 7;                          -- prints commitTs=<ts>
SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5;
SELECT id, amount FROM ledger AS OF VERSION <ts - 1> ORDER BY amount DESC LIMIT 5;

DROP INDEX idx_ledger_amount;
EXPLAIN SELECT id, amount FROM ledger ORDER BY amount DESC LIMIT 5;     -- sequential scan + TopK
