import asyncio
import logging

import pytest
from helpers import (
    DND,
    IDLE,
    INVISIBLE,
    ONLINE,
    FakeClient,
    make_runner,
    run_until_cancelled,
)


async def test_full_lifecycle_writes_once_each_way():
    client = FakeClient()
    runner = make_runner(client)

    await runner.tick()
    assert client.calls == []

    client.set_mobile(True)
    await runner.tick()
    assert client.calls == [(IDLE, True)]
    assert client.status == IDLE

    await runner.tick()
    assert len(client.calls) == 1

    client.set_mobile(False)
    await runner.tick()
    assert client.calls == [(IDLE, True), (ONLINE, True)]
    assert client.status == ONLINE


async def test_saved_status_is_restored_not_default():
    client = FakeClient(status=DND)
    runner = make_runner(client, managed=(ONLINE, DND), restore=ONLINE)

    client.set_mobile(True)
    await runner.tick()
    assert client.status == IDLE

    client.set_mobile(False)
    await runner.tick()
    assert client.status == DND


async def test_adopted_idle_is_not_rewritten():
    client = FakeClient(status=IDLE, on_mobile=True)
    runner = make_runner(client)

    await runner.tick()
    assert client.calls == []
    assert runner.controller.holding

    client.set_mobile(False)
    await runner.tick()
    assert client.calls == [(ONLINE, True)]


async def test_hidden_status_is_never_touched():
    client = FakeClient(status=INVISIBLE)
    runner = make_runner(client)

    client.set_mobile(True)
    await runner.tick()
    client.set_mobile(False)
    await runner.tick()

    assert client.calls == []
    assert client.status == INVISIBLE


async def test_observe_mode_writes_nothing():
    client = FakeClient()
    runner = make_runner(client, observe=True)

    client.set_mobile(True)
    await runner.tick()

    assert client.calls == []
    assert client.status == ONLINE


async def test_observe_mode_simulates_its_own_writes():
    client = FakeClient()
    runner = make_runner(client, observe=True)

    client.set_mobile(True)
    await runner.tick()
    assert runner.simulated == IDLE

    client.set_mobile(False)
    await runner.tick()
    assert runner.simulated == ONLINE

    assert client.calls == []
    assert client.status == ONLINE


async def test_failed_write_is_retried_on_a_later_tick():
    client = FakeClient()
    runner = make_runner(client)
    client.set_mobile(True)
    client.raises = RuntimeError("boom")

    with pytest.raises(RuntimeError):
        await runner.tick()
    assert runner.pending == IDLE

    client.raises = None
    await runner.tick()
    assert client.calls == [(IDLE, True), (IDLE, True)]
    assert client.status == IDLE
    assert runner.pending is None


async def test_run_survives_exceptions_and_keeps_ticking(monkeypatch):
    client = FakeClient()
    runner = make_runner(client)
    client.set_mobile(True)
    client.raises = RuntimeError("boom")
    start = runner.last_tick

    await run_until_cancelled(runner, 3, monkeypatch, advance=10.0)

    assert len(client.calls) == 3
    assert runner.last_tick > start


async def test_run_records_transitions(monkeypatch):
    client = FakeClient()
    runner = make_runner(client)
    client.set_mobile(True)

    await run_until_cancelled(runner, 2, monkeypatch, advance=10.0)

    assert runner.transitions == 1
    assert client.status == IDLE


async def test_liveness_logs_after_the_interval(monkeypatch, caplog):
    client = FakeClient()
    runner = make_runner(client)
    caplog.set_level(logging.INFO, logger="idlebot")

    await run_until_cancelled(runner, 3, monkeypatch, advance=1000.0)

    messages = [record.getMessage() for record in caplog.records]
    assert sum("alive uptime=" in message for message in messages) == 1


async def test_hand_back_writes_nothing_when_not_holding():
    client = FakeClient()
    runner = make_runner(client)

    await runner.hand_back()

    assert client.calls == []


async def test_hand_back_writes_nothing_in_observe_mode():
    client = FakeClient()
    runner = make_runner(client, observe=True)
    client.set_mobile(True)
    await runner.tick()

    await runner.hand_back()

    assert client.calls == []
    assert runner.controller.holding


async def test_hand_back_restores_the_saved_status():
    client = FakeClient(status=DND)
    runner = make_runner(client, managed=(ONLINE, DND))
    client.set_mobile(True)
    await runner.tick()
    assert client.status == IDLE

    await runner.hand_back()

    assert client.status == DND


async def test_hand_back_falls_back_to_the_default_restore():
    client = FakeClient(status=IDLE)
    runner = make_runner(client)
    client.set_mobile(True)
    await runner.tick()

    await runner.hand_back()

    assert client.status == ONLINE


async def test_wake_if_mobile_sets_on_rising_edge():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)

    runner.wake_if_mobile()

    assert runner.wake.is_set()


async def test_wake_if_mobile_ignores_when_no_mobile():
    client = FakeClient()
    runner = make_runner(client)

    runner.wake_if_mobile()

    assert not runner.wake.is_set()


async def test_wake_if_mobile_ignores_when_already_stable():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    runner.debouncer.stable = True

    runner.wake_if_mobile()

    assert not runner.wake.is_set()


async def test_wait_for_wake_returns_promptly_and_clears():
    client = FakeClient()
    runner = make_runner(client)
    runner.poll_seconds = 60
    runner.wake.set()

    await runner.wait_for_wake()

    assert not runner.wake.is_set()


async def test_wait_for_wake_times_out_at_the_poll_interval():
    client = FakeClient()
    runner = make_runner(client)

    await runner.wait_for_wake()

    assert not runner.wake.is_set()


async def test_session_wake_causes_a_prompt_tick():
    client = FakeClient()
    runner = make_runner(client)
    runner.poll_seconds = 30
    task = asyncio.create_task(runner.run())
    await asyncio.sleep(0.01)
    assert client.calls == []

    client.set_mobile(True)
    runner.wake_if_mobile()
    await asyncio.sleep(0.01)

    assert client.calls == [(IDLE, True)]
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task


async def test_hand_back_leaves_a_manual_override_alone():
    client = FakeClient()
    runner = make_runner(client)
    client.set_mobile(True)
    await runner.tick()
    client.set_status(DND)
    client.calls.clear()

    await runner.hand_back()

    assert client.calls == []


async def test_hand_back_restores_when_the_idle_write_has_not_echoed():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    assert client.calls == [(IDLE, True)]

    client.set_status(ONLINE)
    await runner.hand_back()

    assert client.calls == [(IDLE, True), (ONLINE, True)]


async def test_hand_back_still_refuses_a_status_someone_else_set():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()

    client.set_status(INVISIBLE)
    await runner.hand_back()

    assert client.calls == [(IDLE, True)]
