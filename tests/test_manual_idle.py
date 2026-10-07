import logging

from helpers import DND, IDLE, INVISIBLE, ONLINE, FakeClient, drive, make_runner

from idlebot.runner import check_afk


async def test_choosing_idle_pulls_our_session_to_idle_without_touching_settings():
    client = FakeClient()
    runner = make_runner(client)
    await runner.tick()

    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    assert client.calls == []
    assert client.presence == [IDLE]
    assert runner.following is True


async def test_a_phone_cycle_leaves_your_idle_alone():
    client = FakeClient(status=IDLE)
    runner = make_runner(client)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    client.set_mobile(True)
    await drive(runner, 3)
    client.set_mobile(False)
    await drive(runner, 3)

    assert client.calls == []
    assert client.presence == [IDLE]
    assert runner.controller.holding is False


async def test_overriding_the_phone_then_choosing_idle_is_never_restored():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    runner.note_settings(IDLE, ONLINE)

    runner.note_settings(DND, IDLE)
    client.set_status(DND)
    await runner.tick()
    runner.note_settings(IDLE, DND)
    client.set_status(IDLE)
    await runner.tick()
    client.set_mobile(False)
    await drive(runner, 3)

    assert client.calls == [(IDLE, True)]
    assert client.presence == [IDLE]
    assert runner.controller.holding is False


async def test_picking_online_again_brings_our_session_back_and_waits_for_the_next_pickup():
    client = FakeClient(status=IDLE, on_mobile=True)
    runner = make_runner(client)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    runner.note_settings(ONLINE, IDLE)
    client.set_status(ONLINE)
    await drive(runner, 2)
    assert client.presence == [IDLE, ONLINE]
    assert client.calls == []
    assert runner.following is False

    client.set_mobile(False)
    await drive(runner, 2)
    client.set_mobile(True)
    await runner.tick()
    assert client.calls == [(IDLE, True)]


async def test_an_unset_status_after_idle_falls_back_to_the_restore_status():
    client = FakeClient(status=IDLE)
    runner = make_runner(client, restore=ONLINE)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    runner.note_settings("unknown", IDLE)
    await runner.tick()

    assert client.presence == [IDLE, ONLINE]


async def test_alone_while_idle_still_goes_offline_and_comes_back_idle():
    client = FakeClient(status=IDLE, others=1, saved=IDLE)
    runner = make_runner(client)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    client.others = 0
    await runner.tick()
    client.others = 1
    await drive(runner, 2)

    assert client.presence == [IDLE, INVISIBLE, IDLE]
    assert runner.following is True
    assert client.calls == []


async def test_invisible_then_idle_comes_back_idle_once():
    client = FakeClient()
    runner = make_runner(client)
    runner.note_settings(INVISIBLE, ONLINE)
    await runner.tick()

    runner.note_settings(IDLE, INVISIBLE)
    await drive(runner, 3)

    assert client.presence == [INVISIBLE, IDLE]
    assert runner.following is True


async def test_idle_then_invisible_stops_following():
    client = FakeClient()
    runner = make_runner(client)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()

    runner.note_settings(INVISIBLE, IDLE)
    await runner.tick()

    assert runner.following is False
    assert client.presence == [IDLE, INVISIBLE]


async def test_an_unchanged_status_in_a_settings_update_is_ignored():
    client = FakeClient(status=IDLE, saved=IDLE)
    runner = make_runner(client)

    runner.note_settings(IDLE, IDLE)

    assert runner.user_status is None
    assert runner.wake.is_set() is False


async def test_a_saved_idle_at_login_is_not_trusted_as_yours():
    client = FakeClient(status=IDLE, on_mobile=True, saved=IDLE)
    runner = make_runner(client)

    runner.seed_user_status()
    await runner.tick()
    client.set_mobile(False)
    await runner.tick()

    assert runner.user_status is None
    assert client.calls == [(ONLINE, True)]


async def test_a_reconnect_keeps_the_idle_you_chose():
    client = FakeClient(status=IDLE, saved=IDLE)
    runner = make_runner(client)
    runner.note_settings(IDLE, ONLINE)

    runner.seed_user_status()

    assert runner.user_status == IDLE


async def test_echoes_landing_before_the_write_returns_are_still_ours():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    client.during_write = lambda status: runner.note_settings(status, IDLE if status == ONLINE else ONLINE)

    await runner.tick()
    client.set_mobile(False)
    await runner.tick()

    assert client.calls == [(IDLE, True), (ONLINE, True)]
    assert runner.user_status is None
    assert runner.setting_echo is None


async def test_an_echo_landing_during_the_shutdown_restore_is_still_ours():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    client.during_write = lambda status: runner.note_settings(status, IDLE)

    await runner.hand_back()

    assert runner.user_status is None
    assert runner.setting_echo is None


async def test_observe_mode_follows_idle_without_writing():
    client = FakeClient()
    runner = make_runner(client, observe=True)
    runner.note_settings(IDLE, ONLINE)
    await runner.tick()
    assert runner.following is True

    runner.note_settings(ONLINE, IDLE)
    await runner.tick()

    assert client.presence == []
    assert runner.following is False


class State:
    def __init__(self, afk):
        self._afk = afk


def test_afk_applied_is_reported(caplog):
    caplog.set_level(logging.INFO, logger="idlebot")
    assert check_afk(State(True)) is True
    assert any("push notifications" in r.getMessage() for r in caplog.records)


def test_afk_missing_warns(caplog):
    caplog.set_level(logging.INFO, logger="idlebot")
    assert check_afk(object()) is False
    assert any(r.levelno == logging.WARNING for r in caplog.records)
