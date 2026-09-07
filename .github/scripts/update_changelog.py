#!/usr/bin/env python3
"""
Append-on-push CHANGELOG maintenance for whyknot-yt-dlp-plugins.

Spirit-port of WKVRCProxy/.github/scripts/Update-Changelog.ps1's Append mode.
Walks the commits in --range, buckets by conventional-commit type, and merges
into the "## Unreleased" section of CHANGELOG.md. Skips merge commits, bot
commits, "[skip changelog]" subjects, and types that aren't user-visible
(docs/build/ci/test/non-deps chore).

Differences from the WKVRCProxy version:
  - No build-stamp regex strip (no prepare-commit-msg hook in this repo)
  - Promote/Notes modes deferred -- release.yml inlines that logic

Invoked by .github/workflows/changelog-append.yml. Idempotent: re-running
against the same range is a no-op (de-duped by short-sha in existing bullets).
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

CHANGELOG = Path("CHANGELOG.md")
BUCKET_ORDER = ["Breaking", "Added", "Changed", "Fixed"]
CONVENTIONAL = re.compile(
    r"^(?P<type>feat|fix|perf|refactor|docs|build|ci|chore|test|revert)"
    r"(?:\((?P<scope>[^)]+)\))?(?P<bang>!)?:\s+(?P<desc>.+)$"
)


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
    if not any(buckets.values()):
        return "\n_No notable changes since the last release._\n"
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


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--range", required=True, help="git-log range (e.g. abc..def)")
    args = parser.parse_args()

    log = subprocess.run(
        ["git", "log", "--no-merges", "--format=%H%x09%s", args.range],
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
    m = re.search(r"(?m)^## +Unreleased\s*$", text)
    if not m:
        print("CHANGELOG.md missing '## Unreleased' heading", file=sys.stderr)
        return 1
    start = m.end()
    tail = text[start:]
    end_m = re.search(r"(?m)^(---|## )", tail)
    end = start + (end_m.start() if end_m else len(tail))

    existing_body = text[start:end].strip("\n")
    existing = parse_existing_body(existing_body)

    # Merge: append new bullets into existing buckets, dedupe by short-sha.
    sha_re = re.compile(r"\(([a-f0-9]{7})\)\s*$")
    for bucket, bullets in new.items():
        existing.setdefault(bucket, [])
        seen = {sha_re.search(b).group(1) for b in existing[bucket] if sha_re.search(b)}
        for b in bullets:
            sm = sha_re.search(b)
            if sm and sm.group(1) in seen:
                continue
            existing[bucket].append(b)

    new_body = render_body(existing)
    CHANGELOG.write_text(text[:start] + "\n" + new_body + text[end:], encoding="utf-8")
    print("Updated CHANGELOG.md")
    return 0


if __name__ == "__main__":
    sys.exit(main())
