from idlebot.controller import Debouncer


def test_requires_consecutive_readings():
    debouncer = Debouncer(on_polls=2, off_polls=3)
    assert debouncer.update(True) is False
    assert debouncer.update(True) is True


def test_flapping_never_flips():
    debouncer = Debouncer(on_polls=2, off_polls=2)
    for reading in [True, False, True, False, True, False]:
        assert debouncer.update(reading) is False


def test_off_threshold_is_independent():
    debouncer = Debouncer(on_polls=1, off_polls=3)
    assert debouncer.update(True) is True
    assert debouncer.update(False) is True
    assert debouncer.update(False) is True
    assert debouncer.update(False) is False


def test_reset_clears_pending_without_changing_stable():
    debouncer = Debouncer(on_polls=3, off_polls=3)
    debouncer.update(True)
    debouncer.update(True)
    debouncer.reset()
    assert debouncer.count == 0
    assert debouncer.stable is False
    assert debouncer.update(True) is False
    assert debouncer.update(True) is False
    assert debouncer.update(True) is True


def test_initial_state_respected():
    debouncer = Debouncer(1, 1, initial=True)
    assert debouncer.update(True) is True
    assert debouncer.update(False) is False


def test_steady_reading_resets_counter():
    debouncer = Debouncer(on_polls=2, off_polls=2)
    debouncer.update(True)
    debouncer.update(False)
    assert debouncer.count == 0
    assert debouncer.pending is False
