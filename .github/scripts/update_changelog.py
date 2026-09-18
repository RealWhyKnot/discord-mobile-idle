#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

CHANGELOG = Path("CHANGELOG.md")
DEFAULT_REPO = "RealWhyKnot/discord-mobile-idle"
BUCKET_ORDER = ["Breaking", "Added", "Changed", "Fixed"]
CONVENTIONAL = re.compile(
    r"^(?P<type>feat|fix|perf|refactor|docs|build|ci|chore|test|revert)"
    r"(?:\((?P<scope>[^)]+)\))?(?P<bang>!)?:\s+(?P<desc>.+)$"
)
UNRELEASED = re.compile(r"(?m)^## +Unreleased[ \t]*$")
SECTION_END = re.compile(r"(?m)^(---|## )")
HAS_ENTRIES = re.compile(r"(?m)^(### |- )")


def parse_subject(sha: str, subject: str) -> tuple[str, str] | None:
    """Return (bucket, bullet) for a commit, or None to skip."""
    if "[skip changelog]" in subject:
        return None
    if subject.startswith("Merge "):
        return None

    short = sha[:7]
    m = CONVENTIONAL.match(subject)
    if not m:
        # Non-conventional surface under Changed rather than dropping
        return ("Changed", f"- {subject} ({short})")

    type_ = m.group("type")
    scope = m.group("scope") or ""
    is_breaking = bool(m.group("bang"))
    desc = m.group("desc")
    desc = desc[:1].upper() + desc[1:] if desc else desc
    scope_prefix = f"**{scope}:** " if scope else ""
    bullet = f"- {scope_prefix}{desc} ({short})"

    if is_breaking:
        return ("Breaking", bullet)

    if type_ == "feat":
        return ("Added", bullet)
    if type_ == "fix":
        return ("Fixed", bullet)
    if type_ in ("perf", "refactor", "revert"):
        return ("Changed", bullet)
    if type_ == "chore" and scope.startswith("deps"):
        return ("Changed", bullet)
    # docs / build / ci / test / other-chore -> skip
    return None


def parse_existing_body(body: str) -> dict[str, list[str]]:
    """Parse '### Section / - bullet' lines from an Unreleased section body."""
    sections: dict[str, list[str]] = {}
    current: str | None = None
    for line in body.splitlines():
        m = re.match(r"^### +(.+?)\s*$", line)
        if m:
            current = m.group(1)
            sections.setdefault(current, [])
            continue
        if line.lstrip().startswith("- ") and current:
            sections[current].append(line)
    return sections


def render_body(buckets: dict[str, list[str]]) -> str:
    """Render bucket map back to markdown body."""
    out: list[str] = [""]
    emitted: set[str] = set()
    for name in BUCKET_ORDER:
        if name in buckets and buckets[name]:
            out.append(f"### {name}")
            out.extend(buckets[name])
            out.append("")
            emitted.add(name)
    for name, bullets in buckets.items():
        if name not in emitted and bullets:
            out.append(f"### {name}")
            out.extend(bullets)
            out.append("")
    return "\n".join(out) + "\n"


def find_unreleased(text: str) -> tuple[int, int, int]:
    m = UNRELEASED.search(text)
    if not m:
        raise LookupError("CHANGELOG.md missing '## Unreleased' heading")
    tail = text[m.end() :]
    end_m = SECTION_END.search(tail)
    return m.start(), m.end(), m.end() + (end_m.start() if end_m else len(tail))


def append(rev_range: str) -> int:
    log = subprocess.run(
        ["git", "log", "--no-merges", "--format=%H%x09%s", rev_range],
        check=True,
        capture_output=True,
        text=True,
    ).stdout

    new: dict[str, list[str]] = {}
    for line in log.splitlines():
        if "\t" not in line:
            continue
        sha, _, subject = line.partition("\t")
        parsed = parse_subject(sha, subject)
        if parsed is None:
            continue
        bucket, bullet = parsed
        new.setdefault(bucket, []).append(bullet)

    if not any(new.values()):
        print("No user-visible commits in range; nothing to append.")
        return 0

    text = CHANGELOG.read_text(encoding="utf-8")
    _, start, end = find_unreleased(text)
    existing = parse_existing_body(text[start:end].strip("\n"))

    sha_re = re.compile(r"\(([a-f0-9]{7})\)\s*$")
    for bucket, bullets in new.items():
        existing.setdefault(bucket, [])
        seen = {sha_re.search(b).group(1) for b in existing[bucket] if sha_re.search(b)}
        for b in bullets:
            sm = sha_re.search(b)
            if sm and sm.group(1) in seen:
                continue
            existing[bucket].append(b)

    CHANGELOG.write_text(text[:start] + "\n" + render_body(existing) + text[end:], encoding="utf-8", newline="\n")
    print("Updated CHANGELOG.md")
    return 0


def promote(version: str) -> int:
    text = CHANGELOG.read_text(encoding="utf-8")
    if re.search(rf"(?m)^## \[{re.escape(version)}\]", text):
        print(f"{version} already has a section; nothing to promote.")
        return 0

    head, start, end = find_unreleased(text)
    body = text[start:end].strip("\n")
    if not HAS_ENTRIES.search(body):
        body = "_No user-visible changes in this release._"

    repo = os.environ.get("GITHUB_REPOSITORY") or DEFAULT_REPO
    date = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    heading = f"## [{version}](https://github.com/{repo}/releases/tag/{version}) - {date}"
    rolled = f"## Unreleased\n\n_No notable changes since the last release._\n\n---\n\n{heading}\n\n{body}\n"
    rest = text[end:]
    CHANGELOG.write_text(text[:head] + rolled + (f"\n{rest}" if rest else ""), encoding="utf-8", newline="\n")
    print(f"Promoted Unreleased to {version}.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--range", dest="rev_range", help="git-log range to append (e.g. abc..def)")
    mode.add_argument("--promote", metavar="VERSION", help="roll Unreleased into a section for this tag")
    args = parser.parse_args()

    try:
        if args.promote:
            return promote(args.promote)
        return append(args.rev_range)
    except LookupError as exc:
        print(exc, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
