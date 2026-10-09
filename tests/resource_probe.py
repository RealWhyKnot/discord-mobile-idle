import asyncio
import json
import os
import random
import sys
import sysconfig
import time
import zlib

NO_LIBRARY = 3
NO_MEMORY_READING = 4
GUILDS = 100
EVENTS = 5000
IDLE_SECONDS = 2.0


def linux_memory():
    values = {}
    with open("/proc/self/status") as handle:
        for line in handle:
            name, _, rest = line.partition(":")
            if name in ("VmRSS", "VmHWM"):
                values[name] = int(rest.split()[0]) / 1024.0
    return values["VmRSS"], values["VmHWM"]


def reset_linux_peak():
    try:
        with open("/proc/self/clear_refs", "w") as handle:
            handle.write("5")
    except OSError:
        pass


def windows_memory():
    import ctypes
    from ctypes import wintypes

    class Counters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    current = ctypes.windll.kernel32.GetCurrentProcess
    current.restype = wintypes.HANDLE
    read = ctypes.windll.psapi.GetProcessMemoryInfo
    read.argtypes = [wintypes.HANDLE, ctypes.POINTER(Counters), wintypes.DWORD]
    read.restype = wintypes.BOOL
    counters = Counters()
    counters.cb = ctypes.sizeof(Counters)
    if not read(current(), ctypes.byref(counters), counters.cb):
        raise ctypes.WinError()
    return counters.WorkingSetSize / 2**20, counters.PeakWorkingSetSize / 2**20


if sys.platform.startswith("linux"):
    memory, reset_peak = linux_memory, reset_linux_peak
elif sys.platform == "win32":
    memory, reset_peak = windows_memory, lambda: None
else:
    sys.exit(NO_MEMORY_READING)


def rss():
    return memory()[0]


def runtime():
    if sys.platform == "win32":
        return "windows"
    return "musl" if "musl" in (sysconfig.get_config_var("HOST_GNU_TYPE") or "") else "glibc"


rand = random.Random(7)


def snowflake():
    return str(rand.randrange(10**17, 10**18))


def user():
    return {
        "id": snowflake(),
        "username": "user%d" % rand.randrange(10**6),
        "global_name": "Some Name",
        "discriminator": "0",
        "avatar": "%032x" % rand.getrandbits(128),
        "public_flags": 0,
    }


def channel():
    return {
        "id": snowflake(),
        "type": rand.choice([0, 0, 2, 4, 5]),
        "name": "channel-%d" % rand.randrange(1000),
        "position": rand.randrange(100),
        "parent_id": snowflake(),
        "topic": "t" * rand.randrange(0, 120),
        "last_message_id": snowflake(),
        "permission_overwrites": [
            {"id": snowflake(), "type": 0, "allow": "1024", "deny": "0"} for _ in range(rand.randrange(0, 5))
        ],
    }


def guild():
    gid = snowflake()
    return {
        "id": gid,
        "properties": {"id": gid, "name": "Guild", "icon": None, "features": ["COMMUNITY", "NEWS"]},
        "channels": [channel() for _ in range(rand.randrange(10, 80))],
        "roles": [
            {"id": snowflake(), "name": "role-%d" % i, "color": 0, "permissions": "0", "position": i}
            for i in range(rand.randrange(5, 60))
        ],
        "emojis": [{"id": snowflake(), "name": "emoji_%d" % i} for i in range(rand.randrange(0, 80))],
        "member_count": rand.randrange(10, 50000),
        "lazy": True,
    }


def session(sid, kind):
    return {
        "session_id": sid,
        "status": "online",
        "activities": [],
        "active": False,
        "client_info": {"client": kind, "os": "windows", "version": 0},
    }


def ready():
    return {
        "t": "READY",
        "s": 1,
        "op": 0,
        "d": {
            "v": 9,
            "user": dict(user(), verified=True, mfa_enabled=True, flags=0, premium_type=0),
            "session_id": "s1",
            "resume_gateway_url": "wss://gateway.discord.gg",
            "sessions": [session("s1", "web"), session("s2", "desktop"), session("all", "unknown")],
            "user_settings_proto": "",
            "guilds": [guild() for _ in range(GUILDS)],
            "users": [user() for _ in range(800)],
            "relationships": [{"id": snowflake(), "type": 1, "user_id": snowflake()} for _ in range(300)],
            "private_channels": [
                {"id": snowflake(), "type": 1, "recipient_ids": [snowflake()], "last_message_id": snowflake()}
                for _ in range(250)
            ],
            "read_state": {"entries": [{"id": snowflake(), "last_message_id": snowflake()} for _ in range(3000)]},
            "user_guild_settings": {"entries": [{"guild_id": snowflake()} for _ in range(GUILDS)]},
            "experiments": [[rand.getrandbits(32), 0, 1, -1, 0] for _ in range(400)],
            "guild_experiments": [[rand.getrandbits(32), None, 0, [], [], [], 0, 0] for _ in range(300)],
            "merged_members": [[{"user_id": snowflake(), "roles": []}] for _ in range(GUILDS)],
        },
    }


def presence():
    return {
        "t": "PRESENCE_UPDATE",
        "s": 0,
        "op": 0,
        "d": {
            "user": {"id": snowflake()},
            "status": rand.choice(["online", "idle", "dnd"]),
            "client_status": {"desktop": "online"},
            "activities": [{"type": 0, "name": "Some Game", "created_at": 1700000000000}],
            "guild_id": snowflake(),
        },
    }


def message():
    return {
        "t": "MESSAGE_CREATE",
        "s": 0,
        "op": 0,
        "d": {
            "id": snowflake(),
            "channel_id": snowflake(),
            "guild_id": snowflake(),
            "author": user(),
            "member": {"roles": [snowflake()], "joined_at": "2024-01-01T00:00:00+00:00"},
            "content": "hello " * rand.randrange(1, 40),
            "timestamp": "2026-10-08T12:00:00.000000+00:00",
            "mentions": [],
            "attachments": [],
            "embeds": [],
            "type": 0,
        },
    }


class Stream:
    def __init__(self):
        from discord import utils

        self.source = utils._ZSTD_SOURCE
        if self.source == "zstandard":
            import zstandard

            self.compressor = zstandard.ZstdCompressor().compressobj()
        elif self.source == "compression.zstd":
            from compression import zstd

            self.compressor = zstd.ZstdCompressor()
        else:
            self.compressor = zlib.compressobj()

    def pack(self, raw):
        if self.source == "zstandard":
            import zstandard

            return self.compressor.compress(raw) + self.compressor.flush(zstandard.COMPRESSOBJ_FLUSH_BLOCK)
        if self.source == "compression.zstd":
            return self.compressor.compress(raw, self.compressor.FLUSH_BLOCK)
        return self.compressor.compress(raw) + self.compressor.flush(zlib.Z_SYNC_FLUSH)

    def frame(self, payload):
        raw = json.dumps(payload, separators=(",", ":")).encode()
        return self.pack(raw), len(raw)


def gateway(client):
    from discord.gateway import DiscordWebSocket

    ws = DiscordWebSocket(None, loop=asyncio.get_running_loop())
    ws._connection = client._connection
    ws._discord_parsers = client._connection.parsers
    ws._dispatch = client.dispatch
    ws.call_hooks = client._connection.call_hooks

    async def no_voice_state():
        return None

    ws.voice_state = no_voice_state
    client.ws = ws
    client._connection._update_references(ws)
    return ws


async def idle_curl_actions():
    from curl_cffi import aio

    curl = aio.AsyncCurl()
    actions = []
    original = curl.socket_action

    def counted(sockfd, bitmask):
        actions.append(sockfd)
        return original(sockfd, bitmask)

    curl.socket_action = counted
    await asyncio.sleep(IDLE_SECONDS)
    await curl.close()
    return len(actions)


async def measure(report, client):
    state = client._connection
    stream = Stream()
    ready_frame, ready_bytes = stream.frame(ready())
    event_frames = [stream.frame(presence() if rand.random() < 0.7 else message())[0] for _ in range(EVENTS)]
    ws = gateway(client)

    report["ready_json_mb"] = ready_bytes / 2**20
    report["before_ready_mb"] = before = rss()
    reset_peak()
    await ws.received_message(ready_frame)
    del ready_frame
    now, peak = memory()
    report["ready_peak_mb"] = max(peak, now) - before

    start = time.perf_counter()
    for frame in event_frames:
        await ws.received_message(frame)
    report["event_us"] = (time.perf_counter() - start) / EVENTS * 1e6
    del event_frames
    report["held_after_ready_mb"] = rss() - before

    report["sessions"] = len(state._sessions)
    report["guilds"] = len(state._guilds)
    report["relationships"] = len(state._relationships)
    report["private_channels"] = len(state._private_channels)
    report["users"] = len(state._users)
    report["dropped"] = sum(state.parsers.dropped.values())
    report["idle_curl_actions"] = await idle_curl_actions()


def main():
    report = {"python": sys.version.split()[0], "runtime": runtime()}
    try:
        import discord  # noqa: F401
    except ImportError:
        return NO_LIBRARY
    from idlebot.discord_client import make_client
    from idlebot.runner import check_afk, check_cache_options

    client, _ = make_client()
    report["startup_mb"] = rss()
    report["cache_problems"] = check_cache_options(client._connection)
    report["afk"] = check_afk(client._connection)
    asyncio.run(measure(report, client))
    print(json.dumps(report))
    return 0


if __name__ == "__main__":
    os.environ.setdefault("PYTHONHASHSEED", "0")
    sys.exit(main())
