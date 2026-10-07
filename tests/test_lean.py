import logging

from helpers import FakeClient, FakeClock, make_runner

from idlebot import runner as runner_module
from idlebot.lean import KEEP_EVENTS, SlimParsers, trim_ready, trim_supplemental


def make_parsers(seen):
    names = set(KEEP_EVENTS) | {"MESSAGE_CREATE", "PRESENCE_UPDATE"}
    return {name: (lambda data, name=name: seen.append((name, data))) for name in names}


def test_only_the_events_we_use_are_parsed():
    seen = []
    parsers = SlimParsers(make_parsers(seen))

    parsers["SESSIONS_REPLACE"]([1])
    parsers["MESSAGE_CREATE"]({"content": "x"})
    parsers["PRESENCE_UPDATE"]({})
    parsers["PRESENCE_UPDATE"]({})
    parsers["SOMETHING_NEW"]({})

    assert seen == [("SESSIONS_REPLACE", [1])]
    assert parsers.dropped == {"PRESENCE_UPDATE": 2, "MESSAGE_CREATE": 1, "SOMETHING_NEW": 1}
    assert parsers.summary() == "PRESENCE_UPDATE=2 MESSAGE_CREATE=1 SOMETHING_NEW=1"


def test_ready_reaches_the_library_without_servers_friends_or_dms():
    seen = []
    parsers = SlimParsers(make_parsers(seen))
    ready = {
        "user": {"id": "1"},
        "sessions": [{"session_id": "a"}],
        "user_settings_proto": "abc",
        "guilds": [{"id": "9"}],
        "relationships": [{"id": "2"}],
        "private_channels": [{"id": "3"}],
        "users": [{"id": "2"}],
        "read_state": {"entries": [{}], "version": 4},
    }

    parsers["READY"](ready)

    name, data = seen[0]
    assert name == "READY"
    assert data["sessions"] == [{"session_id": "a"}]
    assert data["user_settings_proto"] == "abc"
    assert data["guilds"] == [] and data["relationships"] == [] and data["private_channels"] == []
    assert data["users"] == [] and data["read_state"] == {}
    assert "experiments" not in data


def test_supplemental_keeps_the_keys_the_library_indexes():
    seen = []
    parsers = SlimParsers(make_parsers(seen))

    parsers["READY_SUPPLEMENTAL"]({"guilds": [{}], "merged_presences": {"friends": [{}]}, "merged_members": [[]]})

    _, data = seen[0]
    assert data == {"guilds": [], "merged_presences": {}, "merged_members": []}


def test_trimming_leaves_absent_keys_absent():
    assert trim_ready({"user": {}}) == {"user": {}}
    assert trim_supplemental({}) == {"merged_presences": {}}


def test_a_library_without_the_expected_parsers_warns_and_still_works(caplog):
    caplog.set_level(logging.WARNING, logger="idlebot")

    parsers = SlimParsers({})
    handler = parsers["READY"]

    assert handler({}) is None
    assert parsers.summary() == "READY=1"
    assert any("parsers missing" in r.getMessage() for r in caplog.records)


def test_an_empty_summary_says_none():
    assert SlimParsers(make_parsers([])).summary() == "none"


async def test_the_alive_line_includes_the_ignored_events(caplog):
    clock = FakeClock()
    runner = make_runner(FakeClient(), clock=clock, stats=lambda: "MESSAGE_CREATE=3")
    caplog.set_level(logging.INFO, logger="idlebot")

    clock.advance(runner_module.LIVENESS_SECONDS)
    await runner.liveness()

    assert any("ignored gateway events: MESSAGE_CREATE=3" in r.getMessage() for r in caplog.records)


class FakeCurl:
    def __init__(self):
        self._curlm = object()
        self.actions = []

    def socket_action(self, sockfd, bitmask):
        self.actions.append((sockfd, bitmask))
        if len(self.actions) == 2:
            self._curlm = None

    async def _force_timeout(self):
        raise AssertionError("the original timer should be replaced")


async def test_the_curl_safety_tick_is_replaced_and_still_drives_curl():
    from idlebot.lean import slow_curl_safety_tick

    assert slow_curl_safety_tick(FakeCurl, -1, 0, interval=0) is True
    curl = FakeCurl()

    await curl._force_timeout()

    assert curl.actions == [(-1, 0), (-1, 0)]


def test_a_curl_without_the_safety_tick_is_left_alone(caplog):
    from idlebot.lean import slow_curl_safety_tick

    caplog.set_level(logging.WARNING, logger="idlebot")

    assert slow_curl_safety_tick(object, -1, 0) is False
    assert any("safety timer" in r.getMessage() for r in caplog.records)
