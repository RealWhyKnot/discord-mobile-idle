from helpers import FakeClock

from idlebot.runner import Watchdog


def build(timeout=100):
    clock = FakeClock()
    last = {"t": clock.now}
    fired = []
    watchdog = Watchdog(lambda: last["t"], timeout, lambda: fired.append(1), clock=clock)
    return watchdog, clock, last, fired


def test_quiet_while_ticking():
    watchdog, clock, last, fired = build()
    for _ in range(5):
        clock.advance(50)
        last["t"] = clock.now
        assert watchdog.check() is False
    assert fired == []


def test_boundary_is_not_a_stall():
    watchdog, clock, _last, fired = build(timeout=100)
    clock.advance(100)
    assert watchdog.check() is False
    assert fired == []


def test_fires_once_when_stalled():
    watchdog, clock, _last, fired = build(timeout=100)
    clock.advance(101)
    assert watchdog.check() is True
    assert fired == [1]


def test_does_not_fire_twice():
    watchdog, clock, _last, fired = build(timeout=100)
    clock.advance(500)
    assert watchdog.check() is True
    assert watchdog.check() is False
    assert fired == [1]
