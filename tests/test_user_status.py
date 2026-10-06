from helpers import DND, IDLE, INVISIBLE, OFFLINE, ONLINE, FakeClient, drive, make_runner


async def test_choosing_invisible_releases_the_phone_and_hides_us():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    assert client.calls == [(IDLE, True)]

    runner.note_settings(INVISIBLE)
    await runner.tick()

    assert client.calls == [(IDLE, True)]
    assert client.presence == [INVISIBLE]
    assert runner.controller.holding is False
    assert runner.stood_down is True


async def test_offline_counts_the_same_as_invisible():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)

    runner.note_settings(OFFLINE)
    await runner.tick()

    assert client.calls == []
    assert client.presence == [INVISIBLE]


async def test_standing_down_is_not_written_again():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    runner.note_settings(INVISIBLE)

    await drive(runner, 4)

    assert client.presence == [INVISIBLE]


async def test_a_phone_arriving_while_invisible_is_ignored():
    client = FakeClient(status=INVISIBLE)
    runner = make_runner(client)
    runner.note_settings(INVISIBLE)
    await runner.tick()

    client.set_mobile(True)
    await drive(runner, 3)

    assert client.calls == []


async def test_being_alone_while_invisible_does_not_arm_the_unhide():
    client = FakeClient(others=0)
    runner = make_runner(client)
    await runner.tick()
    assert runner.hidden is True

    runner.note_settings(INVISIBLE)
    await runner.tick()
    client.others = 1
    await runner.tick()

    assert client.presence == [INVISIBLE, INVISIBLE]
    assert runner.hidden is False


async def test_our_own_write_is_not_read_as_your_choice():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()
    assert client.calls == [(IDLE, True)]

    runner.note_settings(IDLE)
    await runner.tick()

    assert runner.user_status is None
    assert runner.stood_down is False
    assert runner.controller.holding is True


async def test_a_second_echo_of_the_same_status_is_your_choice():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()

    runner.note_settings(IDLE)
    runner.note_settings(IDLE)

    assert runner.user_status == IDLE


async def test_choosing_a_visible_status_again_resumes_management():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    runner.note_settings(INVISIBLE)
    await runner.tick()
    assert client.presence == [INVISIBLE]

    runner.note_settings(ONLINE)
    await runner.tick()

    assert client.presence == [INVISIBLE, ONLINE]
    assert runner.stood_down is False
    assert client.calls == [(IDLE, True)]


async def test_the_phone_is_taken_over_once_the_echo_lands():
    client = FakeClient(on_mobile=True, echo=False)
    runner = make_runner(client)
    runner.note_settings(INVISIBLE)
    await runner.tick()
    client.set_status(INVISIBLE)

    runner.note_settings(ONLINE)
    await runner.tick()
    assert client.presence == [INVISIBLE, ONLINE]
    assert client.calls == []

    await runner.tick()
    assert client.calls == []

    client.settle()
    await runner.tick()
    assert client.calls == [(IDLE, True)]


async def test_coming_back_uses_the_status_you_chose():
    client = FakeClient()
    runner = make_runner(client, restore=ONLINE, managed=(ONLINE, DND))
    runner.note_settings(INVISIBLE)
    await runner.tick()

    runner.note_settings(DND)
    await runner.tick()

    assert client.presence == [INVISIBLE, DND]


async def test_a_saved_status_is_seeded_at_login():
    client = FakeClient(status=ONLINE, on_mobile=True, saved=INVISIBLE)
    runner = make_runner(client)

    runner.seed_user_status()
    await runner.tick()

    assert runner.user_status == INVISIBLE
    assert client.calls == []
    assert client.presence == [INVISIBLE]


async def test_an_unset_saved_status_seeds_nothing():
    client = FakeClient(on_mobile=True, saved="unknown")
    runner = make_runner(client)

    runner.seed_user_status()
    await runner.tick()

    assert runner.user_status is None
    assert client.calls == [(IDLE, True)]


async def test_the_wake_fires_so_the_reaction_is_not_a_poll_away():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)

    runner.note_settings(INVISIBLE)

    assert runner.wake.is_set() is True


async def test_our_own_echo_does_not_wake_the_loop():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client)
    await runner.tick()

    runner.note_settings(IDLE)

    assert runner.wake.is_set() is False


async def test_observe_mode_writes_nothing_either_way():
    client = FakeClient(on_mobile=True)
    runner = make_runner(client, observe=True)

    runner.note_settings(INVISIBLE)
    await runner.tick()
    assert client.presence == []
    assert runner.stood_down is True

    runner.note_settings(ONLINE)
    await runner.tick()
    assert client.presence == []
    assert runner.stood_down is False
