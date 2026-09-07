import hashlib
import json
import logging
import os
import shutil
import subprocess
import time
import urllib.request
import zipfile

from ..release import DEV, asset, parse_integrity, parse_version, select_release
from . import store

try:
    from ._version import VERSION
except ImportError:
    VERSION = "0.0.0.0-dev"

log = logging.getLogger("idlebot")

API = "https://api.github.com/repos/RealWhyKnot/discord-mobile-idle/releases?per_page=20"
EXE = "discord-mobile-idle.exe"
TIMEOUT = 15
CHUNK = 1048576

_parsed = parse_version(VERSION)
CHECKABLE = _parsed is not None and _parsed[1] != DEV

SCRIPT = """$ErrorActionPreference = 'Stop'
$log = %(log)s
$target = %(target)s
$source = %(source)s
$lock = %(lock)s
$backup = "${target}.bak"
function Note($text) { "$(Get-Date -Format s) $text" | Add-Content -LiteralPath $log -Encoding UTF8 }
Wait-Process -Id %(pid)d -Timeout 60 -ErrorAction SilentlyContinue
$free = $false
foreach ($attempt in 1..30) {
    try {
        foreach ($path in @($target, $lock)) { [System.IO.File]::Open($path, 'Open', 'Write', 'None').Dispose() }
        $free = $true
        break
    } catch {
        Start-Sleep -Seconds 2
    }
}
if (-not $free) {
    Note "gave up waiting for $target"
    exit 1
}
$moved = $false
try {
    Move-Item -LiteralPath $target -Destination $backup -Force
    $moved = $true
} catch {
    Note "could not set aside $target - $_"
}
if ($moved) {
    try {
        Copy-Item -LiteralPath $source -Destination $target -Force
        Remove-Item -LiteralPath $backup -Force -ErrorAction SilentlyContinue
        Note "installed $source over $target"
    } catch {
        Note "install failed, rolled back: $_"
        Move-Item -LiteralPath $backup -Destination $target -Force
    }
}
Start-Process -FilePath $target
"""


def _quote(text):
    return "'%s'" % str(text).replace("'", "''")


def _staging():
    path = store.path_for("update")
    os.makedirs(path, exist_ok=True)
    return path


def _fetch(url, binary=False):
    request = urllib.request.Request(url, headers={"User-Agent": "discord-mobile-idle/%s" % VERSION})
    with urllib.request.urlopen(request, timeout=TIMEOUT) as response:
        payload = response.read()
    return payload if binary else payload.decode("utf-8-sig")


def check(channel, skipped):
    releases = json.loads(_fetch(API))
    return select_release(releases, channel, VERSION, skipped)


def stage(chosen):
    zip_asset = asset(chosen, ".zip")
    tsv_asset = asset(chosen, ".integrity.tsv")
    if zip_asset is None or tsv_asset is None:
        raise RuntimeError("release %s is missing an asset" % chosen.get("tag_name"))

    expected = parse_integrity(_fetch(tsv_asset[1]), zip_asset[0])
    if expected is None:
        raise RuntimeError("no integrity row for %s" % zip_asset[0])
    digest, size = expected

    staging = _staging()
    archive = os.path.join(staging, zip_asset[0])
    try:
        payload = _fetch(zip_asset[1], binary=True)
    except OSError:
        time.sleep(5)
        payload = _fetch(zip_asset[1], binary=True)
    with open(archive, "wb") as handle:
        handle.write(payload)

    actual = os.path.getsize(archive)
    if actual != size:
        raise RuntimeError("%s is %d bytes, expected %d" % (zip_asset[0], actual, size))
    digester = hashlib.sha256()
    with open(archive, "rb") as handle:
        for block in iter(lambda: handle.read(CHUNK), b""):
            digester.update(block)
    if digester.hexdigest() != digest:
        raise RuntimeError("%s failed its sha256 check" % zip_asset[0])

    with zipfile.ZipFile(archive) as bundle:
        bundle.extract(EXE, staging)
    return os.path.join(staging, EXE)


def render(target, source, pid):
    return SCRIPT % {
        "log": _quote(store.path_for("update.log")),
        "target": _quote(target),
        "source": _quote(source),
        "lock": _quote(store.path_for("instance.lock")),
        "pid": pid,
    }


def launch(staged, target, pid):
    script = os.path.join(_staging(), "update.ps1")
    with open(script, "w", encoding="utf-8") as handle:
        handle.write(render(target, staged, pid))
    subprocess.Popen(
        ["powershell.exe", "-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File", script],
        creationflags=subprocess.DETACHED_PROCESS,
        close_fds=True,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    log.info("update helper launched for %s", target)


def clean():
    shutil.rmtree(store.path_for("update"), ignore_errors=True)
