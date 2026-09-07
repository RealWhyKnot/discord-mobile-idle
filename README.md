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

## Running it

```
docker build -t discord-mobile-idle .
docker run -d --name discord-mobile-idle --restart always \
    -e DISCORD_TOKEN=your_token_here \
    discord-mobile-idle
```

Only run one instance. Two logins on the same token fight over your status.

Without Docker: `pip install -r requirements.txt`, then `DISCORD_TOKEN=... python -m idlebot`.

## Configuration

| Variable | Default | Meaning |
| --- | --- | --- |
| `DISCORD_TOKEN` | required | Your user token, from the environment. |
| `POLL_SECONDS` | `10` | Seconds between checks. |
| `RESTORE_STATUS` | `online` | Fallback when there's no saved status. `online` or `dnd`. |
| `MANAGED_STATUSES` | `online` | Which statuses may be taken over. `online` and `dnd` only. |
| `ON_DEBOUNCE_POLLS` | `2` | Settled polls before reacting to the phone appearing. |
| `OFF_DEBOUNCE_POLLS` | `4` | Settled polls before reacting to it going away. |
| `WATCHDOG_SECONDS` | `120` | Force-exit if the poll loop stops ticking, so the container restarts. |
| `READY_FILE` | `/tmp/ready` | Written once after login. Useful as a healthcheck. |

## Debugging

```
python -m idlebot --sessions    # dump the session list and exit
python -m idlebot --observe     # log what it would do, write nothing
```

`--observe` never writes your status, so it's what I use to check detection against a live account.

## Tests

```
python -m venv .venv
.venv/bin/python -m pip install -r requirements-dev.txt    # .venv/Scripts on Windows
.venv/bin/python -m ruff check .
.venv/bin/python -m pytest
```

## Licence

MIT.
