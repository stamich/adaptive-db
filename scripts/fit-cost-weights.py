#!/usr/bin/env python3
"""Fits the page, hash-row and random-access weights of the cost model to measured engine times.

Reads the `calibration.points` of an FFI benchmark report (scripts/benchmark-ffi.sh): for every
measured plan, the unweighted cost components of its root (`cpu`, `io`, `memory_bytes`,
`hash_rows`, `random_io`) and its median engine time (`engine_ms`, prepared-plan path, so planning is
excluded). The points include scan-only plans (nothing hashed), join plans and, since 2.3.0,
index plans of the selectivity sweep (random accesses), so row operations, hashed rows and
random accesses can be told apart.

Model:  engine_ms ~= c + a * (cpu + memory_bytes * w_memory) + d * io + b * hash_rows + r * random_io

* `c` is the fixed cost of one query (plan decoding, the FFI round trips, batch encoding); it
  is not part of the planner's cost, which only compares plans of the same query, but without
  it the small workloads would make every per-row weight look larger than it is.
* `a` is the time of one row operation, `d` of one page read (in the engine: walking the
  primary index and decoding the rows of 8 KiB of row data), `b` of one hashed row, `r` of one
  random access (an index descent or a row fetched by key; reports before 2.3.0 have none).
* `memory` keeps its configured weight relative to `cpu`: peak memory is a budget, not time.
* Least squares on the relative error, so every plan counts the same however long it runs;
  every coefficient is kept non-negative (an exhaustive active-set search over the terms).

The fitted weights relative to one row operation are `d / a` (io), `b / a` (hash_row) and
`r / a` (random_io); put
them into `CostWeights` (jvm/adb-optimizer/src/main/scala/io/adb/optimizer/cost/Cost.scala).

Several reports (runs, machines, scale factors) can be given; their points are pooled, which
steadies the fit: in these workloads the rows a plan scans and its row operations grow
together, so a single run separates the two weights only roughly.

Usage: scripts/fit-cost-weights.py [REPORT.json ...]   (default examples/results/2.3.0-ffi.json)
"""
import itertools
import json
import sys


def solve(matrix, vector):
    """Solves a small dense linear system by Gaussian elimination; None if singular."""
    n = len(vector)
    rows = [list(matrix[i]) + [vector[i]] for i in range(n)]
    for col in range(n):
        pivot = max(range(col, n), key=lambda r: abs(rows[r][col]))
        if abs(rows[pivot][col]) < 1e-18:
            return None
        rows[col], rows[pivot] = rows[pivot], rows[col]
        for r in range(n):
            if r != col:
                factor = rows[r][col] / rows[col][col]
                for c in range(col, n + 1):
                    rows[r][c] -= factor * rows[col][c]
    return [rows[i][n] / rows[i][i] for i in range(n)]


def least_squares(features, targets):
    """Coefficients minimizing sum((features_i . x - 1)^2) for features already divided by t_i."""
    k = len(features[0])
    normal = [[sum(f[i] * f[j] for f in features) for j in range(k)] for i in range(k)]
    rhs = [sum(f[i] for f in features) for i in range(k)]
    return solve(normal, rhs)


def fit(points, w_memory):
    """Returns (c, a, d, b, r) >= 0 of the model above with the smallest relative squared error.

    Every subset of the terms that contains the row-operation term is fitted; solutions with a
    negative coefficient are discarded, the best remaining one wins.
    """
    columns = [
        lambda p: 1.0,
        lambda p: p["cpu"] + p["memory_bytes"] * w_memory,
        lambda p: p["io"],
        lambda p: p["hash_rows"],
        lambda p: p.get("random_io", 0.0),
    ]
    best = None
    for size in range(1, len(columns) + 1):
        for active in itertools.combinations(range(len(columns)), size):
            if 1 not in active:
                continue  # row operations always cost something
            features = [[columns[i](p) / p["engine_ms"] for i in active] for p in points]
            solution = least_squares(features, None)
            if solution is None or any(x < 0 for x in solution):
                continue
            coefficients = [0.0] * len(columns)
            for i, x in zip(active, solution):
                coefficients[i] = x
            error = sum(
                ((sum(k * column(p) for k, column in zip(coefficients, columns)) - p["engine_ms"]) / p["engine_ms"]) ** 2
                for p in points
            )
            if best is None or error < best[0]:
                best = (error, coefficients)
    return tuple(best[1])


def spread(points, weights, overhead_ms=0.0):
    """Max / min engine milliseconds (minus a fixed overhead) per weighted cost unit."""
    ratios = []
    for p in points:
        cost = (p["cpu"] * weights["cpu"] + p["io"] * weights["io"]
                + p["memory_bytes"] * weights["memory"] + p["hash_rows"] * weights["hash_row"]
                + p.get("random_io", 0.0) * weights.get("random_io", 0.0))
        if cost > 0 and p["engine_ms"] > overhead_ms:
            ratios.append((p["engine_ms"] - overhead_ms) / cost)
    return max(ratios) / min(ratios) if ratios else float("nan")


def main():
    paths = sys.argv[1:] or ["examples/results/2.3.0-ffi.json"]
    weights, points = None, []
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            calibration = json.load(handle)["calibration"]
        weights = weights or calibration["weights"]
        points += [dict(p, plan=f"{p['plan']} ({path.rsplit('/', 1)[-1]})") for p in calibration["points"] if p["engine_ms"] > 0]
    if len(points) < 4:
        sys.exit("need at least four calibration points")
    c, a, d, b, r = fit(points, weights["memory"] / weights["cpu"])
    fitted = dict(weights, io=round(d / a, 1), hash_row=round(b / a, 2), random_io=round(r / a, 1))
    print(f"points: {len(points)}")
    for p in points:
        model = (c + a * (p["cpu"] + p["memory_bytes"] * weights["memory"]) + d * p["io"]
                 + b * p["hash_rows"] + r * p.get("random_io", 0.0))
        print(f"  {p['plan']:<32} measured {p['engine_ms']:9.3f} ms  model {model:9.3f} ms")
    print(f"fixed overhead per query {c:.3f} ms; row operation {a * 1e6:.1f} ns; "
          f"page {d * 1e3:.2f} us; hashed row {b * 1e6:.1f} ns; random access {r * 1e6:.1f} ns")
    print(f"current weights {weights}: spread {spread(points, weights, c):.2f}")
    print(f"fitted io = {fitted['io']}, hash_row = {fitted['hash_row']}, random_io = {fitted['random_io']}: "
          f"spread {spread(points, fitted, c):.2f}")


if __name__ == "__main__":
    main()
