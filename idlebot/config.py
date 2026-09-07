import os

from .controller import ADOPTABLE, ALL_STATUSES


class ConfigError(Exception):
    pass


class Config:
    def __init__(self, token, poll_seconds, restore, managed, on_polls, off_polls, watchdog_seconds):
        self.token = token
        self.poll_seconds = poll_seconds
        self.restore = restore
        self.managed = managed
        self.on_polls = on_polls
        self.off_polls = off_polls
        self.watchdog_seconds = watchdog_seconds


def _int(env, name, default):
    raw = env.get(name, "").strip()
    if not raw:
        return default
    try:
        value = int(raw)
    except ValueError:
        raise ConfigError("%s must be an integer, got %r" % (name, raw))
    if value < 1:
        raise ConfigError("%s must be >= 1, got %d" % (name, value))
    return value


def load_config(env=None):
    env = os.environ if env is None else env

    token = env.get("DISCORD_TOKEN", "").strip()
    if not token:
        raise ConfigError("DISCORD_TOKEN is required")

    restore = env.get("RESTORE_STATUS", "online").strip().lower()
    if restore not in ADOPTABLE:
        raise ConfigError("RESTORE_STATUS must be one of %s, got %r" % (sorted(ADOPTABLE), restore))

    managed = {s.strip().lower() for s in env.get("MANAGED_STATUSES", "online").split(",")}
    managed.discard("")
    if not managed:
        raise ConfigError("MANAGED_STATUSES must name at least one status")
    unknown = managed - ALL_STATUSES
    if unknown:
        raise ConfigError("MANAGED_STATUSES has unknown values: %s" % sorted(unknown))
    unmanageable = managed - ADOPTABLE
    if unmanageable:
        raise ConfigError(
            "MANAGED_STATUSES may only contain %s, got %s" % (sorted(ADOPTABLE), sorted(unmanageable))
        )

    poll_seconds = _int(env, "POLL_SECONDS", 10)
    watchdog_seconds = _int(env, "WATCHDOG_SECONDS", 120)
    if watchdog_seconds <= poll_seconds:
        raise ConfigError(
            "WATCHDOG_SECONDS (%d) must exceed POLL_SECONDS (%d)" % (watchdog_seconds, poll_seconds)
        )

    return Config(
        token=token,
        poll_seconds=poll_seconds,
        restore=restore,
        managed=managed,
        on_polls=_int(env, "ON_DEBOUNCE_POLLS", 2),
        off_polls=_int(env, "OFF_DEBOUNCE_POLLS", 4),
        watchdog_seconds=watchdog_seconds,
    )
