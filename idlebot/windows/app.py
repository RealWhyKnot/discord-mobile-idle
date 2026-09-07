import logging
import os
import sys
import threading
import tkinter
import tkinter.messagebox
import tkinter.simpledialog
import traceback

import pystray
from PIL import Image, ImageDraw

from ..config import ConfigError, load_config
from . import store
from .service import Service

log = logging.getLogger("idlebot")

COLOURS = {
    "connecting": (114, 118, 125, 255),
    "active": (59, 165, 92, 255),
    "holding": (242, 159, 30, 255),
    "error": (237, 66, 69, 255),
}


def icon_image(state):
    image = Image.new("RGBA", (64, 64), (0, 0, 0, 0))
    ImageDraw.Draw(image).ellipse((4, 4, 59, 59), fill=COLOURS[state])
    return image


def _dialog(work):
    result = {}

    def show():
        root = tkinter.Tk()
        root.withdraw()
        root.attributes("-topmost", True)
        try:
            result["value"] = work(root)
        finally:
            root.destroy()

    thread = threading.Thread(target=show, name="dialog")
    thread.start()
    thread.join()
    return result.get("value")


def ask_token(current=""):
    value = _dialog(
        lambda root: tkinter.simpledialog.askstring(
            store.APP, "Paste your Discord token", show="*", initialvalue=current, parent=root
        )
    )
    return (value or "").strip()


def message(text):
    _dialog(lambda root: tkinter.messagebox.showinfo(store.APP, text, parent=root))


class App:
    def __init__(self, config):
        self.state = "connecting"
        self.detail = "starting"
        self.service = Service(config, self.set_state)
        self.icon = pystray.Icon(store.APP, icon_image(self.state), store.APP, menu=pystray.Menu(*self.menu()))

    def menu(self):
        return (
            pystray.MenuItem(lambda item: self.detail, lambda icon, item: None, enabled=False),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Open log", self.open_log, default=True),
            pystray.MenuItem("Edit settings", self.open_settings),
            pystray.MenuItem("Set token", self.set_token),
            pystray.MenuItem(
                "Start with Windows",
                self.toggle_autostart,
                checked=lambda item: store.autostart_enabled(),
                enabled=store.frozen(),
            ),
            pystray.MenuItem("Reconnect", self.reconnect),
            pystray.Menu.SEPARATOR,
            pystray.MenuItem("Quit", self.quit),
        )

    def set_state(self, state, detail):
        self.state = state
        self.detail = detail
        log.info("tray %s: %s", state, detail)
        self.icon.icon = icon_image(state)
        self.icon.title = "%s - %s" % (store.APP, detail)

    def start(self, icon):
        icon.visible = True
        self.service.start()

    def open_log(self, icon, item):
        os.startfile(store.path_for("idlebot.log"))

    def open_settings(self, icon, item):
        os.startfile(store.settings_file())

    def set_token(self, icon, item):
        threading.Thread(target=self._set_token, daemon=True, name="set-token").start()

    def _set_token(self):
        token = ask_token(store.read_token())
        if not token:
            return
        store.write_token(token)
        self.service.config.token = token
        self.service.restart()

    def toggle_autostart(self, icon, item):
        store.set_autostart(not store.autostart_enabled())

    def reconnect(self, icon, item):
        self.service.restart()

    def quit(self, icon, item):
        self.service.stop()
        icon.stop()


def self_test():
    try:
        import discord

        if not hasattr(discord, "Client"):
            raise RuntimeError("discord.py-self did not import cleanly")
        if not store.check_crypto():
            raise RuntimeError("DPAPI round trip failed")
        menu = pystray.Menu(pystray.MenuItem("Quit", lambda icon, item: None))
        pystray.Icon(store.APP, icon_image("active"), store.APP, menu=menu)
        root = tkinter.Tk()
        root.withdraw()
        root.destroy()
    except Exception:
        with open(store.path_for("self-test.log"), "w", encoding="utf-8") as handle:
            traceback.print_exc(file=handle)
        return 3
    return 0


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if "--self-test" in argv:
        return self_test()

    store.start_logging()
    lock = store.SingleInstance()
    if not lock.acquire():
        message("%s is already running." % store.APP)
        return 0

    token = store.read_token()
    if not token:
        token = ask_token()
        if not token:
            return 0
        store.write_token(token)

    try:
        config = load_config({**store.read_settings(), "DISCORD_TOKEN": token})
    except ConfigError as exc:
        message("settings.json: %s" % exc)
        return 2

    log.info("starting poll=%ds managed=%s restore=%s", config.poll_seconds, sorted(config.managed), config.restore)
    app = App(config)
    app.icon.run(setup=app.start)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
