-- Adaptive DB 2.2.3: statistics and cost-based optimization phase of the feature tour
-- (see DemoMain.statisticsPhase). The tour loads 4 regions, 40 shops and 400 sales with the
-- formulas below in single-row INSERTs.

CREATE TABLE region (id BIGINT PRIMARY KEY, name STRING NOT NULL);
CREATE TABLE shop (id BIGINT PRIMARY KEY, region_id BIGINT NOT NULL REFERENCES region(id) NOT ENFORCED, name STRING NOT NULL);
CREATE TABLE sale (id BIGINT PRIMARY KEY, shop_id BIGINT NOT NULL REFERENCES shop(id) NOT ENFORCED, amount BIGINT NOT NULL);
-- region i:  (i, 'region-i')                      for i in 1..4
-- shop i:    (i, 1 + i % 4, 'shop-i')             for i in 1..40
-- sale i:    (i, 1 + (7 * i) % 40, (37 * i) % 500 + 1) for i in 1..400

EXPLAIN SELECT r.name, SUM(s.amount) AS total FROM sale s JOIN shop sh ON s.shop_id = sh.id JOIN region r ON sh.region_id = r.id WHERE r.name = 'region-2' GROUP BY r.name;
ANALYZE;
EXPLAIN ANALYZE SELECT r.name, SUM(s.amount) AS total FROM sale s JOIN shop sh ON s.shop_id = sh.id JOIN region r ON sh.region_id = r.id WHERE r.name = 'region-2' GROUP BY r.name;
SELECT r.name, SUM(s.amount) AS total FROM sale s JOIN shop sh ON s.shop_id = sh.id JOIN region r ON sh.region_id = r.id WHERE r.name = 'region-2' GROUP BY r.name;
SET optimizer = rule;
SELECT r.name, SUM(s.amount) AS total FROM sale s JOIN shop sh ON s.shop_id = sh.id JOIN region r ON sh.region_id = r.id WHERE r.name = 'region-2' GROUP BY r.name;
SET optimizer = cost;
-- sale i: (i, 1 + i % 40, 1) for i in 401..600
EXPLAIN SELECT COUNT(*) FROM sale;
