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
    def __init__(self, status=ONLINE, on_mobile=False, others=1, saved=None, echo=True):
        self._status = status
        self._on_mobile = on_mobile
        self.others = others
        self.saved = status if saved is None else saved
        self.echo = echo
        self.calls = []
        self.presence = []
        self.raises = None

    def is_on_mobile(self):
        return self._on_mobile

    def other_sessions(self):
        return self.others

    @property
    def status(self):
        return self._status

    @property
    def saved_status(self):
        return self.saved

    def set_mobile(self, value):
        self._on_mobile = value

    def set_status(self, value):
        self._status = value

    async def change_presence(self, *, status, edit_settings=True):
        self.calls.append((status, edit_settings))
        if self.raises is not None:
            raise self.raises
        self._status = status

    async def hide(self):
        self.presence.append(INVISIBLE)
        self._status = INVISIBLE

    async def unhide(self, status):
        self.presence.append(status)
        if self.echo:
            self._status = status

    def settle(self):
        self._status = self.presence[-1]


class FakeClock:
    def __init__(self, now=1000.0):
        self.now = now

    def __call__(self):
        return self.now

    def advance(self, seconds):
        self.now += seconds


def make_controller(managed=(ONLINE,), restore=ONLINE, grace_seconds=0, clock=None):
    return StatusController(restore, set(managed), grace_seconds=grace_seconds, clock=clock or FakeClock())


def make_runner(
    client, managed=(ONLINE,), restore=ONLINE, on_polls=1, off_polls=1, observe=False, clock=None, grace_seconds=0
):
    clock = clock or FakeClock()
    return Runner(
        client,
        make_controller(managed, restore, grace_seconds, clock),
        Debouncer(on_polls, off_polls),
        poll_seconds=0,
        observe=observe,
        clock=clock,
    )


async def drive(runner, ticks):
    for _ in range(ticks):
        await runner.tick()


async def run_until_cancelled(runner, sleeps, monkeypatch, advance=0.0):
    seen = {"count": 0}

    async def fake_wait():
        seen["count"] += 1
        if advance:
            runner.clock.advance(advance)
        if seen["count"] >= sleeps:
            raise asyncio.CancelledError

    monkeypatch.setattr(runner, "wait_for_wake", fake_wait)
    with pytest.raises(asyncio.CancelledError):
        await runner.run()
    return seen["count"]
