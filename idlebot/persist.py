import logging
import os

from .controller import ADOPTABLE

log = logging.getLogger("idlebot")

START_FREE = 3
START_WINDOW = 3600
START_BASE = 30
START_CAP = 1800


def start_delay(history, now, free=START_FREE, window=START_WINDOW, base=START_BASE, cap=START_CAP):
    recent = sorted(t for t in history if 0 <= now - t < window)
    extra = len(recent) - free
    if extra < 0:
        return 0, recent
    return min(cap, base * 2**extra), recent


class StartLog:
    def __init__(self, path):
        self.path = path

    def load(self):
        try:
            with open(self.path, encoding="ascii") as handle:
                return [float(line) for line in handle if line.strip()]
        except (OSError, ValueError):
            return []

    def save(self, history):
        write_atomic(self.path, "".join("%.0f\n" % t for t in history))

    def forget(self, stamp):
        self.save([t for t in self.load() if abs(t - stamp) >= 1])


class HoldMarker:
    def __init__(self, path):
        self.path = path

    def read(self):
        try:
            with open(self.path, encoding="ascii") as handle:
                status = handle.read().strip()
        except (OSError, ValueError):
            return None
        return status if status in ADOPTABLE else None

    def write(self, status):
        write_atomic(self.path, status)

    def clear(self):
        try:
            os.remove(self.path)
        except FileNotFoundError:
            pass
        except OSError as exc:
            log.warning("could not remove %s: %s", self.path, exc)


def write_atomic(path, text):
    temp = path + ".tmp"
    try:
        with open(temp, "w", encoding="ascii") as handle:
            handle.write(text)
        os.replace(temp, path)
    except OSError as exc:
        log.warning("could not write %s: %s", path, exc)
