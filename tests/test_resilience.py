import asyncio
import logging

import pytest
from helpers import (
    DND,
    IDLE,
    INVISIBLE,
    OFFLINE,
    ONLINE,
    FakeClient,
    FakeClock,
    MemoryMarker,
    drive,
    make_runner,
    run_until_cancelled,
)
from test_shutdown import FakeGateway, catch_stop

from idlebot import runner as runner_module
from idlebot.controller import Debouncer


class Rejected(Exception):
    status = 401


class ServerError(Exception):
    status = 500


async def test_a_take_writes_the_hold_marker_and_the_hand_back_clears_it():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker)

    await runner.tick()
    assert marker.value == ONLINE

    client.set_mobile(False)
    await runner.tick()
    assert marker.value is None
    assert marker.writes == [ONLINE, None]


async def test_the_marker_is_only_written_when_it_changes():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker)

    await drive(runner, 4)

    assert marker.writes == [ONLINE]


async def test_choosing_invisible_mid_hold_clears_the_marker():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker)
    await runner.tick()

    runner.note_settings(INVISIBLE, IDLE)
    await runner.tick()

    assert marker.value is None


async def test_recovery_restores_once_the_phone_is_gone():
    client = FakeClient(status=IDLE, saved=IDLE)
    marker = MemoryMarker(DND)
    runner = make_runner(client, managed=(ONLINE, DND), marker=marker)

    runner.recover()
    runner.seed_user_status()
    await runner.tick()

    assert runner.user_status is None
    assert client.calls == [(DND, True)]
    assert marker.value is None


async def test_recovery_keeps_holding_while_the_phone_is_still_on():
    client = FakeClient(status=IDLE, on_mobile=True, saved=IDLE)
    marker = MemoryMarker(ONLINE)
    runner = make_runner(client, marker=marker)

    runner.recover()
    await drive(runner, 3)
    assert client.calls == []
    assert runner.controller.holding is True

    client.set_mobile(False)
    await runner.tick()
    assert client.calls == [(ONLINE, True)]


async def test_recovery_with_no_marker_does_nothing():
    runner = make_runner(FakeClient(), marker=MemoryMarker())

    runner.recover()

    assert runner.controller.holding is False


async def test_recovery_is_skipped_in_observe_mode_and_without_a_marker():
    observed = make_runner(FakeClient(), observe=True, marker=MemoryMarker(ONLINE))
    observed.recover()
    bare = make_runner(FakeClient())
    bare.recover()

    assert observed.controller.holding is False
    assert bare.controller.holding is False


async def test_a_status_changed_during_the_restart_is_not_overwritten():
    client = FakeClient(status=DND, saved=DND)
    marker = MemoryMarker(ONLINE)
    runner = make_runner(client, managed=(ONLINE, DND), marker=marker)

    runner.recover()
    runner.seed_user_status()
    await drive(runner, 3)

    assert client.calls == []
    assert marker.value is None


async def test_ticks_do_nothing_while_disconnected():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)

    runner.note_disconnected()
    await drive(runner, 3)

    assert client.calls == []


async def test_a_reconnect_sends_our_presence_again():
    client = FakeClient(others=0)
    runner = make_runner(client)
    await runner.tick()
    assert client.presence == [INVISIBLE]

    runner.note_disconnected()
    runner.note_connected()
    await runner.tick()

    assert client.presence == [INVISIBLE, INVISIBLE]
    assert runner.reassert is False


async def test_a_reconnect_before_any_write_sends_nothing():
    client = FakeClient()
    runner = make_runner(client)

    runner.note_connected()
    await runner.tick()

    assert client.presence == []


async def test_a_second_disconnect_keeps_the_first_timestamp():
    clock = FakeClock()
    runner = make_runner(FakeClient(), clock=clock)
    runner.note_disconnected()
    clock.advance(100)
    runner.note_disconnected()

    assert runner.disconnected_at == 1000.0


async def test_a_long_disconnect_gives_up():
    clock = FakeClock()
    runner = make_runner(FakeClient(), clock=clock)
    runner.note_disconnected()

    clock.advance(runner_module.DISCONNECT_LIMIT - 1)
    await runner.tick()
    assert runner.gave_up is None

    clock.advance(2)
    await runner.tick()
    assert "no gateway connection" in runner.gave_up


async def test_an_unconfirmed_presence_gives_up_after_the_deadline():
    clock = FakeClock()
    client = FakeClient(others=0)
    runner = make_runner(client, clock=clock, echo_seconds=60)
    await runner.tick()
    client.own = ONLINE

    clock.advance(59)
    await runner.tick()
    assert runner.gave_up is None

    clock.advance(2)
    await runner.tick()
    assert "never confirmed our invisible" in runner.gave_up


async def test_a_confirmed_presence_clears_the_expectation():
    client = FakeClient(others=0)
    runner = make_runner(client)
    await runner.tick()

    client.own = OFFLINE
    await runner.tick()

    assert runner.expect is None


async def test_an_unknown_own_session_is_not_treated_as_deaf():
    clock = FakeClock()
    client = FakeClient(on_mobile=True)
    runner = make_runner(client, clock=clock, echo_seconds=1)
    await runner.tick()

    clock.advance(10)
    await runner.tick()

    assert runner.gave_up is None
    assert runner.expect is None


async def test_a_rejected_write_stops_the_loop_and_reports_it(caplog):
    client = FakeClient(on_mobile=True)
    client.raises = Rejected()
    runner = make_runner(client)
    reasons = []
    runner.on_fatal = reasons.append

    await asyncio.wait_for(runner.run(), 1)

    assert reasons == ["Discord rejected the token, it needs replacing"]


async def test_other_write_failures_are_retried(monkeypatch):
    client = FakeClient(on_mobile=True)
    client.raises = ServerError()
    runner = make_runner(client)

    await run_until_cancelled(runner, 2, monkeypatch)

    assert runner.gave_up is None
    assert client.calls == [(IDLE, True), (IDLE, True)]


async def test_the_periodic_token_check_gives_up_on_a_rejected_token():
    clock = FakeClock()
    client = FakeClient()
    client.token_error = Rejected()
    runner = make_runner(client, clock=clock)

    clock.advance(runner_module.LIVENESS_SECONDS)
    await runner.liveness()

    assert client.token_checks == 1
    assert runner.gave_up is not None


async def test_a_failed_token_check_for_other_reasons_only_warns(caplog):
    clock = FakeClock()
    client = FakeClient()
    client.token_error = ServerError()
    runner = make_runner(client, clock=clock)
    caplog.set_level(logging.WARNING, logger="idlebot")

    clock.advance(runner_module.LIVENESS_SECONDS)
    await runner.liveness()

    assert runner.gave_up is None
    assert any("token check failed" in r.getMessage() for r in caplog.records)


async def test_the_token_check_is_skipped_while_disconnected_or_observing():
    clock = FakeClock()
    client = FakeClient()
    offline = make_runner(client, clock=clock)
    offline.note_disconnected()
    observing = make_runner(client, clock=clock, observe=True)

    clock.advance(runner_module.LIVENESS_SECONDS)
    await offline.liveness()
    await observing.liveness()

    assert client.token_checks == 0


async def test_a_hanging_write_times_out_and_is_retried():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client, write_timeout=0.01)
    hang = asyncio.Event()

    async def stuck(*, status, edit_settings=True):
        await hang.wait()

    client.change_presence = stuck
    with pytest.raises(asyncio.TimeoutError):
        await runner.tick()

    assert runner.pending == IDLE
    assert runner.presence is None


async def test_a_failed_stand_up_is_retried_and_does_not_strand_us_invisible():
    client = FakeClient()
    runner = make_runner(client)
    runner.note_settings(INVISIBLE, ONLINE)
    await runner.tick()

    runner.note_settings(ONLINE, INVISIBLE)
    original = client.unhide

    async def broken(status):
        raise ServerError()

    client.unhide = broken
    with pytest.raises(ServerError):
        await runner.tick()
    assert runner.stood_down is True

    client.unhide = original
    await runner.tick()
    assert runner.stood_down is False
    assert client.presence == [INVISIBLE, ONLINE]


async def test_coming_back_from_hidden_never_unhides_to_a_hidden_status():
    client = FakeClient(others=0, saved=INVISIBLE)
    runner = make_runner(client)
    await runner.tick()

    client.others = 1
    await runner.tick()

    assert client.presence == [INVISIBLE, ONLINE]


async def test_shutdown_restore_clears_the_marker():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker)
    await runner.tick()

    await runner.hand_back()

    assert marker.value is None
    assert runner.controller.holding is False


async def test_a_failed_shutdown_restore_keeps_the_marker():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker)
    await runner.tick()
    client.raises = ServerError()

    with pytest.raises(ServerError):
        await runner.hand_back()

    assert marker.value == ONLINE


async def test_shutdown_after_a_manual_override_still_clears_the_marker():
    client = FakeClient(on_mobile=True)
    marker = MemoryMarker()
    runner = make_runner(client, marker=marker, grace_seconds=1000)
    await runner.tick()
    client.set_status(DND)

    await runner.hand_back()

    assert client.calls == [(IDLE, True)]
    assert marker.value is None


async def test_serve_waits_out_the_start_delay_before_logging_in(monkeypatch):
    runner = make_runner(FakeClient())
    catch_stop(monkeypatch)
    gateway = FakeGateway()
    gateway.finished.set()

    assert await runner_module.serve(gateway, runner, "token", delay=0.01) == 0
    assert gateway.token == "token"


async def test_a_stop_during_the_start_delay_never_logs_in(monkeypatch):
    runner = make_runner(FakeClient())
    handlers = catch_stop(monkeypatch)
    gateway = FakeGateway()
    served = asyncio.create_task(runner_module.serve(gateway, runner, "token", delay=30))
    while not handlers:
        await asyncio.sleep(0)

    handlers[0]()

    assert await served == 0
    assert gateway.token is None


async def test_serve_abandons_without_restoring_when_the_runner_gives_up(monkeypatch):
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    catch_stop(monkeypatch)
    gateway = FakeGateway()
    served = asyncio.create_task(runner_module.serve(gateway, runner, "token"))
    await asyncio.sleep(0)

    runner._give_up("test")
    runner.on_fatal("test")

    assert await served == 1
    assert gateway.closes == 1
    assert client.calls == [(IDLE, True)]


def test_resume_hold_and_force_put_the_machine_back_in_a_hold():
    runner = make_runner(FakeClient(), grace_seconds=25)
    runner.controller.resume_hold(DND)
    debouncer = Debouncer(1, 4)
    debouncer.force(True)

    assert runner.controller.holding is True
    assert runner.controller.on_mobile is True
    assert runner.controller.saved == DND
    assert runner.controller.grace_until == 1025.0
    assert debouncer.stable is True
    assert debouncer.pending is True


async def test_the_first_reason_to_give_up_is_kept():
    runner = make_runner(FakeClient())

    runner._give_up("first")
    runner._give_up("second")

    assert runner.gave_up == "first"


async def test_a_loop_that_gives_up_with_no_handler_just_ends():
    client = FakeClient(on_mobile=True)
    client.raises = Rejected()
    runner = make_runner(client)

    await asyncio.wait_for(runner.run(), 1)

    assert runner.gave_up is not None
