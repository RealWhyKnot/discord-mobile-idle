import threading
import time

import discord


class DiscordClient:
    def __init__(self, client):
        self._client = client

    def is_on_mobile(self):
        return self._client.is_on_mobile()

    @property
    def status(self):
        return str(self._client.status)

    async def change_presence(self, *, status, edit_settings=True):
        await self._client.change_presence(status=discord.Status(status), edit_settings=edit_settings)


def start_watchdog(watchdog, interval):
    def loop():
        while True:
            time.sleep(interval)
            if watchdog.check():
                return

    thread = threading.Thread(target=loop, daemon=True, name="watchdog")
    thread.start()
    return thread
