#!/usr/bin/env python3
"""Interleaved A/B comparison of the engine at a git ref (A, the baseline) and the working tree (B).

Comparing two reports taken hours apart measures the machine's mood as much as the code (the CPU
governor alone moved write rates by 8x in the 2.5.x results). This tool removes the drift:

1. a git worktree of the baseline ref is created, and the *current* ``crates/adb-bench`` and
   ``examples/rust-benchmark`` are copied over it (the workspace members are patched), so the very
   same harness binaries measure the old and the new engine;
2. both trees are built in release mode;
3. for ``--rounds`` rounds the three variants **A**, **B** and **A2** (a second copy of A, the A/A
   control) run in a rotating order, each as a fresh process, so slow drift hits all of them alike;
4. per case the ratio ``B / A`` is compared with the control ratio ``A2 / A``: the control shows how
   much two identical builds differ on this machine, and a change is only reported when it exceeds
   both the limit and the control.

Rust families only (``rust-local``, ``workloads``, ``memory``, ``scaling``); the JVM families do not
build against an old engine. Deterministic values must agree between A and B (``fail`` otherwise).
Exit codes follow ``bench_lib``: 0, 1 fail, 3 unstable (the control itself moved more than the limit).

    bench_ab.py --family rust-local --baseline v2.5.4 --rounds 3 --out ab.json
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

import bench_lib
import bench_repeat

#: family -> (cargo package, binary name)
FAMILIES = {
    "rust-local": ("adb-benchmark-rust", "adb-benchmark-rust"),
    "workloads": ("adb-benchmark-rust", "workloads"),
    "memory": ("adb-benchmark-rust", "memory"),
    "scaling": ("adb-benchmark-rust", "scaling"),
}

#: directories copied from the working tree over the baseline worktree
OVERLAY = ("crates/adb-bench", "examples/rust-benchmark")

#: workspace members the overlay needs
REQUIRED_MEMBERS = ("crates/adb-bench", "examples/rust-benchmark")

DEFAULT_LIMIT_PERCENT = 5.0


def patch_workspace(text: str) -> str:
    """Adds the members of ``REQUIRED_MEMBERS`` that the baseline's workspace manifest lacks."""
    match = re.search(r"members\s*=\s*\[(.*?)\]", text, re.S)
    if not match:
        raise ValueError("no `members = [...]` in the workspace manifest")
    present = set(re.findall(r'"([^"]+)"', match.group(1)))
    missing = [m for m in REQUIRED_MEMBERS if m not in present]
    if not missing:
        return text
    body = match.group(1).rstrip()
    if body and not body.endswith(","):
        body += ","
    body += "".join(f'\n    "{m}",' for m in missing) + "\n"
    return text[:match.start(1)] + body + text[match.end(1):]


def order(round_index: int):
    """The order of A, B and A2 in a round: the rotation shifts every round."""
    base = ["A", "B", "A2"]
    shift = round_index % 3
    return base[shift:] + base[:shift]


def compare(a_docs, b_docs, c_docs, limit=DEFAULT_LIMIT_PERCENT):
    """Compares the runs of A, B and the control A2. Returns ``(document, status)``."""
    a, problems_a, _ = bench_repeat.condense(a_docs, 1e9)
    b, problems_b, _ = bench_repeat.condense(b_docs, 1e9)
    c, problems_c, _ = bench_repeat.condense(c_docs, 1e9)
    problems = [f"A: {p}" for p in problems_a] + [f"B: {p}" for p in problems_b] + [f"A2: {p}" for p in problems_c]
    cases, statuses = {}, []
    ca, cb, cc = bench_lib.cases(a), bench_lib.cases(b), bench_lib.cases(c)
    for name in ca:
        ra = ca[name].get("repeat")
        rb = cb.get(name, {}).get("repeat")
        rc = cc.get(name, {}).get("repeat")
        reference = bench_repeat.deterministic_of(ca[name])
        if name in cb and bench_repeat.deterministic_of(cb[name]) != reference:
            problems.append(f"case `{name}`: B {bench_repeat.deterministic_of(cb[name])} differs from A {reference}")
        if not (ra and rb and rc) or ra["median_ms"] == 0:
            continue
        ratio = rb["median_ms"] / ra["median_ms"]
        control = rc["median_ms"] / ra["median_ms"]
        change = (ratio - 1.0) * 100.0
        noise = abs(control - 1.0) * 100.0
        if noise > limit:
            verdict = "unstable"
        elif abs(change) <= max(limit, noise):
            verdict = "same"
        else:
            verdict = "faster" if change < 0 else "slower"
        statuses.append(bench_lib.UNSTABLE if verdict == "unstable" else bench_lib.PASS)
        cases[name] = {"a_ms": ra["median_ms"], "b_ms": rb["median_ms"], "a2_ms": rc["median_ms"],
                       "ratio": ratio, "control_ratio": control, "change_percent": change,
                       "control_noise_percent": noise, "verdict": verdict}
    if problems:
        statuses.append(bench_lib.FAIL)
    return {"schema": 3, "kind": "ab", "limit_percent": limit, "limit_is_provisional": True,
            "rounds": len(a_docs), "cases": cases, "problems": problems}, bench_lib.worst(statuses)


def resolve_ref(repo: str, ref: str, run=subprocess.run) -> str:
    """The commit of ``ref``, also accepting a release number without its ``v`` (``2.5.4`` for the
    tag ``v2.5.4``). Raises ``ValueError`` with what to do when neither exists."""
    for candidate in (ref, f"v{ref}"):
        done = run(["git", "-C", repo, "rev-parse", "--verify", "--quiet", f"{candidate}^{{commit}}"],
                   capture_output=True, text=True, check=False)
        if done.returncode == 0:
            return done.stdout.strip()
    raise ValueError(
        f"no commit or tag `{ref}` (nor `v{ref}`) in {repo}. Fetch tags with `git fetch --tags`, or create "
        f"the baseline tag, e.g. `git tag v2.5.4 c42f358 && git push origin v2.5.4` (the merge of PR #117)")


def build_baseline(repo: str, ref: str, workdir: str, run=subprocess.run) -> str:
    """Creates the worktree of ``ref`` in ``workdir`` and overlays the current harness; returns its path."""
    tree = os.path.join(workdir, "baseline")
    commit = resolve_ref(repo, ref, run)
    run(["git", "-C", repo, "worktree", "add", "--detach", tree, commit], check=True)
    for relative in OVERLAY:
        target = os.path.join(tree, relative)
        shutil.rmtree(target, ignore_errors=True)
        shutil.copytree(os.path.join(repo, relative), target, ignore=shutil.ignore_patterns("target"))
    manifest = os.path.join(tree, "Cargo.toml")
    with open(manifest, encoding="utf-8") as handle:
        patched = patch_workspace(handle.read())
    with open(manifest, "w", encoding="utf-8") as handle:
        handle.write(patched)
    return tree


def cargo_build(tree: str, family: str, run=subprocess.run) -> str:
    """Builds the binary of ``family`` in release mode in ``tree``; returns its path."""
    package, binary = FAMILIES[family]
    argv = ["cargo", "build", "--release", "-p", package]
    if binary != package:
        argv += ["--bin", binary]
    run(argv, cwd=tree, check=True)
    return os.path.join(tree, "target", "release", binary)


def main(argv=None) -> int:
    """Command line."""
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--family", choices=sorted(FAMILIES), default="rust-local")
    parser.add_argument("--baseline", default="v2.5.4")
    parser.add_argument("--rounds", type=int, default=3)
    parser.add_argument("--limit", type=float, default=DEFAULT_LIMIT_PERCENT)
    parser.add_argument("--out", required=True)
    parser.add_argument("--repo", default=os.path.join(os.path.dirname(os.path.abspath(__file__)), ".."))
    parser.add_argument("--keep", action="store_true", help="keep the baseline worktree")
    try:
        args = parser.parse_args(argv)
    except SystemExit as exit_:
        return 2 if exit_.code else 0
    if args.rounds < 1:
        print("--rounds must be at least 1", file=sys.stderr)
        return 2
    repo = os.path.abspath(args.repo)
    work = tempfile.mkdtemp(prefix="bench-ab-")
    try:
        tree = build_baseline(repo, args.baseline, work)
        binaries = {"A": cargo_build(tree, args.family), "A2": None, "B": cargo_build(repo, args.family)}
        binaries["A2"] = binaries["A"]
        runs = {"A": [], "B": [], "A2": []}
        for index in range(args.rounds):
            for label in order(index):
                out = os.path.join(work, f"{label}-{index}.json")
                subprocess.run([binaries[label], "--output", out], check=True)
                runs[label].append(bench_lib.load(out))
        document, status = compare(runs["A"], runs["B"], runs["A2"], args.limit)
        document["baseline"], document["family"] = args.baseline, args.family
        with open(args.out, "w", encoding="utf-8") as handle:
            json.dump(document, handle, indent=2)
            handle.write("\n")
        for problem in document["problems"]:
            print(f"FAIL {problem}")
        for name, row in document["cases"].items():
            print(f"{name:40} {row['change_percent']:+7.1f}%  control {row['control_noise_percent']:5.1f}%  {row['verdict']}")
        print(f"A/B {args.baseline} vs working tree -> {args.out}: {status}")
        return bench_lib.exit_code(status)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"bench_ab: {error}", file=sys.stderr)
        return 2
    finally:
        if not args.keep:
            subprocess.run(["git", "-C", repo, "worktree", "remove", "--force", os.path.join(work, "baseline")],
                           check=False, capture_output=True)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
