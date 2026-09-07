import ctypes
import json
import logging
import logging.handlers
import msvcrt
import os
import sys
import winreg
from ctypes import wintypes

APP = "discord-mobile-idle"
RUN_KEY = r"Software\Microsoft\Windows\CurrentVersion\Run"
CRYPTPROTECT_UI_FORBIDDEN = 0x01

DEFAULT_SETTINGS = {
    "POLL_SECONDS": "10",
    "RESTORE_STATUS": "online",
    "MANAGED_STATUSES": "online",
    "ON_DEBOUNCE_POLLS": "2",
    "OFF_DEBOUNCE_POLLS": "4",
    "WATCHDOG_SECONDS": "120",
    "UPDATE_CHANNEL": "release",
    "CHECK_FOR_UPDATES": "true",
    "SKIPPED_TAG": "",
}

log = logging.getLogger("idlebot")


def data_dir():
    base = os.environ.get("LOCALAPPDATA") or os.path.expanduser("~")
    path = os.path.join(base, APP)
    os.makedirs(path, exist_ok=True)
    return path


def path_for(name):
    return os.path.join(data_dir(), name)


def read_settings():
    try:
        with open(path_for("settings.json"), encoding="utf-8") as handle:
            data = json.load(handle)
    except (OSError, ValueError):
        return {}
    if not isinstance(data, dict):
        return {}
    return {str(key): str(value) for key, value in data.items()}


def settings_file():
    path = path_for("settings.json")
    if not os.path.exists(path):
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(DEFAULT_SETTINGS, handle, indent=2)
    return path


_crypt32 = ctypes.WinDLL("crypt32", use_last_error=True)
_kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)


class _Blob(ctypes.Structure):
    _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]


_CRYPT_ARGS = [
    ctypes.POINTER(_Blob),
    ctypes.c_void_p,
    ctypes.POINTER(_Blob),
    ctypes.c_void_p,
    ctypes.c_void_p,
    wintypes.DWORD,
    ctypes.POINTER(_Blob),
]

_crypt32.CryptProtectData.argtypes = _CRYPT_ARGS
_crypt32.CryptProtectData.restype = wintypes.BOOL
_crypt32.CryptUnprotectData.argtypes = _CRYPT_ARGS
_crypt32.CryptUnprotectData.restype = wintypes.BOOL
_kernel32.LocalFree.argtypes = [ctypes.c_void_p]
_kernel32.LocalFree.restype = ctypes.c_void_p


def _crypt(func, data):
    source = ctypes.create_string_buffer(data, len(data))
    incoming = _Blob(len(data), ctypes.cast(source, ctypes.POINTER(ctypes.c_char)))
    outgoing = _Blob()
    ok = func(ctypes.byref(incoming), None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, ctypes.byref(outgoing))
    if not ok:
        raise ctypes.WinError(ctypes.get_last_error())
    try:
        return ctypes.string_at(outgoing.pbData, outgoing.cbData)
    finally:
        _kernel32.LocalFree(ctypes.cast(outgoing.pbData, ctypes.c_void_p))


def check_crypto():
    return _crypt(_crypt32.CryptUnprotectData, _crypt(_crypt32.CryptProtectData, b"ok")) == b"ok"


def read_token():
    try:
        with open(path_for("token.bin"), "rb") as handle:
            blob = handle.read()
    except OSError:
        return ""
    try:
        return _crypt(_crypt32.CryptUnprotectData, blob).decode("utf-8")
    except (OSError, UnicodeDecodeError):
        log.exception("stored token could not be decrypted")
        return ""


def write_token(token):
    blob = _crypt(_crypt32.CryptProtectData, token.encode("utf-8"))
    with open(path_for("token.bin"), "wb") as handle:
        handle.write(blob)


class SingleInstance:
    def __init__(self):
        self._handle = None

    def acquire(self):
        handle = open(path_for("instance.lock"), "a+")
        try:
            handle.seek(0)
            msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
        except OSError:
            handle.close()
            return False
        self._handle = handle
        return True


def start_logging():
    handler = logging.handlers.RotatingFileHandler(
        path_for("idlebot.log"), maxBytes=524288, backupCount=3, encoding="utf-8"
    )
    handler.setFormatter(logging.Formatter("%(asctime)s %(levelname)s %(message)s"))
    logging.basicConfig(level=logging.INFO, handlers=[handler])
    logging.getLogger("discord").setLevel(logging.WARNING)


def frozen():
    return getattr(sys, "frozen", False)


def autostart_command():
    return '"%s"' % os.path.abspath(sys.executable)


def autostart_enabled():
    try:
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY) as key:
            return winreg.QueryValueEx(key, APP)[0] == autostart_command()
    except OSError:
        return False


def set_autostart(enabled):
    with winreg.OpenKey(winreg.HKEY_CURRENT_USER, RUN_KEY, 0, winreg.KEY_SET_VALUE) as key:
        if enabled:
            winreg.SetValueEx(key, APP, 0, winreg.REG_SZ, autostart_command())
            return
        try:
            winreg.DeleteValue(key, APP)
        except OSError:
            pass
