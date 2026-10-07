-- Adaptive DB 2.1.3: relational execution phase of the feature tour (see DemoMain.relationalPhase).

CREATE TABLE customer (id BIGINT PRIMARY KEY, name STRING NOT NULL, city STRING);
CREATE TABLE orders (id BIGINT PRIMARY KEY, customer_id BIGINT NOT NULL, amount BIGINT NOT NULL);

INSERT INTO customer VALUES (1, 'Ada', 'Krakow');
INSERT INTO customer VALUES (2, 'Ben', 'Gdansk');
INSERT INTO customer VALUES (3, 'Cy', 'Krakow');
INSERT INTO customer VALUES (4, 'Dee', 'Poznan');
INSERT INTO orders VALUES (10, 1, 120);
INSERT INTO orders VALUES (11, 1, 80);
INSERT INTO orders VALUES (12, 2, 300);
INSERT INTO orders VALUES (13, 3, 40);
INSERT INTO orders VALUES (14, 3, 60);
INSERT INTO orders VALUES (15, 3, 500);

-- HashJoin + Sort
SELECT c.name, o.id, o.amount FROM customer c JOIN orders o ON o.customer_id = c.id ORDER BY o.amount DESC;

-- LEFT JOIN null-filling; COUNT(x) skips NULLs (Dee has 0 orders)
SELECT c.name, COUNT(o.id) AS orders FROM customer c LEFT JOIN orders o ON o.customer_id = c.id
GROUP BY c.name ORDER BY c.name;

-- GROUP BY with SUM / AVG; LIMIT over ORDER BY runs as TopK
SELECT c.city, SUM(o.amount) AS total, AVG(o.amount) FROM customer c JOIN orders o ON o.customer_id = c.id
GROUP BY c.city ORDER BY total DESC LIMIT 2;

-- Self-join: both instances of orders get their own slots
SELECT a.id, b.id FROM orders a JOIN orders b ON a.customer_id = b.customer_id AND a.id < b.id;

-- No equality key: bounded NestedLoopJoin; o.amount >= 300 is pushed below the join
SELECT c.name, o.id, o.amount FROM customer c JOIN orders o ON o.customer_id <> c.id AND o.amount >= 300
ORDER BY c.name, o.id;

-- Plans, decisions with reasons, and the native per-operator profile
EXPLAIN ANALYZE SELECT c.city, SUM(o.amount) AS total FROM customer c JOIN orders o ON o.customer_id = c.id
WHERE o.amount >= 50 GROUP BY c.city ORDER BY total DESC LIMIT 2;
