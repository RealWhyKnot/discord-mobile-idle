# discord-mobile-idle

[![CI](https://img.shields.io/github/actions/workflow/status/RealWhyKnot/discord-mobile-idle/ci.yml?branch=main&label=CI)](https://github.com/RealWhyKnot/discord-mobile-idle/actions/workflows/ci.yml)

Shows you as idle on Discord while you're on your phone.

Discord puts a phone icon on your avatar when you're on mobile, but it won't change your status, so
I wrote this.

## Before you use this

This drives a **user account** with a user token, not a bot account. Discord's Terms of Service
prohibit automating user accounts, and doing so can get your account terminated. I run it on my own
account, but use this at your own risk.

Your token is equivalent to your password, keep it safe.

## Getting a token

Open Discord in a browser, open devtools, and look at the Network tab. Pick any request to `/api`
and copy the `Authorization` request header. That value is the token.

## Windows

Grab the zip from [Releases](https://github.com/RealWhyKnot/discord-mobile-idle/releases), unpack it
anywhere, and run `discord-mobile-idle.exe`. It asks for your token once, then lives in the
notification area with no window. There is nothing to install and no Python needed.

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

The icon is grey while connecting, green once connected, amber while it is holding you on idle, and
red if your token was rejected.

Everything it keeps lives in `%LOCALAPPDATA%\discord-mobile-idle`:

| File | What it is |
| --- | --- |
| `token.bin` | Your token, encrypted so only your Windows account can read it. |
| `settings.json` | The settings below, same names, written on first use. |
| `idlebot.log` | Rotating log, three files of 512KB. |
| `instance.lock` | Held while it runs. A second copy shows a message and exits. |
| `update.log` | What the updater did the last time it replaced the exe. |

The lock only sees other copies on the same machine. If you also run the container, stop one of
them: two logins on the same token fight over your status.

If the poll loop ever stalls it reconnects instead of exiting, since there is no supervisor to
restart it. That covers a hung connection, not a wedged event loop, so if the tray is stuck on grey
for minutes the fix is Quit and start it again.

It checks for a new release when it starts and once a day after that, and downloads the zip in the
background so installing is one click. It replaces its own exe in place, so it needs write access to
its own folder. If you put it somewhere you cannot write, like `Program Files`, the swap is undone
and it carries on running the version you already had. Set `CHECK_FOR_UPDATES` to `false` if you
would rather it never phoned home, `UPDATE_CHANNEL` to `beta` to get the nightly builds, or
`SKIPPED_TAG` to a tag you want it to stop offering.

## Docker

```
docker build -t discord-mobile-idle .
docker run -d --name discord-mobile-idle --restart always \
    -e DISCORD_TOKEN=your_token_here \
    discord-mobile-idle
```

Only run one instance, here or in the tray app. Two logins on the same token fight over your
status.

Without Docker: `pip install -r requirements.txt`, then `DISCORD_TOKEN=... python -m idlebot`.

## Configuration

The container reads these from the environment. The Windows app reads the same names from
`settings.json`, except `DISCORD_TOKEN` and `READY_FILE`, which are container-only.

| Variable | Default | Meaning |
| --- | --- | --- |
| `DISCORD_TOKEN` | required | Your user token, from the environment. |
| `POLL_SECONDS` | `10` | Seconds between checks. |
| `RESTORE_STATUS` | `online` | Fallback when there's no saved status. `online` or `dnd`. |
| `MANAGED_STATUSES` | `online` | Which statuses may be taken over. `online` and `dnd` only. |
| `ON_DEBOUNCE_POLLS` | `2` | Settled polls before reacting to the phone appearing. |
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

`--observe` never writes your status, so it's what I use to check detection against a live account.

## Building the exe yourself

```
pip install -r requirements-win.txt pyinstaller
pyinstaller --onefile --clean --noconfirm --noconsole ^
    --name discord-mobile-idle --distpath dist/exe --paths . --icon packaging/icon.ico ^
    --collect-all curl_cffi --collect-all discord_protos ^
    --collect-submodules discord --collect-submodules pystray ^
    --hidden-import audioop packaging/entry.py
```

`discord-mobile-idle.exe --self-test` checks that the frozen build can reach everything it needs and
exits 0. That is what the release workflow runs before publishing.

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
Drop a `verify.local.ps1` beside it and that runs too, which is where a machine-specific check
belongs. `git push --no-verify` skips the lot.

## Licence

MIT.
