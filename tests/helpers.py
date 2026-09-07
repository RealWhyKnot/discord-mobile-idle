import asyncio

import pytest

from idlebot.controller import Debouncer, StatusController
from idlebot.runner import Runner

ONLINE = "online"
IDLE = "idle"
DND = "dnd"
INVISIBLE = "invisible"
OFFLINE = "offline"


class FakeClient:
    def __init__(self, status=ONLINE, on_mobile=False):
        self._status = status
        self._on_mobile = on_mobile
        self.calls = []
        self.raises = None

    def is_on_mobile(self):
        return self._on_mobile

    @property
    def status(self):
        return self._status

    def set_mobile(self, value):
        self._on_mobile = value

    def set_status(self, value):
        self._status = value

    async def change_presence(self, *, status, edit_settings=True):
        self.calls.append((status, edit_settings))
        if self.raises is not None:
            raise self.raises
        self._status = status


class FakeClock:
    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now

    def advance(self, seconds):
        self.now += seconds


def make_controller(managed=(ONLINE,), restore=ONLINE, grace_ticks=0):
    return StatusController(restore, set(managed), grace_ticks=grace_ticks)


def make_runner(
    client, managed=(ONLINE,), restore=ONLINE, on_polls=1, off_polls=1, observe=False, clock=None, grace_ticks=0
):
    return Runner(
        client,
        make_controller(managed, restore, grace_ticks),
        Debouncer(on_polls, off_polls),
        poll_seconds=0,
        observe=observe,
        clock=clock or FakeClock(),
    )


async def drive(runner, ticks):
    for _ in range(ticks):
        await runner.tick()


async def run_until_cancelled(runner, sleeps, monkeypatch, advance=0.0):
    seen = {"count": 0}

    async def fake_sleep(_seconds):
        seen["count"] += 1
        if advance:
            runner.clock.advance(advance)
        if seen["count"] >= sleeps:
            raise asyncio.CancelledError

    monkeypatch.setattr(asyncio, "sleep", fake_sleep)
    with pytest.raises(asyncio.CancelledError):
        await runner.run()
    return seen["count"]
