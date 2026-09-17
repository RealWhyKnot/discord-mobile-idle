from helpers import DND, IDLE, INVISIBLE, ONLINE, FakeClient, drive, make_runner


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
