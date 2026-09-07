IDLE = "idle"
ONLINE = "online"
DND = "dnd"
INVISIBLE = "invisible"
OFFLINE = "offline"

ALL_STATUSES = frozenset({ONLINE, IDLE, DND, INVISIBLE, OFFLINE})
ADOPTABLE = frozenset({ONLINE, DND})


class StatusController:
    def __init__(self, default_restore, managed, idle_status=IDLE, grace_ticks=2):
        self.idle_status = idle_status
        self.default_restore = default_restore
        self.managed = frozenset(managed)
        self.grace_ticks = grace_ticks
        self.on_mobile = False
        self.saved = None
        self.holding = False
        self.grace = 0

    def evaluate(self, is_on_mobile, current):
        if self.holding and not self._in_grace() and current != self.idle_status:
            self.release()
            return None
        if is_on_mobile == self.on_mobile:
            return None
        self.on_mobile = is_on_mobile
        if is_on_mobile:
            return self._take(current)
        return self._hand_back(current)

    def _in_grace(self):
        if self.grace <= 0:
            return False
        self.grace -= 1
        return True

    def _take(self, current):
        if current != self.idle_status and current not in self.managed:
            return None
        self.saved = None if current == self.idle_status else current
        self.holding = True
        self.grace = self.grace_ticks
        return self.idle_status

    def _hand_back(self, current):
        if not self.holding:
            return None
        target = self.saved or self.default_restore
        self.release()
        if current != self.idle_status:
            return None
        return target

    def release(self):
        self.holding = False
        self.saved = None
        self.grace = 0


class Debouncer:
    def __init__(self, on_polls, off_polls, initial=False):
        self.on_polls = on_polls
        self.off_polls = off_polls
        self.stable = initial
        self.pending = initial
        self.count = 0

    def update(self, reading):
        if reading == self.stable:
            self.pending = reading
            self.count = 0
            return self.stable
        if reading != self.pending:
            self.pending = reading
            self.count = 1
        else:
            self.count += 1
        if self.count >= (self.on_polls if reading else self.off_polls):
            self.stable = reading
            self.count = 0
        return self.stable

    def reset(self):
        self.pending = self.stable
        self.count = 0
