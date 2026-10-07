# Adaptive DB — Milestone 2.0.3

Adaptive DB to baza danych, w której **kanoniczny log jest źródłem prawdy**, a fizyczne struktury
(current store, version store, w przyszłości projekcje kolumnowe, wyszukiwawcze i grafowe) są
odtwarzalnymi projekcjami tego logu. Koncepcja całości jest w [docs/concept/AdaptiveDB_1_3_baza_nowej_generacji.md](docs/concept/AdaptiveDB_1_3_baza_nowej_generacji.md).
To repozytorium realizuje jej rdzeń V1: jednowęzłowy silnik transakcyjny z historią i natywnym
strumieniem zmian.

## Co wnosi 2.0.3

| Obszar | 2.0.2 | 2.0.3 |
|---|---|---|
| Źródło prawdy | store'y; WAL przycinany po checkpoincie | **log, nigdy nieprzycinany**; store'y odtwarzalne (`rebuild_projections`) |
| Trwałość commitu | 6 fsync na commit, globalnie serializowane | WAL fsync + **group commit**; strony store'ów tylko w checkpointach |
| Checkpoint / torn pages | zapis stron w miejscu, bez ochrony | **atomowy dziennik (doublewrite)** dla wszystkich plików naraz |
| Błąd po zapisie do WAL | klient dostaje błąd, stan w pamięci rozjechany | **poisoning** + `CommitOutcomeUnknown`, rozstrzyga recovery |
| Current heap | każdy UPDATE zostawiał martwą krotkę | zwalnianie slotów, kompaktowanie, mapa wolnego miejsca; **`vacuum()`** tombstone'ów |
| Skan tabeli | skan całej bazy + filtr, materializacja w `Vec` | **`EntityScan`**: zakres kluczy encji, stronicowany strumieniowo |
| Izolacja | Snapshot (write skew możliwy) | **Serializable domyślnie** (walidacja read-setu), Snapshot opcjonalnie |
| CDC | brak | **natywny strumień zmian** z logu: before/after, kursory, filtry, trwałe offsety konsumentów |
| C ABI | v2 | **v3** (CDC, checkpoint, vacuum, statusy `POISONED`, `LOG_TRUNCATED`) |
| B+Tree | dwie prawie identyczne implementacje | **jedna generyczna** `BPlusTree<K>` (ten sam format na dysku) |
| Testy Rust | 30 | **113** |

Pomiary (`examples/rust-benchmark`, 5 000 wierszy na encję, ta sama maszyna):

| Miara | 2.0.2 | 2.0.3 |
|---|---:|---:|
| commit jednowierszowy, 1 wątek | 522 /s | ~3 500–3 900 /s |
| commit jednowierszowy, 8 wątków | 459 /s | ~9 900 /s |
| ładowanie (100 wierszy / tx) | 21 147 wierszy/s | 48 189 wierszy/s |
| skan jednej encji z 3 (5 000 wierszy) | 71 ms | 2,9 ms |
| strony heapu po 2 000 aktualizacjach 100 wierszy | 1 → 7 | 1 → 1 |

## Architektura

```text
SQL -> Scala (parser, binder, planner) -> PhysicalPlan JSON -> Java FFM -> C ABI v3 -> Rust
                                                                                      |
  commit: validate -> append to LOG -> apply to projections (memory) -> group fsync -> publish
  read:   snapshot over current + version projections; EntityScan = key-range scan
  CDC:    WalCursor over the durable log prefix
  checkpoint: dirty pages + roots + record -> journal -> in place
```

Szczegóły: [docs/architecture.md](docs/architecture.md), niezmienniki z testami:
[docs/invariants.md](docs/invariants.md), CDC: [docs/cdc.md](docs/cdc.md), ABI:
[docs/ffi.md](docs/ffi.md), format planu: [docs/plan-wire-format.md](docs/plan-wire-format.md),
roadmapa: [docs/roadmap.md](docs/roadmap.md).

## Użycie (Rust)

```rust
use adb_core::{Row, RowId, Value};
use adb_engine::{ChangeCursor, ChangeFilter, Database};

let db = Database::open("data")?;

let mut tx = db.begin();                                   // Serializable
let id = RowId::compose(/* entity */ 1, /* pk */ 42);
if db.get_in_tx(&mut tx, id)?.is_none() {
    tx.put(id, Row::new().with_field(1, Value::Int64(100)));
}
let ts = db.commit(tx)?;                                   // durable po powrocie

let old = db.get_at(id, ts)?;                              // time travel
let changes = db.read_changes(ChangeCursor::BEGINNING, 100, &ChangeFilter::entities([1]))?;
db.vacuum()?;
db.close()?;                                               // checkpoint (opcjonalny)
```

## Obsługiwany SQL (warstwa JVM, bez zmian względem 2.0)

```sql
CREATE TABLE account (id BIGINT PRIMARY KEY, balance BIGINT NOT NULL, owner STRING);
INSERT INTO account VALUES (1, 100, 'Alice');
SELECT id, balance FROM account WHERE balance > 50 LIMIT 10;   -- EntityScan
SELECT * FROM account AS OF VERSION 3 WHERE id = 1;            -- PointLookup w snapshocie
UPDATE account SET balance = 200 WHERE id = 1;
DELETE FROM account WHERE id = 1;
EXPLAIN SELECT * FROM account WHERE id = 1;
```

## Budowanie i walidacja

```bash
./build-milestone2.0.3.sh          # fmt, clippy -D warnings, testy, rustdoc, benchmark, smoke JVM
```

Wymagania: Rust (stable), JDK 22+. Gradle 9 i dostęp do Maven Central są potrzebne tylko do
testów i dema warstwy Scala/JVM (`cd jvm && gradle clean test`, `./demo/run-demo.sh`). Ścieżka
Java FFM → Rust jest sprawdzana bez Gradle: `examples/jvm-cdc-smoke/run.sh`.

## Układ repozytorium

```text
crates/        silnik Rust (core, journal, page, buffer, btree, storage, wal, tx, execution, engine, ffi)
include/adb.h  C ABI v3
jvm/           Scala/Java: model, katalog, SQL, planowanie, FFM, gateway, CLI
proto/         docelowy binarny kontrakt planu
examples/      benchmark Rust, smoke test JVM CDC, wyniki
demo/          przegląd funkcji przez CLI JVM
docs/          architektura, niezmienniki, CDC, ABI; docs/history — artefakty starszych milestone'ów
```
