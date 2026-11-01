# Encore

[![ci](https://github.com/takaramono-nl/tk-encore/actions/workflows/ci.yml/badge.svg)](https://github.com/takaramono-nl/tk-encore/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Resend a past listmonk campaign to subscribers who joined after it was sent.

Encore is a standalone Rust binary with its own SQLite ledger. It does not fork
listmonk or change its schema. Every resend goes through a private "pending
list" that acts as the durable marker of "still owed this mail".

## Requirements

- listmonk **v6.2+** (built and tested against v6.2.0; v7 should work but is
  untested)
- An API user with these permissions:
  - `campaigns:get`, `campaigns:manage`, `campaigns:send`
  - `lists:manage`
  - `subscribers:manage`
  - `subscribers:sql_query` (required for the cohort query expression)

## Build

Requires Rust 1.94+ (the `sqlx` 0.9 MSRV).

```sh
cargo build --release
# target/release/encore
```

## Quick start

```sh
export ENCORE_LISTMONK_URL="https://listmonk.example.com"
export ENCORE_LISTMONK_USER="encore"
export ENCORE_LISTMONK_TOKEN="..."

encore init
encore run --campaign 42 --dry-run   # show the cohort, write nothing
encore run --campaign 42 --yes       # compute, send, and close
```

`init` writes `~/.config/encore/config.toml` (mode 0600) and creates
`~/.local/share/encore/encore.db`. Everything can be overridden with flags or
environment variables.

## Monitoring new subscribers

listmonk does not emit a webhook when a subscriber registers or confirms a
double opt-in subscription, so Encore polls. Run it from cron every 15–30
minutes:

```sh
*/15 * * * * cd /srv/encore && . /etc/encore/env && \
  encore run --campaign 42 --min-new 5 --yes
```

`run` recomputes the exact cohort on every tick. If fewer than `--min-new`
subscribers are waiting, it exits `0` without creating a cohort or touching
listmonk. If the campaign already has a resend in flight, it refuses with exit
`2`; use `--resume` after a crash.

Because the cohort includes confirmed memberships created after the campaign
cutoff, a signup that confirms later is picked up on the next tick. Someone who
signed up **before** the campaign and confirmed afterwards is excluded — this
avoids re-sending to people who may already have received it.

For several campaigns, add one line per campaign and order them oldest-first.
The file lock serializes overlapping ticks.

## Local development

Requires Docker and Rust 1.94+.

```sh
make dev-init      # first run: Postgres, listmonk v6.2.0, Mailpit, demo data
make dev-up        # later starts
make dev-reset     # wipe volumes and the local ledger, then re-init
```

`make e2e` additionally uses `curl` and `python3`.

| Service | URL |
| --- | --- |
| listmonk | <http://localhost:9001> (`admin` / `adminpass`) |
| Mailpit | <http://localhost:8025> |
| Encore API user | `encore` / `encore-dev-token` |

`make dev-reset` is destructive: it removes the compose volumes and `.dev/`.
`make dev-seed` resets the demo fixture and removes Encore-created campaigns and
pending lists from listmonk. Ports are configurable through `LISTMONK_PORT`,
`MAILPIT_UI_PORT`, and `MAILPIT_SMTP_PORT`.

Run Encore against the stack:

```sh
make encore ARGS="run --campaign 420 --dry-run"
make encore ARGS="run --campaign 420 --min-new 1 --yes"
make encore ARGS="status"
```

The seed creates a double opt-in list 50 and finished campaign 420 with
subscribers 101–109:

- 106 and 107 are confirmed post-campaign signups and form the first cohort.
- 101 predates the campaign, 102 clicked, 103 opened, 104 unsubscribed, and 105
  is disabled.
- 108 signed up after the campaign but starts unconfirmed; the e2e confirms it
  later and expects a one-member resend.
- 109 signed up before the campaign and confirmed after it; it must never be
  sent.

`make e2e` resets the demo data and the local ledger, runs `run` three times,
and asserts 2 then 1 recipients, 3 messages in Mailpit, archived pending lists,
and that 109 never receives anything. It then verifies that `backfill` marks two
recipients as served and shrinks the next dry-run cohort to one. The fixed dev
credentials are local-only.

## How the cohort is chosen

1. The source campaign must be `finished` and must not be an Encore resend.
2. Cutoff = `max(started_at, updated_at)`, or `--since` if supplied.
   `updated_at` moves on every send batch and on the final status change, so it
   is a conservative boundary.
3. A subscriber is included when they currently have a **confirmed** membership
   in one of the campaign's source lists, and did **not** have an active
   membership in any source list at or before the cutoff.
4. Excluded server-side:
   - anyone with a `campaign_views` row for the campaign (they opened it)
   - anyone with a `link_clicks` row for the campaign (they clicked it)
5. Excluded locally:
   - anyone in the ledger for this campaign (already served)
   - members of another open cohort for this campaign (already owed)

`--list 5` narrows to a subset of the campaign's source lists. `--since`
accepts RFC3339 or `YYYY-MM-DD`.

This biases toward **under-sending**: someone who joined during the original
send window is skipped, because listmonk may have included them in a later
batch.

## Safety model

- **Pending list** — a private, single-opt-in list named
  `encore/campaign-<id>/cohort-<id>`. It is fully populated *before* the clone
  is started, because listmonk snapshots recipients by subscriber ID when a
  campaign starts.
- **Ledger** — `(campaign_uuid, subscriber_id)` is the primary key. Writing the
  same recipient twice is a no-op.
- **Find-or-create** — the pending list is found by exact name and the clone by
  the tag `encore-cohort-<id>`, so a crash between an HTTP call and the local
  commit does not create duplicates.
- **Resume** — `run --resume` replays the remaining stages from the persisted
  cohort state. Re-adding members is an upsert on listmonk's side, so replaying
  a batch is safe.
- **Lock** — an advisory file lock on the database prevents concurrent runs.
- **Clone** — copies content, template, messenger, headers, attribs, tags, and
  media. It never copies `archive*`, `send_at`, or `uuid`.

## Commands

| Command | Description |
| --- | --- |
| `encore init` | Write config, test credentials, create the ledger |
| `encore run --campaign 42 [--dry-run] [--min-new N] [--yes] [--list 5] [--since ...]` | Compute the cohort, send if it meets the threshold, and close |
| `encore run --campaign 42 --resume` | Continue an unfinished cohort |
| `encore run --campaign 42 --abandon` | Cancel the resend and drop the cohort without recording it |
| `encore backfill --campaign 42 (--emails FILE \| --email ADDR...) [--dry-run] [--skip-unknown]` | Mark subscribers who already received the campaign as served |
| `encore status [--cohort 7] [--json]` | Cohort state and progress |

`run` is the only writer of cohort state. On success it records the ledger,
clears the pending list, archives it, and marks the cohort closed. If a
previous run finished but was not closed, the next `run` closes it before
starting a new wave. A normal close records the ledger only when the campaign
handed off at least one message, so a total failure remains retryable.

## Marking manual sends as served

listmonk's **Send test** control does not record which subscribers received the
message, so Encore cannot see those sends and would include the recipients in a
later cohort. Seed the ledger before the first real run:

```sh
encore backfill --campaign 42 --emails served.txt
encore backfill --campaign 42 --email alice@example.com --email bob@example.com
encore backfill --campaign 42 --emails served.txt --dry-run
```

`--emails` reads one address per line and ignores blank lines and `#` comments;
use `-` for stdin. Every address must resolve to an existing listmonk
subscriber: unknown addresses abort the whole run unless `--skip-unknown` is
passed. Rows are written with `served_via = 'backfill'` and are idempotent on
`(campaign_uuid, subscriber_id)`.

Backfill refuses while an unfinished cohort exists for the campaign, because
that cohort's member list is already frozen and the new ledger rows would not
remove anyone from it. Finish or `--abandon` the cohort first.

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | ok, dry run, or below `--min-new` |
| 1 | unexpected error |
| 2 | preflight refusal (campaign not finished, open cohort, missing config, ...) |
| 4 | partial send (paused, cancelled, timed out, or `sent < to_send`) |
| 5 | another encore process holds the lock |

## Configuration

Environment (checked when the flag is absent):

```
ENCORE_CONFIG
ENCORE_LISTMONK_URL
ENCORE_LISTMONK_USER
ENCORE_LISTMONK_TOKEN
ENCORE_AUTH=basic|token
ENCORE_DB
ENCORE_INSECURE
ENCORE_TIMEOUT_SECS
ENCORE_SEND_TIMEOUT_SECS
```

The config file is TOML:

```toml
[listmonk]
url = "https://listmonk.example.com"
user = "encore"
token = "..."
auth = "basic"
timeout_secs = 30
send_timeout_secs = 3600
insecure = false

db = "/home/me/.local/share/encore/encore.db"
```

## Known limits

- listmonk has no delivery ledger. `sent` means "handed to the messenger", not
  "arrived". Opens and clicks are only positive evidence of receipt.
- listmonk's test sends leave no per-subscriber record; use `encore backfill` to
  mark their recipients as served.
- If tracking is disabled, `campaign_views`/`link_clicks` are empty, so the
  exclusions rely on membership dates and Encore's own ledger.
- The cutoff is conservative by design; subscribers who joined while the
  original campaign was still sending are not included.
- An abandoned run leaves the draft/finished clone in listmonk; it is tagged
  `encore:resend` and `encore-cohort-<id>` for cleanup.
- `serve`, `reconcile`, and `gc` are not in this slice.

## Design decisions

- Default strategy is `since` (new members minus known receivers), not `ledger`.
  `ledger` alone cannot cover campaigns sent before Encore existed.
- SQLite only. The ledger is local; listmonk's own database choice is
  irrelevant. Runtime `sqlx::query` is used instead of `query!`.
- File locking uses the stable `std::fs::File::try_lock` API; the `fs4`
  dependency is not needed on Rust 1.94.
- The schema adds `source_list_ids` and removes the `UNIQUE(campaign_id,
  strategy)` constraint in favour of a partial unique index on open cohorts, so
  the same campaign can be resent again for a later wave.
- There is no plan hash or batch bookkeeping: `run` computes and sends in one
  process, so there is no drift window, and replaying a membership batch is
  idempotent.

## Tests

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
make e2e
```

The unit/contract suite covers the query builder, the SQLite store, API
payloads, pagination, and the full run pipeline (including the below-threshold
no-write path) against a wiremock listmonk. `make e2e` runs the same flow
against the real listmonk v6.2 dev stack.

## License

MIT. See [LICENSE](LICENSE).
