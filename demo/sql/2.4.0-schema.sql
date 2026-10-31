-- Adaptive DB 2.4.0: schema-evolution phase of the feature tour (see DemoMain.schemaPhase).
-- Runs after the 2.3.0 phase on its ledger table (2,000 rows, index idx_ledger_account).
-- Every DDL statement commits a catalog record to the engine's log and advances the catalog
-- epoch; no statement rewrites the stored rows.

ALTER TABLE ledger ADD COLUMN currency STRING NOT NULL DEFAULT 'EUR';
SELECT id, amount, currency FROM ledger WHERE id <= 3;          -- old rows read the default
EXPLAIN SELECT id FROM ledger WHERE currency = 'EUR';           -- Schema section; estimate from the default

UPDATE ledger SET currency = 'PLN' WHERE id = 2;                 -- the row is migrated to version 2
INSERT INTO ledger (id, account, amount, memo) VALUES (2001, 1, 9999, 'memo-2001');   -- currency defaults
SELECT id, currency FROM ledger WHERE id = 2 OR id = 3 OR id = 2001;

ALTER TABLE ledger RENAME COLUMN memo TO note;                  -- metadata only, same field id
SELECT id, note FROM ledger WHERE id = 1;

ALTER TABLE ledger DROP COLUMN account;                         -- refused: idx_ledger_account uses it
ALTER TABLE ledger DROP COLUMN note;                            -- logical drop, values stay in the log
SELECT * FROM ledger WHERE id = 1;

INSERT INTO ledger VALUES (2002, 1, 'not a number', 'EUR');     -- refused: type mismatch
SHOW SCHEMA ledger;                                             -- one row per version and epoch

CREATE TABLE scratch (id BIGINT PRIMARY KEY, v BIGINT DEFAULT 0);
DROP TABLE scratch;

-- After a close and reopen:
SHOW SCHEMA ledger;
SELECT id, amount, currency FROM ledger WHERE id <= 3;
