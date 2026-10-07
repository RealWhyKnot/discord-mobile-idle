import threading
import time

import discord

from .runner import count_other_sessions


class DiscordClient:
    def __init__(self, client):
        self._client = client

    def is_on_mobile(self):
        return self._client.is_on_mobile()

    def other_sessions(self):
        return count_other_sessions(self._client.sessions)

    @property
    def status(self):
        return str(self._client.status)

    @property
    def saved_status(self):
        settings = self._client.settings
        return "unknown" if settings is None else str(settings.status)

    @property
    def own_status(self):
        session = self._client._connection.current_session
        return None if session is None else str(session.status)

    async def check_token(self):
        await self._client.http.get_me(with_analytics_token=False)

    async def change_presence(self, *, status, edit_settings=True):
        await self._client.change_presence(status=discord.Status(status), edit_settings=edit_settings)

    async def hide(self):
        await self._client.change_presence(status=discord.Status.invisible, edit_settings=False)

    async def unhide(self, status):
        await self._client.change_presence(status=discord.Status(status), edit_settings=False)


def start_watchdog(watchdog, interval):
    def loop():
        while True:
            time.sleep(interval)
            if watchdog.check():
                return

    thread = threading.Thread(target=loop, daemon=True, name="watchdog")
    thread.start()
    return thread
