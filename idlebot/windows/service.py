import asyncio
import logging
import threading

import discord

from ..controller import IDLE, Debouncer, StatusController
from ..discord_client import DiscordClient, start_watchdog
from ..runner import Runner, Watchdog, check_cache_options, close_session

log = logging.getLogger("idlebot")

BACKOFF_START = 5
BACKOFF_MAX = 300


class NotifyingClient(DiscordClient):
    def __init__(self, client, on_write, on_hidden):
        super().__init__(client)
        self._on_write = on_write
        self._on_hidden = on_hidden

    async def change_presence(self, *, status, edit_settings=True):
        await super().change_presence(status=status, edit_settings=edit_settings)
        self._on_write(status)

    async def hide(self):
        await super().hide()
        self._on_hidden(None)

    async def unhide(self, status):
        await super().unhide(status)
        self._on_hidden(status)


class Service:
    def __init__(self, config, on_state):
        self.config = config
        self.on_state = on_state
        self.loop = None
        self.client = None
        self.runner = None
        self.stopping = False
        self._retry = asyncio.Event()

    def start(self):
        self.loop = asyncio.new_event_loop()
        threading.Thread(target=self._serve, daemon=True, name="discord").start()

    def stop(self):
        if self.loop is None:
            return
        try:
            asyncio.run_coroutine_threadsafe(self._stop(), self.loop).result(timeout=10)
        except Exception:
            log.exception("shutdown did not finish cleanly")

    def restart(self):
        if self.loop is None:
            return
        asyncio.run_coroutine_threadsafe(self._restart(), self.loop)

    def _serve(self):
        asyncio.set_event_loop(self.loop)
        try:
            self.loop.run_until_complete(self._supervise())
        finally:
            self.loop.close()

    async def _supervise(self):
        delay = BACKOFF_START
        while not self.stopping:
            self.on_state("connecting", "connecting")
            try:
                await self._session()
                delay = BACKOFF_START
            except discord.LoginFailure:
                self.on_state("error", "token rejected, set a new one")
                await self._retry.wait()
                self._retry.clear()
                delay = BACKOFF_START
                continue
            except Exception:
                log.exception("session ended")
            if self.stopping:
                return
            self.on_state("connecting", "reconnecting in %ds" % delay)
            try:
                await asyncio.wait_for(self._retry.wait(), delay)
            except asyncio.TimeoutError:
                delay = min(delay * 2, BACKOFF_MAX)
            else:
                self._retry.clear()
                delay = BACKOFF_START

    async def _session(self):
        client = discord.Client(
            chunk_guilds_at_startup=False,
            max_messages=None,
            guild_subscriptions=False,
            member_cache_flags=discord.MemberCacheFlags.none(),
            sync_presence=False,
        )
        check_cache_options(client._connection)
        debouncer = Debouncer(self.config.on_polls, self.config.off_polls)
        runner = Runner(
            NotifyingClient(client, self._wrote, self._hidden),
            StatusController(self.config.restore, self.config.managed),
            debouncer,
            self.config.poll_seconds,
        )
        self.client = client
        self.runner = runner

        @client.event
        async def on_ready():
            self.on_state("active", "connected as %s" % client.user)
            debouncer.reset()
            runner.last_tick = runner.clock()
            if runner.task is not None:
                return
            runner.task = asyncio.create_task(runner.run())
            start_watchdog(
                Watchdog(lambda: runner.last_tick, self.config.watchdog_seconds, lambda: self._stall(client)),
                max(1.0, self.config.watchdog_seconds / 3.0),
            )

        @client.event
        async def on_resumed():
            debouncer.reset()
            runner.last_tick = runner.clock()
            runner.wake_if_mobile()

        @client.event
        async def on_session_create(session):
            runner.wake_if_mobile()

        @client.event
        async def on_session_update(before, after):
            runner.wake_if_mobile()

        @client.event
        async def on_session_delete(session):
            runner.wake_if_mobile()

        try:
            await client.start(self.config.token)
        finally:
            self.client = None
            self.runner = None

    def _hidden(self, status):
        if status is None:
            self.on_state("active", "offline while nothing else is connected")
        else:
            self.on_state("active", "back to %s" % status)

    def _wrote(self, status):
        if status == IDLE:
            self.on_state("holding", "idle while your phone is awake")
        else:
            self.on_state("active", "back to %s" % status)

    def _stall(self, client):
        if client is not self.client or self.loop is None:
            return
        log.error("watchdog fired, rebuilding the session")
        self.on_state("connecting", "recovering")
        asyncio.run_coroutine_threadsafe(self._close_session(), self.loop)

    async def _restart(self):
        self._retry.set()
        await self._close_session()

    async def _stop(self):
        self.stopping = True
        self._retry.set()
        await self._close_session()

    async def _close_session(self):
        client = self.client
        if client is None:
            return
        await close_session(self.runner, client.close)
