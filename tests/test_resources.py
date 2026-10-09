import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "tests" / "resource_probe.py"
NO_LIBRARY = 3
NO_MEMORY_READING = 4

STARTUP_BUDGET_MB = {"musl": 54, "glibc": 58, "windows": 74}
HELD_AFTER_READY_MB = {"musl": 5, "glibc": 14, "windows": 10}
READY_PEAK_PER_JSON_MB = 7
EVENT_BUDGET_US = 150
IDLE_CURL_ACTIONS = 4


@pytest.fixture(scope="module")
def probe():
    saved = os.environ.get("RESOURCE_REPORT")
    if saved:
        return json.loads(Path(saved).read_text(encoding="utf-8").strip().splitlines()[-1])
    paths = [os.environ.get("PYTHONPATH"), str(ROOT)]
    env = dict(os.environ, PYTHONPATH=os.pathsep.join(p for p in paths if p))
    env.pop("PYTHONDONTWRITEBYTECODE", None)
    subprocess.run(
        [sys.executable, "-OO", "-c", "import idlebot.discord_client"],
        cwd=ROOT,
        env=env,
        capture_output=True,
        timeout=180,
    )
    result = subprocess.run(
        [sys.executable, "-OO", str(PROBE)],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
    )
    if result.returncode == NO_LIBRARY:
        if os.environ.get("REQUIRE_RESOURCE_PROBE"):
            pytest.fail("REQUIRE_RESOURCE_PROBE is set but the runtime requirements are not installed")
        pytest.skip("the runtime requirements are not installed")
    if result.returncode == NO_MEMORY_READING:
        pytest.skip("no memory reading for %s" % sys.platform)
    assert result.returncode == 0, result.stderr
    return json.loads(result.stdout.strip().splitlines()[-1])


def test_startup_memory_stays_within_budget(probe):
    budget = STARTUP_BUDGET_MB[probe["runtime"]]
    assert probe["startup_mb"] < budget, "%.1f MB after start, budget %d" % (probe["startup_mb"], budget)


def test_the_cache_options_and_afk_flag_take_effect(probe):
    assert probe["cache_problems"] == []
    assert probe["afk"] is True


def test_ready_leaves_no_servers_friends_or_dms_in_memory(probe):
    assert probe["sessions"] == 3
    assert probe["guilds"] == 0
    assert probe["relationships"] == 0
    assert probe["private_channels"] == 0
    assert probe["users"] <= 1


def test_ready_peak_and_what_it_leaves_behind_stay_within_budget(probe):
    peak_budget = READY_PEAK_PER_JSON_MB * probe["ready_json_mb"]
    assert probe["ready_peak_mb"] < peak_budget, "READY peak %.1f MB, budget %.1f" % (
        probe["ready_peak_mb"],
        peak_budget,
    )
    held_budget = HELD_AFTER_READY_MB[probe["runtime"]]
    assert probe["held_after_ready_mb"] < held_budget, "%.1f MB held after READY, budget %d" % (
        probe["held_after_ready_mb"],
        held_budget,
    )


def test_a_dropped_gateway_event_costs_microseconds(probe):
    assert probe["dropped"] == 5000
    assert probe["event_us"] < EVENT_BUDGET_US, "%.1f us per dropped event" % probe["event_us"]


def test_curl_stays_asleep_while_idle(probe):
    assert probe["idle_curl_actions"] <= IDLE_CURL_ACTIONS
