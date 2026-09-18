from __future__ import annotations

import importlib.util
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / ".github" / "scripts" / "update_changelog.py"

HEADER = """# Changelog

All notable changes to this project will be documented in this file.

"""


def load_script():
    spec = importlib.util.spec_from_file_location("update_changelog", SCRIPT)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def git(cwd: Path, *args: str) -> str:
    return subprocess.run(["git", *args], cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def commit(cwd: Path, subject: str) -> str:
    (cwd / "file.txt").write_text(subject, encoding="utf-8")
    git(cwd, "add", "file.txt")
    git(cwd, "commit", "--no-verify", "-m", subject)
    return git(cwd, "rev-parse", "--short=7", "HEAD")


@pytest.fixture
def repo(tmp_path, monkeypatch):
    git(tmp_path, "init", "-b", "main")
    git(tmp_path, "config", "user.email", "test@example.invalid")
    git(tmp_path, "config", "user.name", "Test")
    git(tmp_path, "config", "commit.gpgsign", "false")
    (tmp_path / "CHANGELOG.md").write_text(HEADER + "## Unreleased\n", encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    return tmp_path


@pytest.fixture
def script(monkeypatch):
    monkeypatch.setenv("GITHUB_REPOSITORY", "owner/repo")
    return load_script()


def read(repo: Path) -> str:
    return (repo / "CHANGELOG.md").read_text(encoding="utf-8")


def test_repeated_appends_keep_one_blank_line_under_unreleased(repo, script):
    for subject in ("feat: one", "fix: two", "feat: three"):
        before = git(repo, "rev-parse", "HEAD") if git(repo, "rev-list", "-n", "1", "--all") else None
        sha = commit(repo, subject)
        assert sha
        script.append(f"{before}..HEAD" if before else "HEAD")

    text = read(repo)
    assert "## Unreleased\n\n### Added\n" in text
    assert "\n\n\n" not in text


def test_append_dedupes_by_short_sha(repo, script):
    base = git(repo, "rev-parse", "HEAD") if git(repo, "rev-list", "-n", "1", "--all") else None
    commit(repo, "feat: one")
    rev_range = f"{base}..HEAD" if base else "HEAD"
    script.append(rev_range)
    script.append(rev_range)

    assert read(repo).count("- One (") == 1


def test_promote_moves_the_body_into_a_dated_section(repo, script):
    commit(repo, "feat: one")
    script.append("HEAD")
    script.promote("v2026.9.18.0")

    text = read(repo)
    today = datetime.now(timezone.utc).strftime("%Y-%m-%d")
    assert "## Unreleased\n\n_No notable changes since the last release._\n\n---\n\n" in text
    assert f"## [v2026.9.18.0](https://github.com/owner/repo/releases/tag/v2026.9.18.0) - {today}\n" in text
    assert text.index("### Added") > text.index("v2026.9.18.0")
    assert "\n\n\n" not in text


def test_promote_marks_a_release_with_nothing_user_visible(repo, script):
    script.promote("v2026.9.7.2")

    assert "_No user-visible changes in this release._" in read(repo)


def test_promote_is_a_no_op_once_the_version_has_a_section(repo, script):
    commit(repo, "feat: one")
    script.append("HEAD")
    script.promote("v2026.9.18.0")
    once = read(repo)
    script.promote("v2026.9.18.0")

    assert read(repo) == once


def test_append_after_promote_lands_above_the_released_section(repo, script):
    commit(repo, "feat: one")
    script.append("HEAD")
    script.promote("v2026.9.18.0")
    base = git(repo, "rev-parse", "HEAD")
    commit(repo, "fix: two")
    script.append(f"{base}..HEAD")

    text = read(repo)
    assert text.index("- Two (") < text.index("## [v2026.9.18.0]")
    assert "_No notable changes since the last release._" not in text
    assert "\n\n\n" not in text


def test_both_modes_write_lf_endings(repo, script):
    commit(repo, "feat: one")
    script.append("HEAD")
    assert b"\r\n" not in (repo / "CHANGELOG.md").read_bytes()
    script.promote("v2026.9.18.0")
    assert b"\r\n" not in (repo / "CHANGELOG.md").read_bytes()
