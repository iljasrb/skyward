#!/usr/bin/env python3
"""Keep the pages' texts in `locales/en.json`, whatever upstream writes.

Upstream's pages write every text three times, once per language, as
`["简体", "繁體", "English"]` arrays, `l.pick([...])` or `tr!(l, ...)`. This
fork keeps them in `locales/en.json` and draws them from the `S_*` constants
`crates/skyward-ui/build.rs` generates. A merge of upstream therefore lands
with texts in the wrong shape.

`check` says whether any are left, and `convert` moves them: each English
text goes into `locales/en.json` under the fork's key, and the source comes
to name that key. Run `convert` after a merge and `check` before a commit.

The English text is the only one kept: the fork speaks English alone, and a
translation adds a file beside `en.json` rather than a second string in the
source.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
LOCALES = ROOT / "locales" / "en.json"
SOURCES = ROOT / "crates" / "skyward-ui" / "src"
# The shims themselves write the three languages on purpose.
SKIP = {"lang.rs"}

STR = r'"(?:[^"\\]|\\.)*"'
LIT = re.compile(STR)
CJK = re.compile(r"[\u3400-\u9fff\uf900-\ufaff]")

# `const NAME: Text = ["a", "b", "c"];`
CONST = re.compile(r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*Text\s*=\s*\[(.*?)\]\s*;", re.S)
# `entry(["a", "b", "c"], ...`
ENTRY = re.compile(r"entry\(\s*\[(.*?)\]\s*,", re.S)
# `l.pick(["a", "b", "c"])`, however it is laid out.
PICK = re.compile(r"l\s*\.pick\(\s*\[(.*?)\]\s*\)", re.S)
# `tr!(l, "a", "b", "c")`, however it is laid out.
TR = re.compile(r"tr!\(\s*l\s*,\s*(" + STR + r")\s*,\s*(" + STR + r")\s*,\s*(" + STR + r")\s*,?\s*\)", re.S)
# `l.pick(SOME_TEXT)`: a text already named.
PICK_NAME = re.compile(r"l\.pick\(([A-Z][A-Z0-9_]*)\)")
# A language this fork does not speak.
LANG = re.compile(r"Lang::(?:Hans|Hant)\b")


def slug(text: str) -> str:
    """The key the fork gives `text`.

    Apostrophes go rather than becoming an underscore, and placeholders go
    whole: `A partition didn't verify.` is `a_partition_didnt_verify`, and
    `A copy of the key: {shown}` is `a_copy_of_the_key`.
    """
    text = text.replace("'", "").replace("\u2019", "")
    text = re.sub(r"\{[^}]*\}", "", text)
    text = text.lower()
    text = re.sub(r"[^a-z0-9]+", "_", text)
    return text.strip("_")


def unquote(lit: str) -> str:
    return json.loads(lit)


def literals(body: str) -> list[str]:
    return [unquote(lit) for lit in LIT.findall(body)]


@dataclass
class Finding:
    path: Path
    line: int
    kind: str
    english: str
    key: str
    start: int
    end: int
    replacement: str
    note: str = ""


def findings(path: Path, text: str) -> list[Finding]:
    """Every text still written in the three languages, and what to write instead."""
    out: list[Finding] = []

    def line_of(at: int) -> int:
        return text.count("\n", 0, at) + 1

    def add(kind: str, m: re.Match[str], body: str, replacement: str, note: str = "") -> None:
        lits = literals(body)
        if len(lits) != 3:
            out.append(Finding(path, line_of(m.start()), "manual", "", "", m.start(), m.end(), "",
                               f"{kind} holds {len(lits)} texts, not 3"))
            return
        english = lits[2]
        if not CJK.search(lits[0] + lits[1]) and not CJK.search(english):
            # Nothing was written three times: a list of three other things.
            return
        out.append(Finding(path, line_of(m.start()), kind, english, slug(english), m.start(), m.end(), replacement(english)))

    for m in CONST.finditer(text):
        name = m.group(1)
        add("const", m, m.group(2), lambda en, name=name: f"const {name}: Text = S_{slug(en).upper()};")

    for m in ENTRY.finditer(text):
        add("entry", m, m.group(1), lambda en: f"entry(S_{slug(en).upper()},")

    for m in PICK.finditer(text):
        add("pick", m, m.group(1), lambda en: f"S_{slug(en).upper()}")

    for m in TR.finditer(text):
        english = unquote(m.group(3))
        args = re.findall(r"\{(\w+)\}", english)
        call = "crate::t!(S_" + slug(english).upper() + "".join(f", {a} = {a}" for a in args) + ")"
        if not CJK.search(unquote(m.group(1)) + unquote(m.group(2))):
            continue
        out.append(Finding(path, line_of(m.start()), "tr", english, slug(english), m.start(), m.end(), call))

    for m in PICK_NAME.finditer(text):
        out.append(Finding(path, line_of(m.start()), "pick-name", "", "", m.start(), m.end(), m.group(1),
                           "a text already named is its own English"))

    for m in LANG.finditer(text):
        out.append(Finding(path, line_of(m.start()), "lang", "", "", m.start(), m.end(), "Lang::En",
                           "the pages speak English; check what the line expects"))

    out.sort(key=lambda f: f.start)
    # A bare array inside a const, an entry or a pick is already handled.
    seen: list[Finding] = []
    for f in out:
        if seen and f.start < seen[-1].end:
            continue
        seen.append(f)
    return seen


def scan() -> list[Finding]:
    found: list[Finding] = []
    for path in sorted(SOURCES.glob("*.rs")):
        if path.name in SKIP:
            continue
        found.extend(findings(path, path.read_text()))
    return found


def load() -> dict[str, str]:
    return json.loads(LOCALES.read_text())


def save(locales: dict[str, str]) -> None:
    LOCALES.write_text(json.dumps(dict(sorted(locales.items())), indent=2, ensure_ascii=False) + "\n")


def check() -> int:
    found = scan()
    if not found:
        print("every text is in locales/en.json")
        return 0
    for f in found:
        where = f"{f.path.relative_to(ROOT)}:{f.line}"
        if f.kind == "manual":
            print(f"{where}: {f.note}")
        elif f.kind == "lang":
            print(f"{where}: {f.note}")
        else:
            print(f"{where}: {f.kind} {f.key}")
    print(f"\n{len(found)} to convert; run scripts/locales.py convert")
    return 1


def convert(dry_run: bool) -> int:
    found = scan()
    if not found:
        print("every text is in locales/en.json")
        return 0
    locales = load()
    manual = [f for f in found if f.kind == "manual"]
    if manual:
        for f in manual:
            print(f"{f.path.relative_to(ROOT)}:{f.line}: {f.note}")
        print("fix these by hand, then run convert again")
        return 1

    collisions: dict[str, str] = {}
    by_file: dict[Path, list[Finding]] = {}
    for f in found:
        if f.kind in ("pick-name", "lang"):
            by_file.setdefault(f.path, []).append(f)
            continue
        if not f.key:
            print(f"{f.path.relative_to(ROOT)}:{f.line}: {f.english!r} has no key")
            return 1
        if "{" in f.english and f.kind == "pick":
            print(f"{f.path.relative_to(ROOT)}:{f.line}: {f.english!r} fills placeholders a pick cannot")
            return 1
        known = locales.get(f.key)
        if known is not None and known != f.english:
            collisions.setdefault(f.key, known)
        else:
            locales[f.key] = f.english
        by_file.setdefault(f.path, []).append(f)

    if collisions:
        for key, known in collisions.items():
            print(f"{key}: already {known!r}, and another text wants it")
        print("give one of them a key of its own")
        return 1

    for path, items in by_file.items():
        text = path.read_text()
        for f in reversed(items):
            text = text[:f.start] + f.replacement + text[f.end:]
        if not dry_run:
            path.write_text(text)

    if not dry_run:
        save(locales)
    print(f"converted {len(found)} texts in {len(by_file)} files" + (" (dry run)" if dry_run else ""))
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("command", choices=["check", "convert"])
    parser.add_argument("--dry-run", action="store_true", help="say what convert would do, and change nothing")
    args = parser.parse_args()
    return check() if args.command == "check" else convert(args.dry_run)


if __name__ == "__main__":
    sys.exit(main())
