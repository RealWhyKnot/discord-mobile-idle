import asyncio
import logging
import signal

import pytest
from helpers import DND, IDLE, ONLINE, FakeClient, make_runner

from idlebot import runner as runner_module


class FakeLoop:
    def __init__(self, supported=True):
        self.supported = supported
        self.added = []
        self.threadsafe = []

    def add_signal_handler(self, sig, callback):
        if not self.supported:
            raise NotImplementedError
        self.added.append((sig, callback))

    def call_soon_threadsafe(self, callback):
        self.threadsafe.append(callback)
        callback()


class FakeGateway:
    def __init__(self, error=None):
        self.error = error
        self.finished = asyncio.Event()
        self.token = None
        self.closes = 0

    async def start(self, token):
        self.token = token
        if self.error is not None:
            raise self.error
        await self.finished.wait()

    async def close(self):
        self.closes += 1
        self.finished.set()


def catch_stop(monkeypatch):
    handlers = []
    monkeypatch.setattr(runner_module, "install_stop_handlers", lambda loop, on_stop: handlers.append(on_stop))
    return handlers


def test_stop_handlers_use_the_loop_where_it_supports_them():
    loop = FakeLoop()
    fired = []

    runner_module.install_stop_handlers(loop, lambda: fired.append(1))

    assert [sig for sig, _ in loop.added] == [signal.SIGINT, signal.SIGTERM]
    for _sig, callback in loop.added:
        callback()
    assert fired == [1, 1]


def test_stop_handlers_fall_back_to_signal_signal(monkeypatch):
    loop = FakeLoop(supported=False)
    installed = {}
    monkeypatch.setattr(signal, "signal", lambda sig, handler: installed.__setitem__(sig, handler))
    fired = []

    runner_module.install_stop_handlers(loop, lambda: fired.append(1))

    assert set(installed) == {signal.SIGINT, signal.SIGTERM}
    assert loop.added == []
    installed[signal.SIGTERM](signal.SIGTERM, None)
    assert fired == [1]
    assert len(loop.threadsafe) == 1


async def test_close_session_cancels_the_run_loop():
    runner = make_runner(FakeClient())
    running = asyncio.Event()
    closes = []

    async def forever():
        running.set()
        await asyncio.sleep(3600)

    async def close():
        closes.append(1)

    runner.task = asyncio.create_task(forever())
    await running.wait()

    await runner_module.close_session(runner, close)

    assert runner.task is None
    assert closes == [1]


async def test_close_session_tolerates_a_run_loop_that_already_ended():
    runner = make_runner(FakeClient())
    closes = []

    async def nothing():
        return None

    async def close():
        closes.append(1)

    runner.task = asyncio.create_task(nothing())
    await runner.task

    await runner_module.close_session(runner, close)

    assert runner.task is None
    assert closes == [1]


async def test_close_session_closes_even_when_the_restore_fails(caplog):
    client = FakeClient()
    runner = make_runner(client)
    client.set_mobile(True)
    await runner.tick()
    assert client.status == IDLE
    client.raises = RuntimeError("gateway gone")
    closes = []

    async def close():
        closes.append(1)

    with caplog.at_level(logging.ERROR):
        await runner_module.close_session(runner, close)

    assert closes == [1]
    assert "could not restore the status on the way out" in caplog.text


async def test_serve_restores_the_saved_status_on_a_stop_signal(monkeypatch):
    client = FakeClient(status=DND)
    runner = make_runner(client, managed=(ONLINE, DND))
    client.set_mobile(True)
    await runner.tick()
    assert client.status == IDLE

    handlers = catch_stop(monkeypatch)
    gateway = FakeGateway()
    served = asyncio.create_task(runner_module.serve(gateway, runner, "token"))
    while not handlers:
        await asyncio.sleep(0)

    handlers[0]()
    await served

    assert client.status == DND
    assert gateway.token == "token"
    assert gateway.closes == 1


async def test_serve_leaves_a_session_that_ended_by_itself_alone(monkeypatch):
    client = FakeClient()
    runner = make_runner(client)
    catch_stop(monkeypatch)
    gateway = FakeGateway()
    gateway.finished.set()

    await runner_module.serve(gateway, runner, "token")

    assert gateway.closes == 0
    assert client.calls == []


async def test_serve_propagates_a_failed_login(monkeypatch):
    runner = make_runner(FakeClient())
    catch_stop(monkeypatch)
    gateway = FakeGateway(error=RuntimeError("token rejected"))

    with pytest.raises(RuntimeError):
        await runner_module.serve(gateway, runner, "token")

    assert gateway.closes == 0
