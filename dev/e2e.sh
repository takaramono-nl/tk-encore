#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
# shellcheck disable=SC1091
. ./dev/encore.env

COMPOSE="docker compose -f docker-compose.dev.yml"
API="http://127.0.0.1:${LISTMONK_PORT:-9001}"
MAILPIT="http://127.0.0.1:${MAILPIT_UI_PORT:-8025}"
ENCORE=./target/debug/encore
AUTH="encore:encore-dev-token"

say() {
    printf '\n==> %s\n' "$*"
}

fail() {
    echo "$*" >&2
    exit 1
}

campaign_count() {
    $ENCORE status --json | python3 -c 'import json,sys; print(len(json.load(sys.stdin)))'
}

mailpit_total() {
    curl -fsS "$MAILPIT/api/v1/messages" |
        python3 -c 'import json,sys; print(json.load(sys.stdin)["total"])'
}

say "starting stack"
$COMPOSE up -d --wait db mailpit listmonk

say "resetting demo data"
$COMPOSE --profile init run --rm -T seed >/dev/null
$COMPOSE restart listmonk >/dev/null
$COMPOSE up -d --wait listmonk

say "fresh ledger"
rm -f "$ENCORE_DB" "$ENCORE_DB-shm" "$ENCORE_DB-wal"
cargo build --quiet

say "dry run"
plan=$($ENCORE run --campaign 420 --min-new 1 --dry-run --json)
recipients=$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin)["recipients"])')
[ "$recipients" = "2" ] || fail "expected 2 recipients, got $recipients"
echo "recipients: $recipients"

say "clearing mailpit"
curl -fsS -X DELETE "$MAILPIT/api/v1/messages" >/dev/null

say "run 1 (106 and 107)"
$ENCORE run --campaign 420 --min-new 1 --yes

say "verifying first send"
[ "$(campaign_count)" = "1" ] || fail "expected 1 cohort"
cohort_one=$($ENCORE status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)[0]["id"])')
detail=$($ENCORE status --cohort "$cohort_one" --json)
resend_id=$(printf '%s' "$detail" | python3 -c 'import json,sys; print(json.load(sys.stdin)["resend_campaign_id"])')
pending_id=$(printf '%s' "$detail" | python3 -c 'import json,sys; print(json.load(sys.stdin)["pending_list_id"])')

curl -fsS -u "$AUTH" "$API/api/campaigns/$resend_id" | python3 -c '
import json, sys
c = json.load(sys.stdin)["data"]
assert c["status"] == "finished", c["status"]
assert c["sent"] == 2, c["sent"]
assert c["to_send"] == 2, c["to_send"]
print("campaign", c["id"], c["status"], c["sent"], "/", c["to_send"])
'

curl -fsS -u "$AUTH" "$API/api/lists/$pending_id" | python3 -c '
import json, sys
l = json.load(sys.stdin)["data"]
assert l["status"] == "archived", l["status"]
assert l["subscriber_count"] == 0, l["subscriber_count"]
print("pending list", l["id"], l["status"], l["subscriber_count"])
'

[ "$(mailpit_total)" = "2" ] || fail "expected 2 messages, got $(mailpit_total)"

say "run 2 (nothing new)"
out=$($ENCORE run --campaign 420 --min-new 1 --yes)
printf '%s' "$out" | grep -q "nothing sent" || fail "expected 'nothing sent': $out"
[ "$(campaign_count)" = "1" ] || fail "run 2 created a cohort"
[ "$(mailpit_total)" = "2" ] || fail "run 2 sent mail"
echo "$out"

say "confirming 108 (post-campaign signup, confirmed late)"
$COMPOSE exec -T db psql -U listmonk -d listmonk -v ON_ERROR_STOP=1 -c \
    "UPDATE subscriber_lists SET status = 'confirmed', updated_at = NOW()
     WHERE subscriber_id = 108 AND list_id = 50;" >/dev/null

say "run 3 (108 only)"
$ENCORE run --campaign 420 --min-new 1 --yes

say "verifying second send"
[ "$(campaign_count)" = "2" ] || fail "expected 2 cohorts"
detail=$($ENCORE status --json | python3 -c 'import json,sys; print(json.dumps(json.load(sys.stdin)[0]))')
resend_id=$(printf '%s' "$detail" | python3 -c 'import json,sys; print(json.load(sys.stdin)["resend_campaign_id"])')
expected=$(printf '%s' "$detail" | python3 -c 'import json,sys; print(json.load(sys.stdin)["expected_count"])')
[ "$expected" = "1" ] || fail "expected a 1-member cohort, got $expected"

curl -fsS -u "$AUTH" "$API/api/campaigns/$resend_id" | python3 -c '
import json, sys
c = json.load(sys.stdin)["data"]
assert c["status"] == "finished", c["status"]
assert c["sent"] == 1, c["sent"]
assert c["to_send"] == 1, c["to_send"]
print("campaign", c["id"], c["status"], c["sent"], "/", c["to_send"])
'

curl -fsS "$MAILPIT/api/v1/messages" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["total"] == 3, d["total"]
to = sorted(m["To"][0]["Address"] for m in d["messages"])
assert to == ["late-new@example.com", "new1@example.com", "new2@example.com"], to
assert "late-old@example.com" not in to
print("messages:", ", ".join(to))
'

say "run 4 (nothing new)"
out=$($ENCORE run --campaign 420 --min-new 1 --yes)
printf '%s' "$out" | grep -q "nothing sent" || fail "expected 'nothing sent': $out"
[ "$(campaign_count)" = "2" ] || fail "run 4 created a cohort"
[ "$(mailpit_total)" = "3" ] || fail "run 4 sent mail"
echo "$out"

say "backfill (manual sends already served)"
rm -f "$ENCORE_DB" "$ENCORE_DB-shm" "$ENCORE_DB-wal"

out=$($ENCORE backfill --campaign 420 \
    --email New1@Example.com --email new2@example.com --json)
printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["requested"] == 2, d
assert d["resolved"] == 2, d
assert d["recorded"] == 2, d
'

out=$($ENCORE backfill --campaign 420 \
    --email new1@example.com --email new2@example.com --json)
printf '%s' "$out" | python3 -c '
import json, sys
d = json.load(sys.stdin)
assert d["recorded"] == 0, d
assert d["already_served"] == 2, d
'

plan=$($ENCORE run --campaign 420 --min-new 1 --dry-run --json)
recipients=$(printf '%s' "$plan" | python3 -c 'import json,sys; print(json.load(sys.stdin)["recipients"])')
[ "$recipients" = "1" ] || fail "expected 1 recipient after backfill, got $recipients"
echo "recipients after backfill: $recipients"

say "OK"
