import discord

from idlebot.discord_client import DiscordClient, start_watchdog


class FakeSession:
    def __init__(self, name, overall=False, current=False):
        self.client = name
        self._overall = overall
        self._current = current

    def is_overall(self):
        return self._overall

    def is_current(self):
        return self._current


class FakeSettings:
    def __init__(self, status):
        self.status = discord.Status(status)


class FakeGatewayClient:
    def __init__(self, status="online", saved="online", on_mobile=False, sessions=()):
        self.status = discord.Status(status)
        self.settings = FakeSettings(saved)
        self.sessions = list(sessions)
        self.presence = []
        self._on_mobile = on_mobile

    def is_on_mobile(self):
        return self._on_mobile

    async def change_presence(self, *, status, edit_settings=True):
        self.presence.append((str(status), edit_settings))


def test_the_rollup_and_our_own_session_are_not_counted():
    client = FakeGatewayClient(
        sessions=[
            FakeSession("all", overall=True),
            FakeSession("web", current=True),
            FakeSession("desktop"),
            FakeSession("mobile"),
        ]
    )
    assert DiscordClient(client).other_sessions() == 2


def test_being_the_only_login_counts_as_nothing_connected():
    client = FakeGatewayClient(
        sessions=[
            FakeSession("all", overall=True),
            FakeSession("web", current=True),
        ]
    )
    assert DiscordClient(client).other_sessions() == 0


def test_the_rollup_and_the_saved_status_are_read_separately():
    wrapper = DiscordClient(FakeGatewayClient(status="idle", saved="dnd"))
    assert wrapper.status == "idle"
    assert wrapper.saved_status == "dnd"


def test_mobile_detection_is_delegated():
    assert DiscordClient(FakeGatewayClient(on_mobile=True)).is_on_mobile() is True
    assert DiscordClient(FakeGatewayClient(on_mobile=False)).is_on_mobile() is False


async def test_going_offline_never_touches_the_saved_status():
    client = FakeGatewayClient()
    await DiscordClient(client).hide()
    assert client.presence == [("invisible", False)]


async def test_coming_back_never_touches_the_saved_status():
    client = FakeGatewayClient()
    await DiscordClient(client).unhide("dnd")
    assert client.presence == [("dnd", False)]


async def test_taking_over_the_status_saves_it():
    client = FakeGatewayClient()
    await DiscordClient(client).change_presence(status="idle")
    assert client.presence == [("idle", True)]


class CountingWatchdog:
    def __init__(self, fire_on):
        self.fire_on = fire_on
        self.checks = 0

    def check(self):
        self.checks += 1
        return self.checks >= self.fire_on


def test_the_watchdog_thread_polls_until_it_fires():
    watchdog = CountingWatchdog(fire_on=3)

    thread = start_watchdog(watchdog, 0)
    thread.join(timeout=5)

    assert not thread.is_alive()
    assert watchdog.checks == 3
    assert thread.daemon
