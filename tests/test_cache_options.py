import logging

from idlebot.runner import check_cache_options


class Flags:
    def __init__(self, value):
        self.value = value


class State:
    def __init__(self, max_messages=None, chunk=False, subscribe=False, flags=0):
        self.max_messages = max_messages
        self._chunk_guilds = chunk
        self._subscribe_guilds = subscribe
        if flags is not None:
            self.member_cache_flags = Flags(flags)


def test_all_options_applied_reports_nothing():
    assert check_cache_options(State()) == []


def test_applied_state_logs_confirmation(caplog):
    caplog.set_level(logging.INFO, logger="idlebot")
    check_cache_options(State())
    assert any("cache options applied" in r.getMessage() for r in caplog.records)


def test_message_cache_left_on_is_reported():
    problems = check_cache_options(State(max_messages=1000))
    assert len(problems) == 1
    assert "max_messages" in problems[0]


def test_chunking_left_on_is_reported():
    problems = check_cache_options(State(chunk=True))
    assert len(problems) == 1
    assert "_chunk_guilds" in problems[0]


def test_guild_subscriptions_left_on_is_reported():
    problems = check_cache_options(State(subscribe=True))
    assert len(problems) == 1
    assert "_subscribe_guilds" in problems[0]


def test_member_cache_flags_set_is_reported():
    problems = check_cache_options(State(flags=1))
    assert len(problems) == 1
    assert "member_cache_flags" in problems[0]


def test_missing_member_cache_flags_is_reported():
    problems = check_cache_options(State(flags=None))
    assert len(problems) == 1
    assert "member_cache_flags" in problems[0]


def test_renamed_internals_are_reported_not_fatal():
    problems = check_cache_options(object())
    assert len(problems) == 4


def test_problems_are_warned_not_raised(caplog):
    caplog.set_level(logging.WARNING, logger="idlebot")
    check_cache_options(State(chunk=True, subscribe=True))
    warnings = [r for r in caplog.records if r.levelno == logging.WARNING]
    assert len(warnings) == 1
    assert "memory will be higher" in warnings[0].getMessage()
