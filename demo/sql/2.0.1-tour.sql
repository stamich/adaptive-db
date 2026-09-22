-- Adaptive DB 2.0.1 chronological feature-tour SQL.
-- The executable DemoMain dynamically substitutes the AS OF VERSION timestamp.

-- Bootstrap using the 2.0.1 SQL/catalog layer.
CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);

-- 1.0.1 lineage: committed mutations + WAL/recovery.
INSERT INTO account VALUES (1, 100, 'Alice');
INSERT INTO account VALUES (2, 200, 'Bob');
SELECT * FROM account;
-- Demo closes and reopens the database here.
SELECT * FROM account;

-- 1.5.1 lineage: persistent current store / primary-key point lookup.
EXPLAIN SELECT * FROM account WHERE id = 2;
SELECT * FROM account WHERE id = 2;

-- 1.6.1 lineage: durable historical version.
UPDATE account SET balance = 250 WHERE id = 2;
SELECT * FROM account WHERE id = 2;
-- Executed by DemoMain with the captured old commit timestamp:
-- SELECT * FROM account AS OF VERSION <OLD_COMMIT_TS> WHERE id = 2;

-- 1.7.1 lineage: physical execution + native RecordBatch output.
EXPLAIN ANALYZE SELECT id, owner, balance FROM account WHERE balance >= 100 LIMIT 10;
SELECT id, owner, balance FROM account WHERE balance >= 100 LIMIT 10;

-- 2.0.1: complete SQL/control-plane path.
INSERT INTO account VALUES (3, 300, 'Carol');
UPDATE account SET balance = 275 WHERE id = 2;
DELETE FROM account WHERE id = 1;
SELECT id, owner, balance FROM account WHERE balance > 200 LIMIT 10;
EXPLAIN SELECT * FROM account WHERE id = 3;
