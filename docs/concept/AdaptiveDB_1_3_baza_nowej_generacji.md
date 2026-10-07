Rozwiń punkt 18. Jak wyglądałaby baza nowej generacji?

Baza nowej generacji nie powinna być „jedną bazą, która zna wszystkie typy danych”. To za mało. Powinna być **systemem, który oddziela logiczny model danych od fizycznego sposobu ich przechowywania i sam optymalizuje storage, partycjonowanie, indeksy, replikację i consistency pod rzeczywisty workload**.

Najkrócej:

```text
Application
    ↓
Logical Data Model
    ↓
Semantic / Intent Layer
    ↓
Adaptive Planner
    ↓
Physical Representations
    ↓
Distributed Storage
```

To byłaby fundamentalna zmiana względem dzisiejszego:

```text
developer
   ↓
chooses PostgreSQL / MongoDB / Cassandra / Neo4j / Elasticsearch
   ↓
adapts application to database
```

na:

```text
developer
   ↓
describes semantics and requirements
   ↓
database adapts itself
```

## 1. Jeden model logiczny, wiele modeli fizycznych

Dzisiaj wybierasz zwykle jeden dominujący sposób reprezentacji.

Relacyjny:

```text
Customer
   |
   +---- Order
            |
            +---- OrderItem
```

Dokumentowy:

```json
{
  "customerId": 10,
  "orders": [...]
}
```

Grafowy:

```text
Customer --PLACED--> Order --CONTAINS--> Product
```

Nowa baza powinna widzieć to jako **ten sam model danych**.

Na poziomie logicznym moglibyśmy mieć:

```text
ENTITY Customer {
    id: UUID
    email: Email
    name: String
}

ENTITY Order {
    id: UUID
    customer: REF Customer
    createdAt: Timestamp
    status: OrderStatus
}

ENTITY Product {
    id: UUID
    name: String
}

RELATION Order CONTAINS Product
```

A fizycznie baza mogłaby utworzyć:

```text
Customer → row store
Order → row store

Customer.orders → document-like projection

Order → Product
        graph projection

Order.createdAt
        columnar projection

Product.description
        inverted search index

Product.embedding
        vector index
```

Aplikacja nie musi wiedzieć, gdzie i jak to leży.

---

# 2. Canonical data + projections

Kluczowy element architektury:

> tylko jedna reprezentacja powinna być źródłem prawdy.

Nazwijmy ją:

```text
Canonical State
```

lub jeszcze lepiej:

```text
Canonical Transaction Log
```

Architektura:

```text
                        CANONICAL LOG
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
          Row Store      Column Store       Search
              │               │               │
              ├───────────────┼───────────────┤
              ▼               ▼               ▼
             Graph           Vector          Cache
```

Jeżeli coś stanie się z:

```text
vector index
```

nie tracimy danych.

Robimy:

```text
canonical log
     ↓
rebuild vector projection
```

Tak samo:

```text
graph index corrupted
       ↓
rebuild
```

To usuwa jeden z dużych problemów współczesnego polyglot persistence.

Dzisiaj często:

```text
PostgreSQL
    ↓ CDC
Kafka
    ↓
Elasticsearch
    ↓
Redis
    ↓
Vector DB
```

i pojawia się pytanie:

```text
który system mówi prawdę?
```

W nowym modelu odpowiedź byłaby zawsze jednoznaczna.

---

# 3. Log jako fundament storage engine

Można pójść dalej i oprzeć bazę o append-only log.

Na przykład:

```text
TX 1001:
Order/10 created

TX 1002:
Order/10.status = PAID

TX 1003:
Order/10.status = SHIPPED
```

Log:

```text
offset 1001 ──► CREATE Order/10
offset 1002 ──► status PAID
offset 1003 ──► status SHIPPED
```

Z niego można zbudować aktualny stan:

```text
Order/10
status = SHIPPED
```

oraz historię:

```text
CREATED → PAID → SHIPPED
```

To daje jednocześnie elementy:

```text
WAL
+
CDC
+
event sourcing
+
temporal database
+
replication log
```

w jednym mechanizmie.

Nie znaczy to jednak, że aplikacja musi używać event sourcingu.

To może być wyłącznie wewnętrzny model storage engine.

---

# 4. Aktualny stan powinien być oddzielony od historii

To bezpośrednio wynika z problemów PostgreSQL MVCC.

Nie chciałbym:

```text
current tuple
old tuple
old tuple
old tuple
current tuple
dead tuple
```

w jednym podstawowym heapie.

Zamiast tego:

```text
                   Logical Data
                        │
              ┌─────────┴─────────┐
              ▼                   ▼
       CURRENT STATE         VERSION HISTORY
              │                   │
          optimized            compressed
          for OLTP             sequential
              │                   │
             NVMe            SSD/Object Store
```

Current store:

```text
Order 10 → SHIPPED
Order 11 → PAID
Order 12 → CREATED
```

History:

```text
Order 10:
CREATED
PAID
SHIPPED
```

Wtedy zwykłe:

```sql
SELECT *
FROM orders
WHERE id = 10;
```

nie musi przechodzić przez stare wersje.

Ale:

```sql
SELECT *
FROM orders
AS OF TIMESTAMP '2026-01-01 12:00';
```

może korzystać z version store.

---

# 5. Immutable history + mutable current state

To ciekawy kompromis.

Możemy potraktować bazę jako dwa światy:

```text
CURRENT
mutable
optimized for latency

HISTORY
immutable
optimized for compression/scans
```

Czyli:

```text
UPDATE account
SET balance = 200
```

powodowałby:

```text
old current value
        ↓
append to history

new current value
        ↓
replace current state
```

Nie musimy przechowywać dziesięciu historycznych wersji w primary OLTP structure.

---

# 6. Row store + column store jednocześnie

Jedna z kluczowych funkcji nowej generacji powinna być natywnym HTAP.

OLTP najlepiej obsługuje:

```text
ROW

[id=1,name=John,balance=100]
[id=2,name=Anna,balance=500]
```

Zapytanie:

```sql
SELECT *
FROM account
WHERE id = 1;
```

Column store:

```text
id:
1
2
3
4

balance:
100
500
300
900
```

jest lepszy dla:

```sql
SELECT country, SUM(amount)
FROM orders
GROUP BY country;
```

Nowa baza mogłaby mieć:

```text
             Orders
                │
       canonical changes
          /           \
         ▼             ▼
    Row Projection  Column Projection
         │             │
        OLTP           OLAP
```

Bez:

```text
Postgres
  ↓
ETL
  ↓
warehouse
```

---

# 7. Ale nie synchronizowałbym obu reprezentacji transakcyjnie

To byłby poważny błąd.

Jeżeli INSERT miałby zrobić synchronicznie:

```text
row write
+
column update
+
graph update
+
search update
+
vector update
```

to pojedynczy zapis robi się gigantyczną transakcją.

Zamiast:

```text
canonical state = strongly consistent
```

a projekcje:

```text
row projection        STRONG
unique index          STRONG

column projection     <500 ms stale
search index          <1 s stale
vector index          <10 s stale
```

To musi być jawnie zdefiniowane.

---

# 8. Consistency nie jako ustawienie całej bazy

Dzisiejsze pytania często brzmią:

> czy baza jest strongly consistent czy eventually consistent?

To jest zbyt prymitywne.

W tej samej aplikacji:

```text
Account.balance
```

wymaga:

```text
STRONG
```

ale:

```text
User.lastSeen
```

może mieć:

```text
EVENTUAL
```

A:

```text
recommendations
```

mogą mieć:

```text
BOUNDED_STALENESS 30s
```

Dlatego model mógłby wyglądać:

```text
ENTITY Account {

    id UUID

    balance Decimal {
        consistency = SERIALIZABLE
        durability = SYNC
    }

    lastViewed Timestamp {
        consistency = EVENTUAL
    }
}
```

To byłby ogromny krok naprzód.

---

# 9. Consistency classes

Dałbym kilka jasno zdefiniowanych modeli:

```text
LINEARIZABLE
SERIALIZABLE
SNAPSHOT
CAUSAL
BOUNDED_STALENESS
EVENTUAL
```

Nie setki subtelnych kombinacji.

Przykład:

```text
email:
    LINEARIZABLE
    UNIQUE

profileBio:
    CAUSAL

lastSeen:
    EVENTUAL

recommendationScore:
    BOUNDED_STALENESS(30s)
```

Baza może wtedy optymalizować fizyczne wykonanie.

---

# 10. Constraints muszą pozostać deterministyczne

Nowa baza powinna mieć bardzo mocny system invariants.

Na przykład:

```text
Account.balance >= 0
```

albo:

```text
User.email UNIQUE
```

albo:

```text
Order.customer EXISTS
```

Nie pozwalałbym optimizerowi rozluźniać takich reguł.

Czyli mamy podział:

```text
HARD SEMANTICS

PK
FK
UNIQUE
CHECK
transaction invariants

        versus

SOFT PHYSICAL DESIGN

index
replica
partition
cache
projection
storage tier
```

Pierwsza grupa:

```text
cannot change automatically
```

Druga:

```text
can change constantly
```

To bardzo ważna granica.

---

# 11. Adaptive partitioning

Tu wychodzimy poza MongoDB/Cassandra.

Dzisiaj:

```text
developer chooses shard key
```

Nowy model:

```text
database observes workload
```

Przykładowo:

```text
queries:

70% customerId
20% orderId
10% createdAt
```

System może zdecydować:

```text
primary locality:
hash(customerId)
```

Po roku:

```text
queries:

30% customerId
50% createdAt
20% analytics
```

System może zmienić layout.

Nie przez wielką jednorazową operację:

```text
RESHARD EVERYTHING
```

ale stopniowo:

```text
layout v1 ────────┐
                  │ migrate
                  ▼
layout v2 ────────────────
```

---

# 12. Logical partition IDs zamiast fizycznego shard key

To bardzo ważny szczegół implementacyjny.

Aplikacja nie powinna znać:

```text
customerId → shard 7
```

Można wprowadzić:

```text
logical partition
```

np.:

```text
LP-10001
```

Mapowanie:

```text
LP-10001
    ↓
physical node A
```

później:

```text
LP-10001
    ↓
node C
```

bez zmiany aplikacji.

Jeszcze później:

```text
LP-10001
   ├── subpartition 1 → node A
   ├── subpartition 2 → node C
   └── subpartition 3 → node D
```

Router zna mapowanie.

Aplikacja nie.

---

# 13. Hot partition splitting

Załóżmy:

```text
tenant 100
```

ma:

```text
5 requests/s
```

i cały mieści się:

```text
one partition
```

Ale tenant 999 ma:

```text
200 000 req/s
```

Zamiast zmuszać wszystkie tenanty do bucketowania:

```text
tenantId + bucket
```

system może automatycznie zrobić:

```text
tenant 100
   ↓
P17

tenant 999
   ↓
┌────┬────┬────┬────┐
P81 P82 P83 P84
```

Logicznie nadal:

```text
tenantId = 999
```

To byłby **elastic partition key**.

---

# 14. Transaction Affinity Graph

To jeden z najciekawszych elementów.

System może analizować:

```text
które dane często uczestniczą razem w transakcjach?
```

Przykład:

```text
TX1 → Account A + Ledger A
TX2 → Account A + Ledger A
TX3 → Account A + Ledger A
TX4 → Account B + Ledger B
```

Możemy zbudować graf:

```text
Account A ===== Ledger A

Account B ===== Ledger B
```

Im częściej dwa obiekty uczestniczą razem:

```text
stronger edge
```

Planner próbuje je colocate.

Czyli fizyczny placement może być wyliczany z:

```text
transaction affinity graph
```

nie tylko z jednego shard key.

---

# 15. Cel: maksymalizować local transactions

Załóżmy:

```text
1 million transactions/s
```

Jeżeli:

```text
99.7%
```

możemy wykonać na jednym replication group, mamy ogromną korzyść.

Distributed consensus potrzebny jest tylko dla:

```text
0.3%
```

Zamiast:

```text
every transaction
       ↓
global coordination
```

mamy:

```text
common case
       ↓
local consensus

rare case
       ↓
distributed transaction
```

To powinno być jedną z podstawowych funkcji optimizer-a.

---

# 16. Distributed transactions nadal muszą istnieć

Nie próbowałbym ich usunąć.

To częsty błąd systemów NoSQL:

```text
cross-partition transaction is hard
        ↓
don't support it
        ↓
application handles consistency
```

Efekt:

```text
Saga
outbox
compensation
retry
deduplication
business inconsistency
```

wszędzie w aplikacji.

Nowa baza powinna oferować:

```text
BEGIN DISTRIBUTED TRANSACTION
```

ale sprawiać, żeby było to:

```text
rare
```

a nie:

```text
impossible
```

---

# 17. Global distributed transaction

Przykład:

```text
Warsaw
Account A

Frankfurt
Account B
```

Transakcja:

```text
A -= 100
B += 100
```

system powinien móc wykonać poprzez consensus / transaction protocol.

Ale powinien też jasno pokazać:

```text
latency = 37 ms

reason:
cross-region synchronous commit
```

To prowadzi do ważnej rzeczy.

---

# 18. Database powinna pokazywać koszt semantyki

Dzisiaj developer często widzi:

```sql
COMMIT;
```

ale nie wie:

```text
co to naprawdę kosztuje?
```

Nowa baza mogłaby pokazywać:

```text
Transaction SLA analysis

participants: 3 partitions
regions: Warsaw, Frankfurt
consistency: SERIALIZABLE

estimated minimum latency:
18–32 ms
```

To byłaby ogromna pomoc projektowa.

---

# 19. Intent-driven schema

Zamiast mówić bazie:

```sql
CREATE INDEX idx_user_email ON users(email);
```

mówimy:

```text
ACCESS PATTERN userByEmail {
    filter = email
    expectedQPS = 50_000
    p99 = 5ms
}
```

System dobiera:

```text
hash index?
B-tree?
cache?
replica?
partition?
```

A developer definiuje:

```text
what
```

nie:

```text
how
```

---

# 20. Query SLA jako część schematu

Przykład:

```text
QUERY PROFILE FindCustomerByEmail {

    p99 < 5ms

    availability = 99.99%

    consistency = LINEARIZABLE
}
```

Baza może na tej podstawie zdecydować:

```text
index
+
replica placement
+
cache
+
partition strategy
```

Jeśli nie da się spełnić SLA, powinna powiedzieć:

```text
Impossible under current topology.

Reason:
cross-region linearizable read
```

zamiast udawać, że wszystko działa.

---

# 21. Query planner nowej generacji

Dzisiejszy optimizer robi głównie:

```text
SQL
 ↓
logical plan
 ↓
physical plan
```

np.:

```text
hash join
index scan
seq scan
nested loop
```

Nowy planner miałby dużo większy zakres.

```text
                 Semantic Planner
                       │
         ┌─────────────┼─────────────┐
         ▼             ▼             ▼
      Query         Storage       Placement
     Planner        Planner        Planner
         │             │             │
         ▼             ▼             ▼
      indexes      row/column       shards
      joins        compression      replicas
```

---

# 22. Physical Design Optimizer

System stale analizowałby:

```text
queries
writes
cardinality
latency
storage
hot keys
transaction graph
network topology
hardware cost
```

i decydował:

```text
CREATE index
DROP index

CREATE projection
DROP projection

split partition
merge partition

move replica
change compression

promote data RAM→NVMe
demote NVMe→object store
```

Bez ręcznego DBA dla większości przypadków.

---

# 23. Ale każda automatyczna zmiana musi być odwracalna

To absolutnie konieczne.

Planner robi:

```text
layout A
   ↓
layout B
```

i nagle:

```text
p99 latency +40%
```

System powinien mieć:

```text
rollback layout
```

Tak jak query optimizer może wybrać inny plan.

Czyli:

```text
physical design
```

staje się wersjonowany.

Na przykład:

```text
Design v51
Design v52
Design v53
```

---

# 24. Shadow optimization

Jeszcze lepiej:

zanim nowy layout stanie się aktywny:

```text
current layout
       │
       ├── real workload
       │
       └── shadow evaluation
                ↓
          candidate layout
```

Planner może porównać:

```text
CPU
latency
network
storage
```

i dopiero potem zrobić cutover.

Analogicznie do canary deployment.

---

# 25. Jeden język zapytań

Nie tworzyłbym pięciu całkowicie oddzielnych API.

Bazą powinien pozostać SQL lub SQL-like language.

Przykład relacyjny:

```sql
SELECT *
FROM Orders
WHERE customerId = ?;
```

Graph:

```sql
MATCH
    (c:Customer)-[:PLACED]->(o:Order)-[:CONTAINS]->(p:Product)
WHERE c.id = ?
RETURN p;
```

Full-text:

```sql
SELECT *
FROM Product
WHERE SEARCH(description, 'distributed database');
```

Vector:

```sql
SELECT *
FROM Product
ORDER BY
    VECTOR_DISTANCE(embedding, :queryEmbedding)
LIMIT 10;
```

Temporal:

```sql
SELECT *
FROM Account
AS OF TIMESTAMP '2026-08-01 10:00';
```

Jedna transakcyjna semantyka.

---

# 26. Relational model nadal jako fundament

Mimo całego multi-model podejścia nie porzucałbym modelu relacyjnego.

Powód jest prosty:

```text
relations
constraints
algebra
```

są niezwykle uniwersalne.

Raczej:

```text
Relational Core
     +
nested/document types
     +
graph edges
     +
vector values
```

Czyli bardziej:

```text
PostgreSQL-type model expanded
```

niż:

```text
MongoDB plus SQL syntax
```

---

# 27. Document jako wartość pierwszej klasy

Powinniśmy jednak pozwolić na:

```text
STRUCT
ARRAY
MAP
```

bez tworzenia dziesięciu tabel.

Przykład:

```text
Customer {
    id UUID,
    name String,

    address {
        city String,
        country String
    },

    preferences Map<String,String>
}
```

Czyli:

```text
normalized where useful
nested where useful
```

bez ideologii:

> wszystko musi być tabelą

albo:

> wszystko musi być dokumentem.

---

# 28. Graph edges jako first-class data

Przykład:

```text
RELATION FOLLOWS(
    Person,
    Person
)
```

czy:

```text
RELATION OWNS(
    Customer,
    Account
)
```

Edge może mieć własne dane:

```text
OWNS {
    since Timestamp
    role OwnershipType
}
```

Storage engine może zdecydować:

```text
adjacency index
```

jeżeli traversal jest częsty.

Ale jeśli graph queries nie występują:

```text
no graph projection
```

Czyli płacimy za feature tylko wtedy, gdy jest potrzebny.

---

# 29. Search też jako projection

Pole:

```text
Product.description
```

można oznaczyć:

```text
SEARCHABLE
```

System tworzy inverted index tylko wtedy, kiedy workload tego wymaga.

Przykład:

```text
TEXT {
    tokenizer = english
}
```

ale fizyczna reprezentacja jest pochodną.

---

# 30. Vector jako normalny typ

Na przykład:

```text
embedding VECTOR<FLOAT32, 1536>
```

Można zrobić:

```sql
SELECT id
FROM articles
ORDER BY embedding <-> :query
LIMIT 20;
```

System może wybrać:

```text
HNSW
IVF
disk ANN
brute-force SIMD
```

zależnie od:

```text
dataset size
accuracy SLA
latency SLA
memory
```

A nie developer.

---

# 31. Indeks nie powinien być permanentną strukturą

Dzisiaj:

```text
CREATE INDEX
```

często zostaje na lata.

Nowa baza mogłaby traktować indeks jak cache:

```text
workload needs it
       ↓
create

workload disappears
       ↓
drop
```

Oczywiście nie dotyczy to indeksów realizujących correctness, np.:

```text
UNIQUE
```

To ważne rozróżnienie:

```text
semantic index
```

versus:

```text
performance index
```

---

# 32. Data tiering

Storage:

```text
RAM
PMEM / future NVRAM
NVMe
SSD
Object Storage
Archive
```

Planner powinien automatycznie decydować.

Na przykład:

```text
Orders

0–5 min      RAM/cache
0–30 days    NVMe row store
30d–2y       columnar SSD
>2y          compressed object storage
```

A użytkownik nadal:

```sql
SELECT *
FROM Orders;
```

---

# 33. Query może przechodzić przez wiele tiers

Zapytanie:

```sql
SELECT SUM(amount)
FROM Orders
WHERE createdAt >= now() - interval '5 years';
```

planner:

```text
2026 data → NVMe
2025 data → SSD
2021–2024 → Object Store
```

wykona:

```text
parallel scans
       ↓
aggregate
       ↓
merge
```

Bez ręcznego:

```text
orders_hot
orders_archive
orders_s3
```

w aplikacji.

---

# 34. Retention policy jako część modelu

Przykład:

```text
Order.currentState:
    forever

Order.history:
    7 years

RequestLog:
    30 days
```

Baza może automatycznie:

```text
compress
archive
delete
```

bez osobnych cronów.

---

# 35. Temporal database by default

Każda zmiana może mieć:

```text
validFrom
validTo
transactionTime
```

Nie musisz budować ręcznie:

```text
audit_table
history_table
trigger
```

Zapytania:

```sql
SELECT *
FROM Employee
AS OF '2025-01-01';
```

albo:

```sql
SELECT HISTORY(balance)
FROM Account
WHERE id=100;
```

To wynika naturalnie z version store.

---

# 36. CDC jako natywna funkcja

Skoro istnieje canonical log:

```text
consumer
   ↓
SUBSCRIBE Orders
```

może dostawać:

```text
OrderCreated
OrderUpdated
OrderDeleted
```

bez:

```text
Debezium parsing WAL
```

Czyli baza ma natywny:

```text
transactional change stream
```

---

# 37. Kafka nie znika

To ważne.

Nie próbowałbym zrobić:

> baza zastępuje Kafka.

Kafka ma inne zastosowanie.

Natomiast prosty pattern:

```text
DB
 ↓
outbox table
 ↓
Debezium
 ↓
Kafka
```

mógłby stać się:

```text
COMMIT transaction
      ↓
canonical change log
      ↓
publish transactional stream
```

Czyli outbox staje się natywną właściwością bazy.

---

# 38. Exactly-once identifiers

Każda transakcja miałaby globalny:

```text
TransactionID
```

Każda zmiana:

```text
ChangeID
```

Subscriber może zapamiętać:

```text
lastProcessedOffset
```

co znacznie ułatwia:

```text
deduplication
replay
idempotency
```

---

# 39. Global IDs

Nie polegałbym na:

```text
auto_increment BIGINT
```

w distributed world.

Podstawowe ID powinny być czymś w rodzaju:

```text
128-bit sortable IDs
```

łącząc:

```text
time
+
randomness
```

czyli konceptualnie rodzina UUIDv7.

Daje to:

```text
global uniqueness
+
rough chronological ordering
```

bez centralnego sequence server.

---

# 40. Replikacja jako osobny wymiar

Sharding odpowiada:

```text
gdzie znajduje się primary ownership?
```

Replication:

```text
gdzie powinny być kopie?
```

Nowy planner optymalizuje oba.

Przykład:

```text
dataset: European customers

write primary:
Frankfurt

read replicas:
Warsaw
Paris
Amsterdam
```

Ale:

```text
Japanese users
```

mogą mieć inne placement.

---

# 41. Geo constraints

Aplikacja może zadeklarować:

```text
Customer.personalData {
    residency = EU
}
```

Planner nie może wtedy przenieść danych:

```text
EU → US
```

nawet jeśli byłoby to szybsze.

Czyli fizyczne planowanie musi uwzględniać:

```text
security
compliance
residency
cost
latency
```

jednocześnie.

---

# 42. Failure domains jako first-class concept

Database powinna wiedzieć:

```text
node
rack
availability zone
region
cloud provider
```

Replica placement:

```text
replica A → AZ1
replica B → AZ2
replica C → AZ3
```

a nie:

```text
three replicas accidentally on one rack
```

---

# 43. Storage topology jako graf

Planner może widzieć infrastrukturę:

```text
Warsaw
  │ 12 ms
Frankfurt
  │ 80 ms
New York
  │ 140 ms
Tokyo
```

oraz:

```text
NVMe cost
object cost
network cost
```

i wykonywać placement jako problem optymalizacyjny.

---

# 44. Cost model nowej generacji

Dzisiejszy optimizer liczy:

```text
CPU cost
I/O cost
rows
```

Nowy:

```text
CPU
RAM
NVMe IOPS
network latency
network cost
cross-region traffic
storage cost
consensus cost
carbon/energy optionally
```

Czyli fizyczny plan może być:

```text
fastest
```

albo:

```text
cheapest while p99 < 20ms
```

---

# 45. Workload classes

Można deklarować:

```text
PAYMENTS:
    priority = CRITICAL

REPORTING:
    priority = LOW

RECOMMENDATIONS:
    priority = MEDIUM
```

Przy przeciążeniu:

```text
payments continue
reporting throttled
```

zamiast losowej degradacji wszystkich workloadów.

---

# 46. Admission control

Nowoczesna baza powinna powiedzieć:

```text
This query would scan 42 TB
Estimated cost: high
```

i np.:

```text
reject
queue
run on analytics replicas
```

zamiast pozwolić jednemu:

```sql
SELECT ...
```

zabić OLTP.

---

# 47. Resource isolation

Logicznie jedna baza, ale:

```text
OLTP pool
Analytics pool
Search pool
Maintenance pool
```

Powinny mieć osobne limity:

```text
CPU
RAM
I/O
network
```

To chroni bazę przed „noisy neighbor”.

---

# 48. Nie zrobiłbym z niej jednego gigantycznego procesu

Architektura mogłaby być modułowa:

```text
                 Control Plane
                      │
        ┌─────────────┼─────────────┐
        ▼             ▼             ▼
 Transaction      Query          Placement
  Service         Service         Service

        │
─────────────────────────────────────────
               Data Plane
─────────────────────────────────────────
        │             │             │
      Node A        Node B        Node C
```

Ale critical data path powinien być możliwie prosty.

---

# 49. Control plane i data plane

To bardzo ważne rozdzielenie.

## Data plane

obsługuje:

```text
GET
PUT
SELECT
COMMIT
```

musi być:

```text
fast
deterministic
small
```

## Control plane

robi:

```text
resharding
index recommendation
replica movement
workload analysis
ML optimization
```

Może być dużo bardziej złożony.

Jeśli control plane padnie:

```text
database continues serving traffic
```

To eliminuje ogromną kategorię awarii.

---

# 50. AI może działać tylko w control plane

To miejsce dla AI/ML.

AI może analizować:

```text
query workload
hot partitions
indexes
capacity
```

i zaproponować:

```text
candidate layout
```

Ale nie chciałbym:

```text
LLM
 ↓
decides whether transaction commits
```

Nigdy.

Data plane powinien pozostawać:

```text
formal
deterministic
verifiable
```

---

# 51. Self-tuning, nie self-randomizing

Automatyczny optimizer musi mieć ograniczenia.

Każda zmiana:

```text
candidate
  ↓
simulate
  ↓
shadow
  ↓
canary
  ↓
measure
  ↓
promote
```

Nigdy:

```text
AI thinks new partitioning seems good
         ↓
move 100 TB
```

---

# 52. Explain powinien obejmować całą architekturę

Dzisiaj:

```sql
EXPLAIN ANALYZE
```

pokazuje plan query.

Nowa wersja mogłaby mieć:

```text
EXPLAIN SYSTEM
```

i pokazać:

```text
Query:
Find customer orders

Routing:
1 partition

Region:
Warsaw

Consistency:
Snapshot

Index:
customer_order_idx

Replica:
pl-waw-02

Estimated latency:
2.7 ms

Reason for placement:
82% customer affinity
```

To byłoby bardzo wartościowe dla Senior/Staff engineerów i DBA.

---

# 53. Powinna istnieć możliwość ręcznego override

Automatyzacja nie może oznaczać:

```text
operator has no control
```

Można powiedzieć:

```text
PIN partition TO region EU
```

albo:

```text
DO NOT AUTO DROP INDEX x
```

albo:

```text
PARTITION STRATEGY = MANUAL
```

Czyli:

```text
automatic by default
manual when required
```

---

# 54. Co z CAP?

Nowa baza nie może go oszukać.

Załóżmy:

```text
Warsaw ↔ Frankfurt
```

network partition.

Dla danych:

```text
consistency = LINEARIZABLE
```

jeden side może musieć przestać przyjmować writes.

Dla:

```text
consistency = EVENTUAL
```

oba mogą kontynuować.

Dlatego API powinno ujawniać te decyzje.

Nie:

```text
magical globally consistent database
```

---

# 55. PACELC jest jeszcze ważniejsze

Nawet bez awarii:

```text
Else:
Latency vs Consistency
```

Globalny strongly consistent write:

```text
Warsaw
   ↔
Frankfurt
   ↔
New York
```

ma koszt RTT.

Nie da się tego „zoptymalizować do zera”.

Nowa baza powinna pomagać developerowi świadomie wybrać semantykę.

---

# 56. Co dzieje się przy INSERT

Przykład:

```sql
INSERT INTO Orders(...)
```

Flow:

```text
Application
    ↓
Transaction Coordinator
    ↓
validate constraints
    ↓
choose owning partition
    ↓
replication group consensus
    ↓
append canonical log
    ↓
update current state
    ↓
COMMIT
```

Asynchronicznie:

```text
canonical log
    ├─► column projection
    ├─► search index
    ├─► graph projection
    └─► analytics
```

To jest bardzo czysty model.

---

# 57. UPDATE

```sql
UPDATE Orders
SET status='SHIPPED'
WHERE id=100;
```

Flow:

```text
Current:
status=PAID

        ↓

transaction

        ↓

History:
PAID

Current:
SHIPPED
```

Nie:

```text
heap:
PAID dead
SHIPPED live
```

w podstawowym current store.

---

# 58. DELETE

Nie musi oznaczać natychmiastowego fizycznego delete.

Może oznaczać:

```text
tombstone in canonical log
```

Current store:

```text
record disappears
```

History:

```text
retained according to policy
```

Po:

```text
retention expired
```

fizyczne GC.

---

# 59. Garbage collection staje się prostszy semantycznie

Zamiast pytać:

```text
which tuples are no longer visible to any snapshot?
```

możemy mieć:

```text
current store
```

bez historycznych wersji.

Version store:

```text
immutable segments
```

GC może działać segmentowo:

```text
segment 2024-01
retention expired
     ↓
drop segment
```

To może być dużo tańsze niż sprzątanie pojedynczych tuples.

---

# 60. Segment-based storage

History i analytics mogłyby działać przez immutable segments:

```text
Segment 1001
Segment 1002
Segment 1003
```

Po zamknięciu:

```text
immutable
```

Można:

```text
compress
replicate
move
cache
delete
```

jako całość.

To znacznie upraszcza distributed storage.

---

# 61. LSM + B-tree hybrid

Nie zakładałbym jednego storage structure.

Write-heavy dataset:

```text
LSM-like
```

Read-heavy point lookup:

```text
B-tree-like
```

Archive:

```text
columnar segments
```

Planner może dobierać storage per dataset.

To ponownie realizuje zasadę:

> logical semantics != physical format.

---

# 62. Jeden dataset może zmienić engine

Dziś migracja:

```text
Postgres → Cassandra
```

jest projektem na miesiące.

W nowym systemie:

```text
Dataset Orders

physical engine v1:
B-tree row store

        ↓ workload changes

physical engine v2:
LSM row store
```

Aplikacja nie widzi zmiany.

To byłoby coś bardzo dużego.

---

# 63. Jaka byłaby największa wada takiej bazy?

Zdecydowanie:

```text
complexity
```

System wewnętrznie robi:

```text
transactions
consensus
adaptive placement
multiple representations
temporal history
HTAP
vector
search
graph
```

To ogromny obszar.

Jeżeli nie postawimy twardych granic, powstanie:

> baza, która teoretycznie robi wszystko, a praktycznie niczego nie robi najlepiej.

---

# 64. Dlatego wersja 1 powinna być znacznie mniejsza

Gdybym faktycznie rozpoczynał projekt, **nie implementowałbym od razu graph + vector + search + OLAP**.

Rdzeń V1:

```text
1. Relational logical model

2. ACID transactions

3. Canonical append log

4. Current-state store

5. Separate version store

6. Native horizontal partitioning

7. Automatic partition splitting

8. Transaction-affinity placement

9. Explicit consistency model

10. SQL
```

To samo w sobie byłoby bardzo ambitnym projektem.

---

# 65. V2

Dopiero:

```text
automatic indexes
adaptive repartitioning
geo replication
columnar projection
HTAP
```

---

# 66. V3

Potem:

```text
full-text projection
graph projection
vector indexes
storage tiering
automatic SLA optimization
```

---

# 67. Architektura końcowa

W uproszczeniu widzę ją tak:

```text
                           APPLICATIONS
                                │
                   SQL / Transaction API
                                │
                    ┌───────────▼───────────┐
                    │   SEMANTIC DATA MODEL │
                    │                       │
                    │ entities              │
                    │ relations             │
                    │ constraints           │
                    │ consistency           │
                    │ SLA                   │
                    │ retention             │
                    └───────────┬───────────┘
                                │
                    ┌───────────▼───────────┐
                    │    ADAPTIVE PLANNER   │
                    │                       │
                    │ query optimization    │
                    │ partitioning          │
                    │ affinity              │
                    │ indexes               │
                    │ replication           │
                    │ placement             │
                    └───────────┬───────────┘
                                │
                     TRANSACTION ENGINE
                                │
                   ┌────────────▼────────────┐
                   │    CANONICAL LOG        │
                   │    source of truth      │
                   └────────────┬────────────┘
                                │
          ┌─────────────────────┼──────────────────────┐
          │                     │                      │
          ▼                     ▼                      ▼
     CURRENT STATE        VERSION STORE          CHANGE STREAM
       Row Store          Temporal History
          │
          │
 ┌────────┼──────────┬────────────┬──────────────┐
 ▼        ▼          ▼            ▼              ▼
B-tree   Column     Search       Graph          Vector
Index    Projection Projection   Projection     Projection

──────────────────────────────────────────────────────────
                 DISTRIBUTED STORAGE
──────────────────────────────────────────────────────────

      Partition 1       Partition 2       Partition 3
        │ │ │             │ │ │             │ │ │
        replicas          replicas          replicas

──────────────────────────────────────────────────────────

     RAM  →  NVMe  →  SSD  →  Object Storage → Archive
```

## Najważniejsza idea

Gdybym miał streścić całą tę architekturę jednym zdaniem:

> **Nowa generacja baz danych powinna przestać wymagać od programisty projektowania fizycznego storage i zamiast tego pozwolić mu deklarować semantykę danych, invariants, access patterns i SLA, pozostawiając fizyczną organizację adaptacyjnemu silnikowi.**

A jeszcze krócej:

```text
Today:

Application
    adapts to
Database architecture


Next generation:

Database architecture
    adapts to
Application semantics
```

I właśnie **Canonical Log + Current Store + Version Store + Adaptive Partitioning + Transaction Affinity** uważam za najbardziej interesujący rdzeń tego pomysłu — znacznie bardziej niż samo dodanie SQL, JSON, graph i vector do jednego produktu.
