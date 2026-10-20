#!/usr/bin/env python3
"""Local stand-in for `fdroid rewritemeta --check`.

fdroiddata's CI rejects any metadata file that differs from what its canonical
dumper would produce, and the fold column for long commands is easy to get
wrong by hand. This round-trips the file through the same ruamel settings
fdroidserver uses (metadata.py: YAML(typ='rt'), indent(mapping=2, sequence=4,
offset=2), ruamel's default 80-column width) and prints a diff if they differ.

    pip install 'ruamel.yaml==0.17.40'
    python fdroid/check-rewritemeta.py fdroid/dev.vigov5.wisp.yml [--write]

The version pin matters. fdroidserver only requires `ruamel.yaml >= 0.17.22`,
and the fold behaviour changed after 0.17: 0.18+ breaks *before* column 80 and
leaves a trailing space, while 0.17.x breaks at the first space *after* it.
Checked against CI output on 2026-10-21: 0.17.40 reproduces it exactly, 0.18.14
and 0.19.1 do not. If CI starts disagreeing with this script again, its image
has probably moved to a newer ruamel — compare against the diff CI prints and
re-pin.

--write rewrites the file in canonical form instead of only reporting.
Exit status is 0 when the file is already canonical, 1 otherwise.
"""

import difflib
import io
import sys

import ruamel.yaml


def canonical(text: str) -> str:
    yaml = ruamel.yaml.YAML(typ="rt")
    yaml.indent(mapping=2, sequence=4, offset=2)
    data = yaml.load(text)
    buf = io.StringIO()
    yaml.dump(data, buf)
    return buf.getvalue()


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    write = "--write" in sys.argv
    if not args:
        print(__doc__)
        return 2
    path = args[0]

    with open(path, encoding="utf-8") as fh:
        original = fh.read()
    rewritten = canonical(original)

    if original == rewritten:
        print(f"{path}: already canonical")
        return 0

    if write:
        with open(path, "w", encoding="utf-8", newline="\n") as fh:
            fh.write(rewritten)
        print(f"{path}: rewritten in canonical form")
        return 0

    print(f"{path}: NEEDS rewritemeta\n")
    sys.stdout.writelines(
        difflib.unified_diff(
            original.splitlines(keepends=True),
            rewritten.splitlines(keepends=True),
            fromfile="current",
            tofile="canonical",
        )
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())
