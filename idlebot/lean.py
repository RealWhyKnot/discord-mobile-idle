import asyncio
import logging
from collections import Counter

log = logging.getLogger("idlebot")

CURL_SAFETY_TICK = 1.0

KEEP_EVENTS = frozenset(
    {
        "READY",
        "READY_SUPPLEMENTAL",
        "RESUMED",
        "SESSIONS_REPLACE",
        "USER_SETTINGS_PROTO_UPDATE",
        "USER_UPDATE",
    }
)
READY_LISTS = (
    "guilds",
    "merged_members",
    "relationships",
    "private_channels",
    "users",
    "experiments",
    "guild_experiments",
    "connected_accounts",
)
READY_MAPS = ("read_state", "user_guild_settings")
SUPPLEMENTAL_LISTS = ("guilds", "merged_members", "lazy_private_channels")


def trim_ready(data):
    for key in READY_LISTS:
        if key in data:
            data[key] = []
    for key in READY_MAPS:
        if key in data:
            data[key] = {}
    return data


def trim_supplemental(data):
    for key in SUPPLEMENTAL_LISTS:
        if key in data:
            data[key] = []
    data["merged_presences"] = {}
    return data


def _ignore(data):
    return None


class SlimParsers(dict):
    def __init__(self, parsers):
        super().__init__((name, parsers[name]) for name in KEEP_EVENTS if name in parsers)
        self.dropped = Counter()
        missing = sorted(KEEP_EVENTS - set(self))
        if missing:
            log.warning("gateway parsers missing, the library may have changed: %s", ", ".join(missing))
        ready = self.get("READY")
        if ready is not None:
            self["READY"] = lambda data: ready(trim_ready(data))
        supplemental = self.get("READY_SUPPLEMENTAL")
        if supplemental is not None:
            self["READY_SUPPLEMENTAL"] = lambda data: supplemental(trim_supplemental(data))

    def __missing__(self, name):
        self.dropped[name] += 1
        return _ignore

    def summary(self, limit=8):
        top = self.dropped.most_common(limit)
        return " ".join("%s=%d" % item for item in top) or "none"


def slow_curl_safety_tick(curl_class, socket_timeout, poll_none, interval=CURL_SAFETY_TICK):
    if not hasattr(curl_class, "_force_timeout"):
        log.warning("curl_cffi has no safety timer to slow down, the library may have changed")
        return False

    async def force_timeout(self):
        while self._curlm:
            self.socket_action(socket_timeout, poll_none)
            await asyncio.sleep(interval)

    curl_class._force_timeout = force_timeout
    return True
