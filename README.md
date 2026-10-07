# discord-mobile-idle

[![CI](https://img.shields.io/github/actions/workflow/status/RealWhyKnot/discord-mobile-idle/ci.yml?branch=main&label=CI)](https://github.com/RealWhyKnot/discord-mobile-idle/actions/workflows/ci.yml)

Shows you as idle on Discord while you're on your phone.

It also goes offline for you when no other client is connected, and it leaves your status alone
entirely while you've set yourself invisible or offline, on any device. Pick a visible status again
and it picks up where it left off.

## Before you use this

This logs in as **you**, with your user token. Discord's Terms of Service prohibit automating user
accounts. Use it at your own risk.

Anyone who has your token can log in as you. Don't share it.

## Getting a token

Open Discord in a browser, open devtools, and look at the Network tab. Pick any request to `/api`
and copy the `Authorization` request header. That value is the token.

## Windows

Grab the zip from [Releases](https://github.com/RealWhyKnot/discord-mobile-idle/releases), unpack it
anywhere, and run `discord-mobile-idle.exe`. It asks for your token once, then runs in the
notification area without a window.

The exe isn't code-signed and SmartScreen may stop the first launch. Click More info, then Run
anyway.

Right-click the tray icon for the menu:

| Item | What it does |
| --- | --- |
| Open log | Opens `idlebot.log`. Double-clicking the icon does the same. |
| Edit settings | Opens `settings.json` in your editor. Restart afterwards. |
| Set token | Replaces the stored token and reconnects. |
| Start with Windows | Adds or removes the logon entry. Only offered for the packaged exe. |
| Reconnect | Drops the gateway connection and builds a new one. |
| Check for updates | Looks for a newer release. Once one is downloaded and checked, this becomes Install and restart. |
| Quit | Restores your status first, then exits. |

## Docker

Docker builds it straight from this repo, no clone needed:

```
docker build -t discord-mobile-idle https://github.com/RealWhyKnot/discord-mobile-idle.git
docker run -d --name discord-mobile-idle --restart always \
    -e DISCORD_TOKEN=your_token_here \
    discord-mobile-idle
```

Run one copy only, either this or the tray app. If both are logged in with the same token they keep
overwriting each other's status changes.

`docker stop` restores your status before the container exits, the same as Quit does in the tray
app, and so does Ctrl+C when you run it directly. `docker kill` skips it and leaves you on idle.

Without Docker you need Python 3.10 or newer. Clone the repo, `pip install -r requirements.txt`,
then `DISCORD_TOKEN=... python -m idlebot`.

## Configuration

The container reads these from the environment. The Windows app reads the same names from
`settings.json`, except `DISCORD_TOKEN` and `READY_FILE`, which are container-only.

| Variable | Default | Meaning |
| --- | --- | --- |
| `DISCORD_TOKEN` | required | Your user token, from the environment. |
| `POLL_SECONDS` | `10` | Fallback seconds between checks. Session events from the gateway wake a check sooner. |
| `RESTORE_STATUS` | `online` | Fallback when there's no saved status. `online` or `dnd`. |
| `MANAGED_STATUSES` | `online` | Which statuses may be taken over. `online` and `dnd` only. |
| `ON_DEBOUNCE_POLLS` | `1` | Settled polls before reacting to the phone appearing. At `1` it reacts within about a second of the phone connecting. |
| `OFF_DEBOUNCE_POLLS` | `4` | Settled polls before reacting to it going away. |
| `WATCHDOG_SECONDS` | `120` | Give up on a poll loop that stops ticking. The container exits and restarts, the tray app reconnects. |
| `CHECK_FOR_UPDATES` | `true` | Windows only. Whether it looks for new releases on its own. |
| `UPDATE_CHANNEL` | `release` | Windows only. `release` or `beta`. |
| `SKIPPED_TAG` | empty | Windows only. A release tag to stop offering. |
| `READY_FILE` | `/tmp/ready` | Written once after login. Useful as a healthcheck. |

## Debugging

```
python -m idlebot --sessions    # dump the session list and exit
python -m idlebot --observe     # log what it would do, write nothing
```

`--observe` never writes your status. I use it to check detection against a live account.

## Building the exe yourself

```
pip install -r requirements-win.txt pyinstaller
pyinstaller --onefile --clean --noconfirm --noconsole ^
    --name discord-mobile-idle --distpath dist/exe --paths . --icon packaging/icon.ico ^
    --collect-all curl_cffi --collect-all discord_protos ^
    --collect-submodules discord --collect-submodules pystray ^
    --hidden-import audioop packaging/entry.py
```

`discord-mobile-idle.exe --self-test` imports discord.py-self, round-trips the token encryption and
builds the tray icon. It exits 0 when all of that works, or 3 with the traceback in `self-test.log`.
The release workflow runs it before publishing.

## Tests

```
python -m venv .venv
.venv/bin/python -m pip install -r requirements-dev.txt    # .venv/Scripts on Windows
.venv/bin/python -m ruff check .
.venv/bin/python -m pytest
```

## Before pushing

```
.\verify.ps1
```

That lints, runs the tests, checks the container entry point starts from the image bundle alone,
builds the image if Docker is on PATH, and builds the executable and runs its self test. A
`pre-push` hook in `.githooks` runs it for you once `git config core.hooksPath .githooks` is set.
Machine-specific checks go in a `verify.local.ps1` beside it, which runs too. `git push --no-verify`
skips the lot.

## Licence

MIT.
