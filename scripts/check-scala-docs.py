#!/usr/bin/env python3
"""Checks that every public top-level and member declaration of Scala sources has a Scaladoc comment.

``check-scala-docs.py FILE...`` (the Rust equivalent is `clippy::missing_docs_in_private_items`, which
CI already enforces). A declaration is a `class`, `object`, `trait`, `enum`, `def`, `val` or `type`
that is not `private` or `protected`, sits at the top level or directly inside a class, object, trait
or enum, and has no `/** ... */` comment above it (annotations in between are fine; `override` members
inherit the documentation of what they override). Locals inside method bodies are not checked.
Exit status: 0 when every declaration is documented, 1 otherwise, 2 for a usage error.
"""
import re
import sys

DECLARATION = re.compile(
    r"^\s*((final|sealed|abstract|case|opaque|inline|given|lazy)\s+)*(class|object|trait|enum|def|val|type)\s+\w+")
HIDDEN = re.compile(r"^\s*((final|sealed|abstract|inline|lazy)\s+)*(private|protected)\b")
CONTAINER = re.compile(r"^\s*((final|sealed|abstract|case|opaque)\s+)*(class|object|trait|enum)\b")


def indent(line: str) -> int:
    """The number of leading spaces of `line`."""
    return len(line) - len(line.lstrip())


def enclosing_is_container(lines, index: int) -> bool:
    """Whether the nearest less-indented line above `index` opens a class, object, trait or enum."""
    here = indent(lines[index])
    if here == 0:
        return True
    k = index - 1
    while k >= 0 and (not lines[k].strip() or indent(lines[k]) >= here):
        k -= 1
    return k >= 0 and bool(CONTAINER.match(lines[k]))


def undocumented(text: str):
    """Yields ``(line_number, declaration)`` for each public declaration without Scaladoc."""
    lines = text.split("\n")
    for i, line in enumerate(lines):
        if not DECLARATION.match(line) or HIDDEN.match(line) or line.lstrip().startswith("override"):
            continue
        if not enclosing_is_container(lines, i):
            continue
        j = i - 1
        while j >= 0 and (lines[j].strip().startswith("@") or not lines[j].strip()):
            j -= 1
        if j >= 0 and lines[j].strip().endswith("*/"):
            continue
        yield i + 1, line.strip()[:100]


def main(argv) -> int:
    """Command line; returns the exit status."""
    if not argv:
        print(__doc__.strip().split("\n\n")[0], file=sys.stderr)
        return 2
    missing = 0
    for path in argv:
        try:
            with open(path, encoding="utf-8") as handle:
                text = handle.read()
        except OSError as error:
            print(f"check-scala-docs: {error}", file=sys.stderr)
            return 2
        for number, declaration in undocumented(text):
            print(f"{path}:{number}: no Scaladoc: {declaration}")
            missing += 1
    print(f"{missing} undocumented declaration(s)")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
