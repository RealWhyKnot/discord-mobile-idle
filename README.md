# discord-mobile-idle

Shows you as idle on Discord while you're on your phone, and puts your previous status back when you
put it down.

## Before you use this

This drives a **user account** with a user token, not a bot account. Discord's Terms of Service
prohibit automating user accounts, and doing so can get your account terminated. There is no official
API for setting your own status, so there's no version of this that avoids that. Your call.

The token is equivalent to your password. Anyone who reads it owns the account until you change your
password, which invalidates it.

## How it decides

Every `POLL_SECONDS` it reads whether a mobile session is active and what your account-wide status
is, runs both through a debouncer, and writes a status only on a settled change.

Rules it won't break:

- Only statuses in `MANAGED_STATUSES` are ever taken over. Invisible and offline are never written,
  so it can't unhide you.
- It hands a status back only if it's still showing idle. If something else moved your status
  meanwhile, it releases and leaves you alone.
- If your status changes while it's holding idle, it treats that as you overriding it and stands
  down until the next phone transition.
- On startup it adopts an idle it finds already set, so a restart mid-phone-session can't strand you
  on idle.

Coming back from idle lags by a few minutes. That's Discord: a mobile session stays alive for a
while after you close the app, and nothing here can shorten it.

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

Run **one instance only**. Two logins on the same token will fight over your status.

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

Set `MANAGED_STATUSES=online,dnd` if you want it to take over Do Not Disturb as well. By default it
leaves DND alone entirely.

## Debugging

```
python -m idlebot --sessions    # dump the session list and exit
python -m idlebot --observe     # log what it would do, write nothing
```

`--observe` is the safe way to check detection against a live account: it simulates its own writes so
the state machine still advances, but never touches your status. Use it first after changing
anything about how sessions are read.

It logs status transitions and one liveness line every 30 minutes. Nothing per poll.

## Tests

```
python -m venv .venv
.venv/bin/python -m pip install -r requirements-dev.txt    # .venv/Scripts on Windows
.venv/bin/python -m pytest
```

Coverage is gated at 100% branch on `controller.py`, `runner.py` and `config.py`. The suite uses a
fake clock and a fake client, so it never sleeps and never touches the network. Hypothesis covers the
invariants that matter: it can't get stuck idle, and it can't write a status while you're invisible.

## Licence

MIT.
