import logging
import os

from idlebot.persist import HoldMarker, StartLog, start_delay, write_atomic


def test_the_first_starts_in_an_hour_are_free():
    delay, recent = start_delay([100.0, 200.0], 300.0)

    assert delay == 0
    assert recent == [100.0, 200.0]


def test_the_delay_doubles_past_the_free_starts():
    history = [1000.0 + i for i in range(5)]

    delay, recent = start_delay(history, 2000.0)

    assert delay == 120
    assert len(recent) == 5


def test_the_delay_is_capped():
    history = [1000.0 + i for i in range(40)]

    delay, _ = start_delay(history, 2000.0)

    assert delay == 1800


def test_old_and_future_starts_are_dropped():
    delay, recent = start_delay([0.0, 5000.0, 9000.0], 4000.0)

    assert delay == 0
    assert recent == []


def test_the_start_log_round_trips(tmp_path):
    log = StartLog(str(tmp_path / "starts"))

    log.save([10.0, 20.0])

    assert log.load() == [10.0, 20.0]


def test_a_clean_exit_takes_its_start_back_out(tmp_path):
    log = StartLog(str(tmp_path / "starts"))
    log.save([10.0, 20.0, 30.0])

    log.forget(20.4)

    assert log.load() == [10.0, 30.0]


def test_a_missing_or_garbled_start_log_reads_empty(tmp_path):
    path = tmp_path / "starts"
    assert StartLog(str(path)).load() == []

    path.write_text("12\nnot a number\n")

    assert StartLog(str(path)).load() == []


def test_the_hold_marker_round_trips_and_clears(tmp_path):
    marker = HoldMarker(str(tmp_path / "hold"))

    marker.write("dnd")
    assert marker.read() == "dnd"

    marker.clear()
    marker.clear()
    assert marker.read() is None


def test_a_hold_marker_with_an_unexpected_value_is_ignored(tmp_path):
    path = tmp_path / "hold"
    path.write_text("idle")
    assert HoldMarker(str(path)).read() is None

    path.write_bytes(b"\xff\xfe")
    assert HoldMarker(str(path)).read() is None


def test_a_marker_that_cannot_be_removed_only_warns(tmp_path, caplog):
    caplog.set_level(logging.WARNING, logger="idlebot")
    directory = tmp_path / "hold"
    directory.mkdir()

    HoldMarker(str(directory)).clear()

    assert any("could not remove" in r.getMessage() for r in caplog.records)


def test_an_unwritable_path_only_warns(tmp_path, caplog):
    caplog.set_level(logging.WARNING, logger="idlebot")

    write_atomic(str(tmp_path / "missing" / "file"), "x")

    assert any("could not write" in r.getMessage() for r in caplog.records)
    assert not os.path.exists(tmp_path / "missing")
