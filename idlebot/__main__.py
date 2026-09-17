import argparse
import asyncio
import logging
import os
import sys

import discord

from .config import ConfigError, load_config
from .controller import Debouncer, StatusController
from .discord_client import DiscordClient, start_watchdog
from .runner import Runner, Watchdog, check_cache_options, serve

log = logging.getLogger("idlebot")

READY_FILE = os.environ.get("READY_FILE", "/tmp/ready")


def write_ready():
    try:
        with open(READY_FILE, "w") as handle:
            handle.write("ok")
    except OSError:
        log.exception("could not write %s", READY_FILE)


def log_sessions(client):
    try:
        for session in client.sessions:
            if session.is_overall():
                label = "rollup"
            elif session.is_current():
                label = "this bot"
            else:
                label = str(session.client)
            log.info("session %s status=%s", label, session.status)
    except Exception:
        log.exception("could not read sessions")


def parse_args(argv=None):
    parser = argparse.ArgumentParser(prog="idlebot")
    parser.add_argument("--observe", action="store_true", help="log intended changes, write nothing")
    parser.add_argument("--sessions", action="store_true", help="dump sessions once and exit")
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(argv)
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(message)s",
        stream=sys.stdout,
    )

    try:
        config = load_config()
    except ConfigError as exc:
        log.error("configuration error: %s", exc)
        return 2

    client = discord.Client(
        chunk_guilds_at_startup=False,
        max_messages=None,
        guild_subscriptions=False,
        member_cache_flags=discord.MemberCacheFlags.none(),
        sync_presence=False,
    )
    check_cache_options(client._connection)
    controller = StatusController(config.restore, config.managed)
    debouncer = Debouncer(config.on_polls, config.off_polls)
    runner = Runner(
        DiscordClient(client),
        controller,
        debouncer,
        config.poll_seconds,
        observe=args.observe or args.sessions,
    )

    @client.event
    async def on_ready():
        log.info("connected as %s", client.user)
        log_sessions(client)
        write_ready()
        debouncer.reset()
        runner.last_tick = runner.clock()
        if args.sessions:
            await client.close()
            return
        if runner.task is not None:
            return
        runner.task = asyncio.create_task(runner.run())
        start_watchdog(
            Watchdog(lambda: runner.last_tick, config.watchdog_seconds, lambda: os._exit(1)),
            max(1.0, config.watchdog_seconds / 3.0),
        )

    @client.event
    async def on_resumed():
        log.info("gateway resumed")
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

    log.info(
        "starting observe=%s poll=%ds managed=%s restore=%s",
        args.observe,
        config.poll_seconds,
        sorted(config.managed),
        config.restore,
    )
    try:
        asyncio.run(serve(client, runner, config.token))
    except discord.LoginFailure:
        log.error("login failed: DISCORD_TOKEN was rejected")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
