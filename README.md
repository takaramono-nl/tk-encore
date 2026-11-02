# Encore

[![ci](https://github.com/takaramono-nl/tk-encore/actions/workflows/ci.yml/badge.svg)](https://github.com/takaramono-nl/tk-encore/actions/workflows/ci.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Resend a past listmonk campaign to subscribers who joined after it was sent.

Encore is a standalone Rust binary with its own SQLite ledger. It does not fork
listmonk or change its schema. Every resend goes through a private "pending
list" that acts as the durable marker of "still owed this mail".

## Requirements

- listmonk **v6.2+** (built and tested against v6.2.0; v7 is in beta and
  untested)
- An API user with these user-role permissions:
  - `campaigns:get`, `campaigns:manage`, `campaigns:send`
  - `lists:get_all`, `lists:manage_all`
  - `subscribers:get_all`, `subscribers:manage`
  - `subscribers:sql_query` (required for the cohort query expression)

`campaigns:get` and `campaigns:manage` are still scoped to the API user's
permitted lists, so the blanket list permissions are required too.
`lists:manage_all` is needed to create Encore's `encore/...` lists, and
`lists:get_all` is needed to find and read them. `subscribers:get_all` is
required for `backfill` lookups and to see subscribers outside per-list roles.
`subscribers:sql_query` authorizes the query expression but does not bypass list
scoping.

`encore init` only probes `GET /api/lists`, so a successful init does not prove
the campaign or subscriber permissions. Run `encore run --campaign <id>
--dry-run` after setup to exercise the full permission set.

### Permission mapping

| Encore operation | Endpoint | Permission |
| --- | --- | --- |
| Read campaigns | `GET /api/campaigns[/:id]` | `campaigns:get` + list access |
| Create resend campaign | `POST /api/campaigns` | `campaigns:manage` + list access |
| Start campaign | `PUT /api/campaigns/:id/status` | `campaigns:send` + list access |
| Find/read lists | `GET /api/lists[/:id]` | `lists:get_all` or per-list get role |
| Create pending list | `POST /api/lists` | `lists:manage_all` |
| Archive list | `PUT /api/lists/:id` | `lists:manage_all` or per-list manage role |
| Resolve/query subscribers | `GET /api/subscribers` | `subscribers:get_all` (or `subscribers:get` + list roles) + `subscribers:sql_query` |
| Add/remove list members | `PUT /api/subscribers/lists` | `subscribers:manage` + list manage |

## Build

Requires Rust 1.94+ (the `sqlx` 0.9 MSRV).

```sh
cargo build --release --locked
# target/release/encore
```

## Container image

Tagged releases publish a Linux/amd64 image to GitHub Container Registry:

```sh
docker pull ghcr.io/takaramono-nl/tk-encore:0.2.0
```

Tagged releases provide `:0.2.0`, `:0.2`, and `:latest`; each build also gets a
`:sha-...` tag. The image pre-sets `ENCORE_DB=/data/encore.db`, so mount a
volume at `/data` to keep the ledger:

```sh
docker volume create encore-data

docker run --rm -v encore-data:/data \
  -e ENCORE_LISTMONK_URL="https://listmonk.example.com" \
  -e ENCORE_LISTMONK_USER="encore" \
  -e ENCORE_LISTMONK_TOKEN="..." \
  ghcr.io/takaramono-nl/tk-encore:0.2.0 init

docker run --rm -v encore-data:/data \
  -e ENCORE_LISTMONK_URL="https://listmonk.example.com" \
  -e ENCORE_LISTMONK_USER="encore" \
  -e ENCORE_LISTMONK_TOKEN="..." \
  ghcr.io/takaramono-nl/tk-encore:0.2.0 run --campaign 42 --dry-run
```

The image is a batch CLI, not a server. Its default command is `sleep infinity`,
so it can either be run one-shot as above or kept alive and driven with
`docker exec` from an external scheduler. `encore init` writes its config file
inside the container's home directory; mount that directory as well if you want
the file to persist across containers, otherwise pass every setting through the
environment.

## Quick start

```sh
export ENCORE_LISTMONK_URL="https://listmonk.example.com"
export ENCORE_LISTMONK_USER="encore"
export ENCORE_LISTMONK_TOKEN="..."

encore init
encore run --campaign 42 --dry-run   # show the cohort without creating one
encore run --campaign 42 --yes       # compute, send, and close
```

`init` writes `~/.config/encore/config.toml` (mode 0600) and creates
`~/.local/share/encore/encore.db`. Connection settings can be overridden with
flags or environment variables; the two timeout settings are config-file or
environment only. Use `init --force` to overwrite an existing config and
`init --no-test` to skip the credential probe.

## How a resend works

`run` executes one pipeline:

1. Compute the cohort and persist it as a `planned` cohort.
2. Find or create `encore/campaign-<id>/cohort-<id>`.
3. Add the cohort in batches and verify the live list count.
4. Clone the source campaign, targeting only the pending list.
5. Start the clone and poll listmonk until it is finished.
6. Record the ledger, remove members, archive the pending list, and close the
   cohort.

The cohort is frozen before the clone is created, so the pending list and the
campaign's recipient snapshot always agree.

## Listing the cohort

`--print-emails` writes the cohort to stdout as bare addresses, one per line,
and sends the usual report to stderr. It requires `--dry-run`, so it never
creates a cohort and never talks to listmonk beyond computing the cohort:

```sh
encore run --campaign 42 --dry-run --print-emails > cohort-42.txt
```

Review the file, send from listmonk by hand, then record the recipients so a
later `run` will not include them:

```sh
encore backfill --campaign 42 --emails cohort-42.txt --dry-run   # preview
encore backfill --campaign 42 --emails cohort-42.txt
```

`encore status --cohort 7 --print-emails` does the same for a cohort already in
the local ledger, without contacting listmonk. Both outputs are exactly what
`backfill --emails` accepts; `--emails -` reads the same format from stdin.

Backfilling marks those subscribers as served for that campaign, so only do it
after the manual send has actually happened. If you backfill a cohort you never
sent, Encore will never offer those addresses again for that campaign.

## Monitoring new subscribers

listmonk does not emit a webhook when a subscriber registers or confirms a
double opt-in subscription, so Encore polls. Run it from cron every 15–30
minutes:

```sh
*/15 * * * * cd /srv/encore && . /etc/encore/env && \
  encore run --campaign 42 --min-new 5 --yes
```

`run` recomputes the exact cohort on every tick. If fewer than `--min-new`
subscribers are waiting, it exits `0` without creating a cohort or sending
anything. `--min-new` defaults to `1`; a higher value batches small waves into
larger sends. If the campaign already has an open, unfinished cohort, `run`
refuses with exit `2`; use `--resume` after a crash.

Because the cohort includes confirmed memberships created after the campaign
cutoff, a signup that confirms later is picked up on the next tick. Someone who
signed up **before** the campaign and confirmed afterwards is excluded — this
avoids re-sending to people who may already have received it.

For several campaigns, add one line per campaign and order them oldest-first.
The file lock is non-blocking: an overlapping tick is refused with exit `5`
rather than queued.

After a cohort closes, running the same campaign again starts a new wave for
subscribers who joined later. Closed and cancelled cohorts do not block the next
run.

## Local development

Requires Docker and Rust 1.94+.

```sh
make dev-init      # first run: Postgres, listmonk v6.2.0, Mailpit, demo data
make dev-up        # later starts
make dev-reset     # wipe volumes and the local ledger, then re-init
make dev-logs      # follow listmonk, db, and mailpit logs
```

`make e2e` additionally uses `curl` and `python3`, and expects `make dev-init`
to have been run once.

| Service | URL |
| --- | --- |
| listmonk | <http://localhost:9001> (`admin` / `adminpass`) |
| Mailpit | <http://localhost:8025> |
| Encore API user | `encore` / `encore-dev-token` |

`make dev-reset` is destructive: it removes the compose volumes and `.dev/`.
`make dev-seed` resets the demo fixture and removes Encore-created campaigns and
pending lists from listmonk, but it does not clear the local ledger. Ports are
configurable through `LISTMONK_PORT`, `MAILPIT_UI_PORT`, and
`MAILPIT_SMTP_PORT`.

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

`make e2e` resets the demo data and the local ledger, runs a dry run plus two
sending ticks and two no-op ticks, and asserts 2 then 1 recipients, 3 messages
in Mailpit, archived pending lists, and that 109 never receives anything. It
then verifies that `backfill` marks two recipients as served and shrinks the
next dry-run cohort to one. The fixed dev credentials are local-only.

## How the cohort is chosen

1. The source campaign must be `finished` and must not be an Encore resend.
2. Cutoff = `max(started_at, updated_at)`, or `--since` if supplied.
   `updated_at` moves on every send batch and on the final status change, so it
   is a conservative boundary.
3. A subscriber is included when they are **enabled**, currently have a
   **confirmed** membership in one of the campaign's source lists, and did
   **not** have any membership in a source list at or before the cutoff whose
   status is something other than `unsubscribed`. This is what excludes a
   subscriber who signed up before the campaign and confirmed afterwards.
4. Excluded server-side:
   - anyone with a `campaign_views` row for the campaign (they opened it)
   - anyone with a `link_clicks` row for the campaign (they clicked it)
5. Excluded locally:
   - anyone in the ledger for this campaign (already served)
   - members of another open cohort for this campaign (already owed)

`--list 5 --list 6` narrows to a subset of the campaign's source lists; the
flag can be repeated and every ID must belong to the campaign. `--since`
accepts RFC3339 or `YYYY-MM-DD` (interpreted as midnight UTC).

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
- **Lock** — `run` without `--dry-run` and `backfill` without `--dry-run` take
  an advisory lock on `<db>.lock`. Dry runs, `status`, and `init` do not.
- **Clone** — copies subject, body, content type, template, from address,
  messenger, headers, attribs, source tags, and media. It never copies
  `archive*`, `send_at`, `uuid`, or delivery stats.
- **Close** — records the ledger only when listmonk handed off at least one
  message, so a total failure remains retryable. A campaign that finishes with
  `sent < to_send` is still closed; when `sent > 0`, the whole cohort is
  recorded. See Known limits.

## Commands

| Command | Description |
| --- | --- |
| `encore init [--force] [--no-test]` | Write config, test credentials, create the ledger |
| `encore run --campaign 42 [--dry-run [--print-emails]] [--min-new N] [--yes] [--list ID]... [--since ...]` | Compute the cohort, send if it meets the threshold, and close |
| `encore run --campaign 42 --resume` | Continue an unfinished cohort |
| `encore run --campaign 42 --abandon` | Cancel the resend and drop the cohort without recording it |
| `encore backfill --campaign 42 (--emails FILE \| --email ADDR)... [--dry-run] [--skip-unknown]` | Mark subscribers who already received the campaign as served |
| `encore status [--cohort 7 [--print-emails]] [--json]` | Cohort state and progress |

`--print-emails` requires `--dry-run` and cannot be combined with `--json`.

Without `--yes`, `run` asks for confirmation on a terminal. In a
non-interactive context it refuses with exit `2` rather than sending.

`--resume` ignores cohort-shaping flags such as `--since`, `--list`, and
`--min-new`, because the cohort is already frozen. A cohort in the `failed`
state cannot be resumed; inspect it with `encore status --cohort <id>` and use
`--abandon` if it should be discarded.

`--abandon` cancels a running or paused clone and archives the pending list, but
it does not record the ledger. Abandoning after a send has started means a later
run can resend to the same people; use it only before the send starts, or accept
that manual cleanup is required.

If a previous run finished but was not closed, the next non-dry-run `run` closes
it before starting a new wave. A dry run against a finished-but-open cohort
prints a message and exits `0` instead of showing a new cohort.

`run` is the only writer of cohort state. On success it records the ledger,
clears the pending list, archives it, and marks the cohort closed. A normal
close records the ledger only when the campaign handed off at least one
message, so a total failure remains retryable.

### Global flags and JSON output

`--json`, `--config`, `--url`, `--user`, `--token`, `--auth`, `--db`, and
`--insecure` are global flags and can appear before or after the subcommand.
`--json` prints machine-readable output for every subcommand:

- `run --dry-run --json` returns the cohort report.
- `run --json` returns the send summary.
- `backfill --json` returns the backfill report.
- `status --json` returns the cohort list, or one cohort object with `--cohort`.

`--json` cannot be combined with `--print-emails`.

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
use `-` for stdin. `--emails` and `--email` can be combined. Addresses are
matched case-insensitively, and invalid addresses abort before anything is
written. Every address must resolve to an existing listmonk subscriber visible
to the API user: unknown addresses abort the whole run unless `--skip-unknown`
is passed. Rows are written with `served_via = 'backfill'` and are idempotent on
`(campaign_uuid, subscriber_id)`.

Backfill refuses while an unfinished cohort exists for the campaign, because
that cohort's member list is already frozen and the new ledger rows would not
remove anyone from it. Finish or `--abandon` the cohort first.

An `--emails` file or stream that contains no addresses is a successful no-op,
so piping `encore run --dry-run --print-emails` straight into
`encore backfill --emails -` does not fail on an empty cohort.

### "not listmonk subscribers" but the address exists

`backfill` resolves addresses through `/api/subscribers` as the API user, and
listmonk scopes that endpoint to the user's permitted lists. If the API user
lacks `subscribers:get_all` and has no list role for the subscriber's list, the
API returns zero rows even though the admin UI shows the subscriber.

Grant `subscribers:get_all` (or a get list role for the relevant list) and retry.
To confirm what the API user can see:

```sh
curl -s -u "$ENCORE_LISTMONK_USER:$ENCORE_LISTMONK_TOKEN" \
  --get "$ENCORE_LISTMONK_URL/api/subscribers" \
  --data-urlencode "query=LOWER(subscribers.email) IN ('the-address@example.com')" \
  --data "page=1" --data "per_page=100"
```

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | ok, dry run, below `--min-new`, or user declined the prompt |
| 1 | config/credentials, API, database, not-found, or other unexpected error |
| 2 | preflight refusal (campaign not finished, open cohort, invalid flags, size mismatch, ...) |
| 4 | partial send (paused, cancelled, timed out, or `sent < to_send`) |
| 5 | another encore process holds the lock |

## Configuration

Resolution order: CLI flag > environment variable > config file > built-in
default.

| Variable | Default | Purpose |
| --- | --- | --- |
| `ENCORE_CONFIG` | `~/.config/encore/config.toml` | Config file path |
| `ENCORE_LISTMONK_URL` | `http://localhost:9000` | listmonk base URL |
| `ENCORE_LISTMONK_USER` | (required) | API user |
| `ENCORE_LISTMONK_TOKEN` | (required) | API token |
| `ENCORE_AUTH` | `basic` | `basic` or `token` |
| `ENCORE_DB` | `~/.local/share/encore/encore.db` | SQLite ledger path |
| `ENCORE_INSECURE` | `false` | Accept invalid TLS certs (`1`, `true`, or `yes`) |
| `ENCORE_TIMEOUT_SECS` | `30` | Per-request HTTP timeout |
| `ENCORE_SEND_TIMEOUT_SECS` | `3600` | Total wait for a resend to finish |

The two timeout settings have no CLI flags.

The config file is TOML:

```toml
[listmonk]
url = "https://listmonk.example.com"
user = "encore"
token = "..."
auth = "basic"       # or "token"
timeout_secs = 30
send_timeout_secs = 3600
insecure = false

db = "/home/me/.local/share/encore/encore.db"
```

`encore init` writes the file with mode 0600 on Unix.

## Known limits

- listmonk has no delivery ledger. `sent` means "handed to the messenger", not
  "arrived". Opens and clicks are only positive evidence of receipt.
- listmonk's test sends leave no per-subscriber record; use `encore backfill` to
  mark their recipients as served.
- If tracking is disabled, `campaign_views`/`link_clicks` are empty, so the
  exclusions rely on membership dates and Encore's own ledger.
- The cutoff is conservative by design; subscribers who joined while the
  original campaign was still sending are not included.
- **Partial sends are closed as served.** If listmonk reports `finished` with
  `sent < to_send`, Encore closes the cohort and exits `4`. When `sent > 0`,
  the whole cohort is recorded, and listmonk does not expose which recipients
  were skipped, so those addresses will not be retried automatically. With
  `sent = 0`, the ledger stays empty and a later run can retry.
- **Abandoning after a send starts loses the served set.** `--abandon` does not
  record the ledger, so a later run can resend to the same people.
- An abandoned run leaves the draft, finished, or cancelled clone in listmonk;
  it is tagged `encore:resend` and `encore-cohort-<id>` for cleanup.
- `serve`, `reconcile`, and `gc` are not implemented yet.

## Design decisions

- Default strategy is `since` (new members minus known receivers), not `ledger`.
  `ledger` alone cannot cover campaigns sent before Encore existed. Only the
  `since` strategy is currently implemented; `ledger` and `opens` are reserved
  in the schema.
- SQLite only. The ledger is local; listmonk's own database choice is
  irrelevant. Runtime `sqlx::query` is used instead of `query!`.
- File locking uses the stable `std::fs::File::try_lock` API; the `fs4`
  dependency is not needed on Rust 1.94.
- Cohorts store `source_list_ids`; a partial unique index allows only one open
  cohort per campaign. Closed and cancelled cohorts free the slot so the same
  campaign can be resent in later waves.
- There is no plan hash or batch bookkeeping: `run` computes and sends in one
  process, so there is no drift window, and replaying a membership batch is
  idempotent.

## Tests

```sh
cargo test --locked
cargo clippy --all-targets -- -D warnings
cargo fmt --check
cargo build --locked
make e2e
```

The unit/contract suite covers the query builder, the SQLite store, API
payloads, pagination, and the full run pipeline (including the below-threshold
no-write path) against a wiremock listmonk. It also covers backfill, including
idempotency and the open-cohort refusal. `make e2e` runs the same flow against
the real listmonk v6.2 dev stack after `make dev-init`.

## Security

Encore authenticates with a listmonk API token. Prefer `ENCORE_LISTMONK_TOKEN`
or the `0600` config file over `--token`, which can leak through shell history
and process listings. Report vulnerabilities through the process described in
[SECURITY.md](SECURITY.md), not a public issue.

## License

MIT. See [LICENSE](LICENSE).
