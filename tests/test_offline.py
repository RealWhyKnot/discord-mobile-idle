from helpers import DND, IDLE, INVISIBLE, ONLINE, FakeClient, drive, make_runner

from idlebot.runner import count_other_sessions


async def test_goes_offline_when_nothing_else_is_connected():
    client = FakeClient(others=0)
    runner = make_runner(client)

    await runner.tick()

    assert client.presence == [INVISIBLE]
    assert client.calls == []
    assert runner.hidden is True


async def test_staying_alone_is_not_written_again():
    client = FakeClient(others=0)
    runner = make_runner(client)

    await drive(runner, 4)

    assert client.presence == [INVISIBLE]


async def test_comes_back_when_something_connects():
    client = FakeClient(others=0)
    runner = make_runner(client)
    await runner.tick()

    client.others = 1
    await runner.tick()

    assert client.presence == [INVISIBLE, ONLINE]
    assert client.calls == []
    assert runner.hidden is False


async def test_comes_back_as_the_saved_status():
    client = FakeClient(others=0, saved=DND)
    runner = make_runner(client, restore=ONLINE)
    await runner.tick()

    client.others = 1
    await runner.tick()

    assert client.presence == [INVISIBLE, DND]


async def test_an_unset_saved_status_falls_back_to_the_configured_restore():
    client = FakeClient(others=0, saved="unknown")
    runner = make_runner(client, restore=DND)
    await runner.tick()

    client.others = 1
    await runner.tick()

    assert client.presence == [INVISIBLE, DND]


async def test_the_phone_is_handed_back_before_going_offline():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    assert client.calls == [(IDLE, True)]

    client.set_mobile(False)
    client.others = 0
    await runner.tick()

    assert client.calls == [(IDLE, True), (ONLINE, True)]
    assert client.presence == [INVISIBLE]


async def test_never_goes_offline_while_holding_the_status():
    client = FakeClient(on_mobile=True, others=0)
    runner = make_runner(client)

    await runner.tick()

    assert client.calls == [(IDLE, True)]
    assert client.presence == []
    assert runner.hidden is False


async def test_never_goes_offline_while_the_phone_still_reads_connected():
    client = FakeClient(status=DND, on_mobile=True, others=0)
    runner = make_runner(client, managed=(ONLINE,))

    await runner.tick()

    assert client.calls == []
    assert client.presence == []
    assert runner.hidden is False


async def test_the_phone_is_still_taken_over_after_coming_back():
    client = FakeClient(others=0)
    runner = make_runner(client)
    await runner.tick()
    assert client.presence == [INVISIBLE]

    client.echo = False
    client.set_mobile(True)
    client.others = 1
    await runner.tick()
    assert client.presence == [INVISIBLE, ONLINE]
    assert client.calls == []

    await runner.tick()
    assert client.calls == []
    assert runner.controller.on_mobile is False

    client.settle()
    await runner.tick()
    assert client.calls == [(IDLE, True)]


async def test_a_status_chosen_while_away_is_not_overwritten_on_return():
    client = FakeClient(others=0, echo=False)
    runner = make_runner(client, managed=(ONLINE,))
    await runner.tick()

    client.others = 1
    await runner.tick()
    client._status = DND

    await runner.tick()
    assert runner.returning is False
    assert client.calls == []


async def test_observe_mode_writes_nothing_either_way():
    client = FakeClient(others=0)
    runner = make_runner(client, observe=True)

    await runner.tick()
    assert client.presence == []
    assert runner.hidden is True

    client.others = 1
    await runner.tick()
    assert client.presence == []
    assert runner.hidden is False


class FakeSession:
    def __init__(self, overall=False, current=False):
        self._overall = overall
        self._current = current

    def is_overall(self):
        return self._overall

    def is_current(self):
        return self._current


def test_the_rollup_and_our_own_login_are_not_counted():
    sessions = [
        FakeSession(overall=True),
        FakeSession(current=True),
        FakeSession(),
        FakeSession(),
    ]
    assert count_other_sessions(sessions) == 2


def test_being_the_only_login_counts_as_nothing_connected():
    assert count_other_sessions([FakeSession(overall=True), FakeSession(current=True)]) == 0


def test_an_empty_session_list_counts_as_nothing_connected():
    assert count_other_sessions([]) == 0
