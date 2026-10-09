import asyncio
import gc
import logging
import time
import tracemalloc

from helpers import INVISIBLE, ONLINE, FakeClient, FakeClock, MemoryMarker

from idlebot import lean
from idlebot.controller import Debouncer, StatusController
from idlebot.lean import SlimParsers
from idlebot.runner import LIVENESS_SECONDS, Runner

POLL = 10
DAY = 86400
TICK_BUDGET_US = 100
DROP_BUDGET_US = 5
DAY_GROWTH_BUDGET = 16 * 1024


def production_runner(client, clock, marker=None):
    controller = StatusController(ONLINE, {ONLINE}, clock=clock)
    return Runner(client, controller, Debouncer(1, 4), POLL, clock=clock, marker=marker)


async def best_of(rounds, work):
    best = None
    for _ in range(rounds):
        start = time.perf_counter()
        await work()
        elapsed = time.perf_counter() - start
        best = elapsed if best is None else min(best, elapsed)
    return best


async def traced_growth(work):
    gc.collect()
    tracemalloc.start()
    try:
        before = tracemalloc.get_traced_memory()[0]
        await work
        gc.collect()
        return tracemalloc.get_traced_memory()[0] - before
    finally:
        tracemalloc.stop()


async def test_a_steady_tick_costs_microseconds():
    clock = FakeClock()
    runner = production_runner(FakeClient(), clock)
    ticks = 2000

    async def batch():
        for _ in range(ticks):
            await runner.tick()

    await batch()
    per_tick_us = await best_of(3, batch) / ticks * 1e6

    assert per_tick_us < TICK_BUDGET_US, "%.1f us per tick" % per_tick_us


async def test_steady_ticks_write_log_and_touch_disk_nothing(caplog):
    clock = FakeClock()
    client = FakeClient()
    marker = MemoryMarker()
    runner = production_runner(client, clock, marker)
    caplog.set_level(logging.INFO, logger="idlebot")

    for _ in range(1000):
        await runner.tick()
        await runner.liveness()

    assert client.calls == [] and client.presence == []
    assert client.token_checks == 0
    assert marker.writes == []
    assert caplog.records == []


async def simulate_day(runner, client, clock, trim=False):
    phone_cycles = 0
    for second in range(0, DAY, POLL):
        hour, minute = divmod(second // 60, 60)
        night = 2 <= hour < 7
        client.others = 0 if night else 1
        phone = not night and minute < 10
        if phone and not client.is_on_mobile():
            phone_cycles += 1
        client.set_mobile(phone)
        await runner.tick()
        await runner.liveness()
        await asyncio.sleep(0)
        clock.advance(POLL)
        if trim:
            client.calls.clear()
    return phone_cycles


async def test_a_simulated_day_costs_two_writes_per_phone_cycle_and_two_pushes():
    clock = FakeClock()
    client = FakeClient()
    runner = production_runner(client, clock, MemoryMarker())

    phone_cycles = await simulate_day(runner, client, clock)

    assert phone_cycles == 19
    assert len(client.calls) == runner.transitions == 2 * phone_cycles
    assert client.presence == [INVISIBLE, ONLINE]
    assert client.token_checks == DAY // LIVENESS_SECONDS - 1


async def test_a_simulated_day_does_not_grow_memory():
    clock = FakeClock()
    client = FakeClient()
    runner = production_runner(client, clock, MemoryMarker())
    await simulate_day(runner, client, clock, trim=True)

    grown = await traced_growth(simulate_day(runner, client, clock, trim=True))

    assert grown < DAY_GROWTH_BUDGET, "%d more bytes held after a day" % grown


async def test_the_run_loop_sleeps_between_polls():
    client = FakeClient()
    runner = production_runner(client, time.monotonic)
    runner.poll_seconds = 0.05
    ticks = []
    real_tick = runner.tick

    async def counted():
        ticks.append(time.monotonic())
        if len(ticks) > 40:
            raise asyncio.CancelledError
        await real_tick()

    runner.tick = counted
    task = asyncio.create_task(runner.run())
    await asyncio.sleep(0.25)
    runner.wake.set()
    await asyncio.sleep(0.25)
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass

    assert 4 <= len(ticks) <= 20, "%d ticks in half a second" % len(ticks)


async def test_session_churn_while_the_phone_is_handled_does_not_wake_the_loop():
    client = FakeClient(on_mobile=True)
    runner = production_runner(client, FakeClock())
    await runner.tick()
    assert runner.debouncer.stable is True

    for _ in range(1000):
        runner.wake_if_mobile()

    assert not runner.wake.is_set()


async def test_a_dropped_event_is_cheap_and_kept_nowhere():
    parsers = SlimParsers({})
    payload = {"user": {"id": "1"}, "status": "online", "activities": [{"name": "x" * 200}]}
    calls = 20000

    async def drop():
        for _ in range(calls):
            parsers["PRESENCE_UPDATE"](payload)

    per_event_us = await best_of(3, drop) / calls * 1e6

    async def flood():
        for n in range(5000):
            parsers["PRESENCE_UPDATE"]({"user": {"id": str(n)}, "blob": "y" * 500})

    grown = await traced_growth(flood())

    assert per_event_us < DROP_BUDGET_US, "%.2f us per dropped event" % per_event_us
    assert grown < 16 * 1024, "%d bytes held after 5000 dropped events" % grown
    assert parsers.dropped["PRESENCE_UPDATE"] == 3 * calls + 5000


def big_ready():
    return {
        "user": {"id": "1"},
        "sessions": [{"session_id": "a"}],
        "guilds": [
            {"id": str(n), "channels": [{"id": str(c), "name": "c" * 40} for c in range(40)]} for n in range(100)
        ],
        "relationships": [{"id": str(n), "type": 1} for n in range(300)],
        "private_channels": [{"id": str(n), "recipient_ids": [str(n)]} for n in range(300)],
        "users": [{"id": str(n), "username": "u" * 20} for n in range(800)],
        "read_state": {"entries": [{"id": str(n), "last_message_id": str(n)} for n in range(3000)]},
        "experiments": [[n, 0, 1, -1, 0] for n in range(400)],
    }


def test_ready_bulk_is_freed_before_the_library_parses_it():
    seen = {}

    def library_ready(data):
        seen["at_parse"] = tracemalloc.get_traced_memory()[0]

    parsers = SlimParsers({"READY": library_ready})
    tracemalloc.start()
    try:
        empty = tracemalloc.get_traced_memory()[0]
        ready = big_ready()
        loaded = tracemalloc.get_traced_memory()[0] - empty
        parsers["READY"](ready)
        left = seen["at_parse"] - empty
    finally:
        tracemalloc.stop()

    assert loaded > 1_000_000
    assert left < loaded * 0.02, "%d of %d bytes still held when the library parses READY" % (left, loaded)


async def test_the_curl_safety_timer_runs_once_a_second(monkeypatch):
    class Curl:
        def __init__(self):
            self._curlm = object()
            self.actions = 0

        def socket_action(self, sockfd, bitmask):
            self.actions += 1

        async def _force_timeout(self):
            raise AssertionError("the original timer should be replaced")

    clock = FakeClock(0.0)
    curl = Curl()

    async def fake_sleep(seconds):
        clock.advance(seconds)
        if clock.now >= 60:
            curl._curlm = None

    assert lean.slow_curl_safety_tick(Curl, -1, 0) is True
    monkeypatch.setattr(lean.asyncio, "sleep", fake_sleep)
    await curl._force_timeout()

    assert lean.CURL_SAFETY_TICK >= 1.0
    assert curl.actions <= 60
