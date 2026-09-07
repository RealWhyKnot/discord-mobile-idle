import pytest

from idlebot.config import ConfigError, load_config

BASE = {"DISCORD_TOKEN": "token"}


def env(**overrides):
    merged = dict(BASE)
    merged.update(overrides)
    return merged


def test_defaults():
    config = load_config(env())
    assert config.token == "token"
    assert config.poll_seconds == 10
    assert config.restore == "online"
    assert config.managed == {"online"}
    assert config.on_polls == 2
    assert config.off_polls == 4
    assert config.watchdog_seconds == 120


def test_token_is_required():
    with pytest.raises(ConfigError):
        load_config({})


def test_blank_token_is_rejected():
    with pytest.raises(ConfigError):
        load_config({"DISCORD_TOKEN": "   "})


@pytest.mark.parametrize("value", ["idle", "invisible", "offline", "bogus"])
def test_restore_status_must_be_adoptable(value):
    with pytest.raises(ConfigError):
        load_config(env(RESTORE_STATUS=value))


def test_restore_status_accepts_dnd():
    assert load_config(env(RESTORE_STATUS="DND")).restore == "dnd"


def test_managed_rejects_unknown_status():
    with pytest.raises(ConfigError):
        load_config(env(MANAGED_STATUSES="online,bogus"))


@pytest.mark.parametrize("value", ["invisible", "offline", "idle"])
def test_managed_rejects_unmanageable_status(value):
    with pytest.raises(ConfigError):
        load_config(env(MANAGED_STATUSES="online," + value))


def test_managed_must_not_be_empty():
    with pytest.raises(ConfigError):
        load_config(env(MANAGED_STATUSES=" , "))


def test_managed_accepts_dnd():
    assert load_config(env(MANAGED_STATUSES="online, dnd")).managed == {"online", "dnd"}


def test_non_integer_is_rejected():
    with pytest.raises(ConfigError):
        load_config(env(POLL_SECONDS="ten"))


def test_non_positive_integer_is_rejected():
    with pytest.raises(ConfigError):
        load_config(env(POLL_SECONDS="0"))


def test_blank_integer_falls_back_to_default():
    assert load_config(env(POLL_SECONDS="  ")).poll_seconds == 10


def test_watchdog_must_exceed_poll_interval():
    with pytest.raises(ConfigError):
        load_config(env(POLL_SECONDS="120", WATCHDOG_SECONDS="120"))


def test_debounce_polls_are_configurable():
    config = load_config(env(ON_DEBOUNCE_POLLS="5", OFF_DEBOUNCE_POLLS="9"))
    assert config.on_polls == 5
    assert config.off_polls == 9
