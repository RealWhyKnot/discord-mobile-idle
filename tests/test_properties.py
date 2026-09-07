from helpers import DND, IDLE, INVISIBLE, OFFLINE, ONLINE
from hypothesis import given
from hypothesis import strategies as st

from idlebot.controller import StatusController

EXTERNAL = st.sampled_from([None, ONLINE, DND, INVISIBLE, OFFLINE])
STEPS = st.lists(st.tuples(st.booleans(), EXTERNAL), max_size=60)


@given(STEPS)
def test_controller_invariants(steps):
    controller = StatusController(ONLINE, {ONLINE}, grace_ticks=2)
    current = ONLINE

    for on_mobile, external in steps:
        if external is not None:
            current = external

        target = controller.evaluate(on_mobile, current)

        assert controller.saved != IDLE

        if target is None or target == current:
            continue

        assert current not in (INVISIBLE, OFFLINE)
        assert target not in (INVISIBLE, OFFLINE)
        current = target

    for _ in range(6):
        target = controller.evaluate(False, current)
        if target is not None and target != current:
            current = target

    assert not controller.holding
    assert current != IDLE
