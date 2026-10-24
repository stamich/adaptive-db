# Physical plan wire format v3

The JVM planner sends physical plans to the native engine as UTF-8 JSON through
`adb_execute_plan_json[_at]`. Since version 2 (Milestone 2.1) the plan is wrapped in an envelope;
version 3 (Milestone 2.3.0) adds the index operators `index_scan` and `row_fetch`:

```json
{"wire_version": 3, "plan": { ...plan node... }}
```

Only the current version is accepted: the JVM and the engine are upgraded together, and a
version-2 document from an older planner is reported as a version mismatch.

`adb-plan-wire::decode_json` is the only native entry point. It

1. rejects empty documents and documents over **8 MiB**;
2. reads `wire_version` **before** interpreting the plan: a 2.0-style bare plan is reported as
   "unsupported plan wire version 1", not as a confusing parse error;
3. rejects unknown fields anywhere (`deny_unknown_fields`), so a producer speaking another dialect
   is never half-understood;
4. validates the decoded plan (limits and slot consistency, see [execution.md](execution.md)).

`encode_json` is the symmetric, validating producer (Rust tools and tests). The Scala producer is
`io.adb.physical.PlanJsonEncoder`; the fixtures `crates/adb-plan-wire/tests/fixtures/jvm-relational-plan.json`
and `jvm-index-plan.json` pin its output, and tests on both sides check them (cross-language
contract).

## Plan nodes (`"op"`)

| `op` | Fields |
|---|---|
| `point_lookup` | `row_id` (decimal string of the u128 `(entity << 64) \| pk`; small JSON numbers accepted), `columns` |
| `scan` | `columns` (every entity; diagnostics) |
| `entity_scan` | `entity_id`, `columns` |
| `filter` | `input`, `predicate` |
| `project` | `input`, `slots` (output order) |
| `limit` | `input`, `limit` (≤ 1,000,000) |
| `hash_join` | `left` (probe), `right` (build), `join_type` (`inner` \| `left`), `keys` `[{"left":s,"right":s}]`, optional `residual` |
| `nested_loop_join` | `left`, `right`, `join_type`, optional `predicate` (absent = cross join) |
| `aggregate` | `input`, `group_by` (slots), `aggregates` `[{"function":"count\|sum\|min\|max\|avg","input":s?,"output":s}]` (`input` omitted only for `COUNT(*)`) |
| `sort` | `input`, `keys` `[{"slot":s,"descending":bool}]` |
| `top_k` | `input`, `keys`, `limit` (≤ 1,000,000) |
| `index_scan` (v3) | `index_id`, `bounds`, `direction` (`forward` \| `backward`, default `forward`), `columns` (key or covered fields only; may be empty), optional `limit` (≤ 1,000,000; sizes the first page) |
| `row_fetch` (v3) | `input` (rows with row ids, e.g. an `index_scan`), `columns` (stored fields added to each row; slots must be new) |

`columns` is `[{"field_id": f, "slot": s}]`: the stored field `f` is read into slot `s`.

`bounds` of an `index_scan` is `{"prefix":[v...], "lower":{"value":v,"inclusive":b}, "upper":{...}}`:
the leading key columns equal `prefix`, the next key column lies between `lower` and `upper`
(each optional; omitted = unbounded). Values use the literal form below and are never `"Null"`.
The scan is exact for these columns (NULL keys never match a range), runs at the query snapshot,
and fails if the index cannot serve that snapshot (see [indexes.md](indexes.md)).

## Expressions (`"kind"`)

| `kind` | Fields |
|---|---|
| `slot` | `slot` |
| `literal` | `value`: `"Null"`, `{"Bool":b}`, `{"Int64":n}`, `{"Float64":x}` (finite), `{"String":s}`, `{"Bytes":[..]}` |
| `binary` | `left`, `op` (`eq ne lt le gt ge and or`), `right` |
| `not` | `expr` |

## Example

`SELECT c.city, SUM(o.amount) AS total FROM orders o JOIN customer c ON o.customer_id = c.id
GROUP BY c.city ORDER BY total DESC LIMIT 2`:

```json
{"wire_version":3,"plan":{"op":"top_k","limit":2,
  "keys":[{"slot":4,"descending":true}],
  "input":{"op":"aggregate","group_by":[3],
    "aggregates":[{"function":"sum","input":1,"output":4}],
    "input":{"op":"hash_join","join_type":"inner","keys":[{"left":0,"right":2}],
      "left":{"op":"entity_scan","entity_id":2,"columns":[{"field_id":2,"slot":0},{"field_id":3,"slot":1}]},
      "right":{"op":"entity_scan","entity_id":1,"columns":[{"field_id":1,"slot":2},{"field_id":3,"slot":3}]}}}}}
```

## History

* v1 (Milestones 1.7–2.0.3): bare plan, expressions and projections by storage `field_id`,
  operators `point_lookup`, `scan`, `entity_scan`, `filter`, `project`, `limit`.
* v2 (2.1): envelope, slots, joins, aggregation, sorting, unknown fields rejected.
* v3 (2.3.0): `index_scan` (bounds, direction, covered columns, limit hint) and `row_fetch`.

JSON remains a bootstrap protocol; `proto/` sketches the intended binary IR, which would decode
into the same Rust `PhysicalPlan`.
