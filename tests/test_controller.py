import pytest

from helpers import DND, IDLE, INVISIBLE, OFFLINE, ONLINE, make_controller


def test_no_edge_returns_none():
    controller = make_controller()
    assert controller.evaluate(False, ONLINE) is None


def test_no_edge_while_on_mobile():
    controller = make_controller()
    assert controller.evaluate(True, ONLINE) == IDLE
    assert controller.evaluate(True, IDLE) is None


def test_takes_control_from_online():
    controller = make_controller()
    assert controller.evaluate(True, ONLINE) == IDLE
    assert controller.holding
    assert controller.saved == ONLINE


def test_hands_back_saved_status():
    controller = make_controller()
    controller.evaluate(True, ONLINE)
    assert controller.evaluate(False, IDLE) == ONLINE
    assert not controller.holding
    assert controller.saved is None


def test_repeated_polls_do_not_overwrite_saved():
    controller = make_controller(managed=(ONLINE, DND), restore=ONLINE)
    controller.evaluate(True, DND)
    controller.evaluate(True, IDLE)
    controller.evaluate(True, IDLE)
    assert controller.saved == DND
    assert controller.evaluate(False, IDLE) == DND


@pytest.mark.parametrize("status", [INVISIBLE, OFFLINE])
def test_never_takes_control_of_hidden_status(status):
    controller = make_controller()
    assert controller.evaluate(True, status) is None
    assert not controller.holding
    assert controller.evaluate(False, status) is None


def test_dnd_untouched_by_default():
    controller = make_controller()
    assert controller.evaluate(True, DND) is None
    assert not controller.holding


def test_dnd_managed_when_configured():
    controller = make_controller(managed=(ONLINE, DND))
    assert controller.evaluate(True, DND) == IDLE
    assert controller.saved == DND
    assert controller.evaluate(False, IDLE) == DND


def test_adopts_existing_idle_after_restart():
    controller = make_controller()
    assert controller.evaluate(True, IDLE) == IDLE
    assert controller.holding
    assert controller.saved is None
    assert controller.evaluate(False, IDLE) == ONLINE


def test_hand_back_without_control_is_noop():
    controller = make_controller()
    controller.evaluate(True, INVISIBLE)
    assert controller.evaluate(False, INVISIBLE) is None


def test_manual_override_releases_control():
    controller = make_controller()
    controller.evaluate(True, ONLINE)
    assert controller.evaluate(True, ONLINE) is None
    assert not controller.holding
    assert controller.evaluate(False, ONLINE) is None


def test_grace_window_survives_propagation_lag():
    controller = make_controller(grace_ticks=2)
    assert controller.evaluate(True, ONLINE) == IDLE
    assert controller.evaluate(True, ONLINE) is None
    assert controller.holding
    assert controller.evaluate(True, ONLINE) is None
    assert controller.holding
    assert controller.evaluate(True, ONLINE) is None
    assert not controller.holding


def test_hand_back_skipped_when_status_changed_under_us():
    controller = make_controller(grace_ticks=5)
    controller.evaluate(True, ONLINE)
    assert controller.evaluate(False, INVISIBLE) is None
    assert not controller.holding


def test_restore_falls_back_to_default_when_nothing_saved():
    controller = make_controller(restore=ONLINE)
    controller.evaluate(True, IDLE)
    assert controller.saved is None
    assert controller.evaluate(False, IDLE) == ONLINE


def test_release_clears_state():
    controller = make_controller()
    controller.evaluate(True, ONLINE)
    controller.release()
    assert not controller.holding
    assert controller.saved is None
    assert controller.grace == 0
