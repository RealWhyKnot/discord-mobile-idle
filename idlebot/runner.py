import asyncio
import logging
import time

log = logging.getLogger("idlebot")

LIVENESS_SECONDS = 1800

EXPECTED_CACHE = (
    ("max_messages", None),
    ("_chunk_guilds", False),
    ("_subscribe_guilds", False),
)


def check_cache_options(state):
    problems = []
    for name, want in EXPECTED_CACHE:
        got = getattr(state, name, "<missing>")
        if got != want:
            problems.append("%s=%r wanted %r" % (name, got, want))
    flags = getattr(state, "member_cache_flags", None)
    if getattr(flags, "value", -1) != 0:
        problems.append("member_cache_flags=%r wanted none" % (flags,))
    if problems:
        log.warning("cache options not applied, memory will be higher: %s", "; ".join(problems))
    else:
        log.info("cache options applied: no member, user, message or guild-subscription caching")
    return problems


class Runner:
    def __init__(self, client, controller, debouncer, poll_seconds, observe=False, clock=time.monotonic):
        self.client = client
        self.controller = controller
        self.debouncer = debouncer
        self.poll_seconds = poll_seconds
        self.observe = observe
        self.clock = clock
        self.pending = None
        self.simulated = None
        self.hidden = False
        self.transitions = 0
        self.wake = asyncio.Event()
        self.started = clock()
        self.last_tick = self.started
        self.last_liveness = self.started

    def wake_if_mobile(self):
        if self.client.is_on_mobile() and not self.debouncer.stable:
            self.wake.set()

    async def wait_for_wake(self):
        try:
            await asyncio.wait_for(self.wake.wait(), self.poll_seconds)
        except asyncio.TimeoutError:
            return
        self.wake.clear()

    async def tick(self):
        others = self.client.other_sessions()
        if self.hidden:
            if others:
                await self._unhide()
            return
        await self._manage()
        if not others and not self.controller.holding and not self.debouncer.stable:
            await self._hide()

    async def _hide(self):
        if not self.observe:
            await self.client.hide()
        self.hidden = True
        log.info("nobody connected, going offline")

    async def _unhide(self):
        target = self.client.saved_status
        if target == "unknown":
            target = self.controller.default_restore
        if not self.observe:
            await self.client.unhide(target)
        self.hidden = False
        self.wake.set()
        log.info("something connected, coming back as %s", target)

    async def _manage(self):
        raw = self.client.is_on_mobile()
        stable = self.debouncer.update(raw)
        current = self.client.status if self.simulated is None else self.simulated
        if self.observe:
            log.info("observe: raw_mobile=%s stable=%s status=%s", raw, stable, current)
        if self.pending is None:
            self.pending = self.controller.evaluate(stable, current)
        if self.pending is None:
            return
        if self.pending == current:
            self.pending = None
            return
        if self.observe:
            log.info("observe: would set %s -> %s (on_mobile=%s)", current, self.pending, stable)
            self.simulated = self.pending
            self.pending = None
            return
        await self.client.change_presence(status=self.pending, edit_settings=True)
        self.transitions += 1
        log.info("status %s -> %s (on_mobile=%s)", current, self.pending, stable)
        self.pending = None

    async def hand_back(self):
        if self.observe or not self.controller.holding:
            return
        if self.client.status != self.controller.idle_status:
            return
        target = self.controller.saved or self.controller.default_restore
        await self.client.change_presence(status=target, edit_settings=True)
        log.info("restored %s on shutdown", target)

    async def run(self):
        while True:
            try:
                await self.tick()
            except Exception:
                log.exception("tick failed")
            self.last_tick = self.clock()
            self.liveness()
            await self.wait_for_wake()

    def liveness(self):
        now = self.clock()
        if now - self.last_liveness < LIVENESS_SECONDS:
            return
        self.last_liveness = now
        log.info(
            "alive uptime=%.0fs transitions=%d on_mobile=%s holding=%s",
            now - self.started,
            self.transitions,
            self.debouncer.stable,
            self.controller.holding,
        )


class Watchdog:
    def __init__(self, get_last_tick, timeout, on_stall, clock=time.monotonic):
        self.get_last_tick = get_last_tick
        self.timeout = timeout
        self.on_stall = on_stall
        self.clock = clock
        self.fired = False

    def check(self):
        if self.fired:
            return False
        stalled = self.clock() - self.get_last_tick()
        if stalled <= self.timeout:
            return False
        self.fired = True
        log.error("watchdog: no tick for %.0fs, exiting", stalled)
        self.on_stall()
        return True
