import asyncio
import logging
import signal
import time

from .controller import INVISIBLE, OFFLINE

log = logging.getLogger("idlebot")

LIVENESS_SECONDS = 1800
WRITE_TIMEOUT = 45
ECHO_SECONDS = 90
DISCONNECT_LIMIT = 900
UNKNOWN = "unknown"
HIDDEN_STATUSES = frozenset({INVISIBLE, OFFLINE})

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


def check_afk(state):
    if getattr(state, "_afk", False) is True:
        log.info("session marked afk, so your phone still gets push notifications")
        return True
    log.warning("afk option not applied, your phone will get no push notifications while this runs")
    return False


def count_other_sessions(sessions):
    return sum(1 for s in sessions if not s.is_overall() and not s.is_current())


def token_rejected(exc):
    return getattr(exc, "status", None) == 401


class Runner:
    def __init__(
        self,
        client,
        controller,
        debouncer,
        poll_seconds,
        observe=False,
        clock=time.monotonic,
        marker=None,
        write_timeout=WRITE_TIMEOUT,
        echo_seconds=ECHO_SECONDS,
        stats=None,
    ):
        self.client = client
        self.controller = controller
        self.debouncer = debouncer
        self.poll_seconds = poll_seconds
        self.observe = observe
        self.clock = clock
        self.marker = marker
        self.write_timeout = write_timeout
        self.echo_seconds = echo_seconds
        self.stats = stats
        self.pending = None
        self.simulated = None
        self.hidden = False
        self.returning = False
        self.stood_down = False
        self.following = False
        self.user_status = None
        self.setting_echo = None
        self.presence = None
        self.expect = None
        self.reassert = False
        self.connected = True
        self.disconnected_at = None
        self.marked = None
        self.gave_up = None
        self.on_fatal = None
        self.transitions = 0
        self.wake = asyncio.Event()
        self.task = None
        self.started = clock()
        self.last_tick = self.started
        self.last_liveness = self.started

    def wake_if_mobile(self):
        if self.client.is_on_mobile() and not self.debouncer.stable:
            self.wake.set()

    def recover(self):
        if self.marker is None or self.observe:
            return
        saved = self.marker.read()
        if saved is None:
            return
        self.controller.resume_hold(saved)
        self.debouncer.force(True)
        self.marked = saved
        log.info("picking up a hold from before the restart, will restore %s once the phone goes", saved)

    def seed_user_status(self):
        status = self.client.saved_status
        if status == UNKNOWN:
            return
        if status == self.controller.idle_status and (self.controller.holding or self.client.is_on_mobile()):
            return
        self.user_status = status

    def note_settings(self, status, previous=None):
        if status == previous:
            return
        if status == self.setting_echo:
            self.setting_echo = None
            return
        if status == self.controller.idle_status and self.controller.holding:
            return
        self.user_status = status
        self.wake.set()

    def note_connected(self):
        self.connected = True
        self.disconnected_at = None
        self.expect = None
        self.reassert = self.presence is not None
        self.wake.set()

    def note_disconnected(self):
        if self.connected:
            self.connected = False
            self.disconnected_at = self.clock()

    def wants_hidden(self):
        return self.user_status in HIDDEN_STATUSES

    def wants_idle(self):
        return self.user_status == self.controller.idle_status

    async def wait_for_wake(self):
        try:
            await asyncio.wait_for(self.wake.wait(), self.poll_seconds)
        except asyncio.TimeoutError:
            return
        self.wake.clear()

    async def tick(self):
        try:
            await asyncio.wait_for(self._step(), self.write_timeout)
        finally:
            self._sync_marker()

    async def _step(self):
        if not self.connected:
            if self.clock() - self.disconnected_at > DISCONNECT_LIMIT:
                self._give_up("no gateway connection for %d seconds, starting over" % DISCONNECT_LIMIT)
            return
        if self.reassert:
            await self._show(self.presence)
            self.reassert = False
            log.info("reconnected, sent %s again", self.presence)
        if self._echo_missing():
            self._give_up("Discord never confirmed our %s presence, starting over" % self.expect[0])
            return
        if self.wants_hidden():
            await self._stand_down()
            return
        if self.stood_down:
            await self._stand_up()
        others = self.client.other_sessions()
        if self.hidden:
            if others:
                await self._unhide()
            return
        if self.returning:
            if self.client.status == INVISIBLE:
                return
            self.returning = False
        await self._manage()
        if not others and not self.controller.holding and not self.debouncer.stable:
            await self._hide()

    def _echo_missing(self):
        if self.expect is None:
            return False
        status, deadline = self.expect
        own = self.client.own_status
        if own is None or own == status or {own, status} <= HIDDEN_STATUSES:
            self.expect = None
            return False
        return self.clock() > deadline

    def _give_up(self, reason):
        if self.gave_up is None:
            self.gave_up = reason
            log.error("%s", reason)

    def _chosen(self):
        if self.user_status in (None, UNKNOWN):
            return self.controller.default_restore
        return self.user_status

    async def _write(self, status):
        self.setting_echo = status
        await self.client.change_presence(status=status, edit_settings=True)
        self._sent(status)

    async def _show(self, status):
        if status == INVISIBLE:
            await self.client.hide()
        else:
            await self.client.unhide(status)
        self._sent(status)

    def _sent(self, status):
        self.presence = status
        self.expect = (status, self.clock() + self.echo_seconds)

    async def _stand_down(self):
        if self.stood_down:
            return
        self.controller.stand_down()
        self.hidden = False
        self.returning = False
        self.following = False
        if not self.observe:
            await self._show(INVISIBLE)
        self.stood_down = True
        log.info("you set %s, leaving your status alone", self.user_status)

    async def _stand_up(self):
        self.debouncer.reset()
        target = self._chosen()
        if not self.observe:
            await self._show(target)
        self.stood_down = False
        self.returning = True
        self.following = self.wants_idle()
        log.info("you set %s, managing again", target)

    async def _hide(self):
        if not self.observe:
            await self._show(INVISIBLE)
        self.hidden = True
        log.info("nobody connected, going offline")

    async def _unhide(self):
        target = self.client.saved_status
        if target == UNKNOWN or target in HIDDEN_STATUSES:
            target = self.controller.default_restore
        if not self.observe:
            await self._show(target)
        self.hidden = False
        self.returning = True
        log.info("something connected, coming back as %s", target)

    async def _manage(self):
        raw = self.client.is_on_mobile()
        stable = self.debouncer.update(raw)
        if self.wants_idle():
            await self._follow_idle(stable)
            return
        if self.following:
            await self._stop_following()
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
        await self._write(self.pending)
        self.transitions += 1
        log.info("status %s -> %s (on_mobile=%s)", current, self.pending, stable)
        self.pending = None

    async def _follow_idle(self, stable):
        self.controller.step_aside(stable)
        self.pending = None
        if self.following:
            return
        if not self.observe:
            await self._show(self.controller.idle_status)
        self.following = True
        log.info("you set idle, leaving it alone until you pick another status")

    async def _stop_following(self):
        target = self._chosen()
        if not self.observe:
            await self._show(target)
        self.following = False
        log.info("you set %s, managing again", target)

    def _sync_marker(self):
        if self.marker is None or self.observe:
            return
        want = None
        if self.controller.holding:
            want = self.controller.saved or self.controller.default_restore
        if want == self.marked:
            return
        if want is None:
            self.marker.clear()
        else:
            self.marker.write(want)
        self.marked = want

    async def hand_back(self):
        if self.observe or not self.controller.holding:
            return
        target = self.controller.saved or self.controller.default_restore
        if self.client.status in (self.controller.idle_status, target):
            await self._write(target)
            log.info("restored %s on shutdown", target)
        self.controller.release()
        self._sync_marker()

    async def run(self):
        while self.gave_up is None:
            try:
                await self.tick()
            except Exception as exc:
                if token_rejected(exc):
                    self._give_up("Discord rejected the token, it needs replacing")
                else:
                    log.exception("tick failed")
            self.last_tick = self.clock()
            await self.liveness()
            if self.gave_up is None:
                await self.wait_for_wake()
        if self.on_fatal is not None:
            self.on_fatal(self.gave_up)

    async def liveness(self):
        now = self.clock()
        if now - self.last_liveness < LIVENESS_SECONDS:
            return
        self.last_liveness = now
        log.info(
            "alive uptime=%.0fs transitions=%d on_mobile=%s holding=%s hidden=%s returning=%s following=%s you=%s",
            now - self.started,
            self.transitions,
            self.debouncer.stable,
            self.controller.holding,
            self.hidden,
            self.returning,
            self.following,
            self.user_status,
        )
        if self.stats is not None:
            log.info("ignored gateway events: %s", self.stats())
        if self.observe or not self.connected:
            return
        try:
            await asyncio.wait_for(self.client.check_token(), self.write_timeout)
        except Exception as exc:
            if token_rejected(exc):
                self._give_up("Discord rejected the token, it needs replacing")
            else:
                log.warning("token check failed: %r", exc)


def install_stop_handlers(loop, on_stop):
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, on_stop)
        except NotImplementedError:
            signal.signal(sig, lambda *_: loop.call_soon_threadsafe(on_stop))


async def _cancel_loop(runner):
    task = runner.task
    if task is None:
        return
    runner.task = None
    task.cancel()
    try:
        await task
    except asyncio.CancelledError:
        pass


async def close_session(runner, close):
    await _cancel_loop(runner)
    try:
        await runner.hand_back()
    except Exception:
        log.exception("could not restore the status on the way out")
    await close()


async def abandon_session(runner, close):
    await _cancel_loop(runner)
    await close()


async def serve(client, runner, token, delay=0):
    stop = asyncio.Event()
    install_stop_handlers(asyncio.get_running_loop(), stop.set)
    runner.on_fatal = lambda reason: stop.set()
    if delay > 0:
        log.warning("restarted too often in the last hour, waiting %ds before logging in", delay)
        try:
            await asyncio.wait_for(stop.wait(), delay)
            return 0
        except asyncio.TimeoutError:
            pass
    session = asyncio.create_task(client.start(token))
    stopping = asyncio.create_task(stop.wait())
    try:
        await asyncio.wait((session, stopping), return_when=asyncio.FIRST_COMPLETED)
        if not session.done():
            if runner.gave_up is None:
                log.info("stop requested")
                await close_session(runner, client.close)
            else:
                await abandon_session(runner, client.close)
        await session
    finally:
        stopping.cancel()
    return 0 if runner.gave_up is None else 1


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
