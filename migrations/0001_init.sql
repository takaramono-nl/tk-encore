-- Encore ledger schema.
--
-- All timestamps are stored as UTC RFC3339 with a trailing Z. listmonk returns
-- offsets, so normalising here keeps TEXT comparisons (state cutoffs) sane.

CREATE TABLE cohorts (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    campaign_id        INTEGER NOT NULL,
    campaign_uuid      TEXT    NOT NULL,
    source_list_ids    TEXT    NOT NULL,
    pending_list_id    INTEGER,
    resend_campaign_id INTEGER,
    strategy           TEXT    NOT NULL CHECK (strategy IN ('since', 'ledger', 'opens')),
    state              TEXT    NOT NULL,
    since_at           TEXT    NOT NULL,
    query_text         TEXT    NOT NULL,
    expected_count     INTEGER NOT NULL,
    created_at         TEXT    NOT NULL,
    sent_at            TEXT,
    closed_at          TEXT,
    last_error         TEXT
);

-- One in-flight cohort per campaign. Closed/cancelled runs free the slot so
-- the same campaign can be resent again for a later wave of subscribers.
CREATE UNIQUE INDEX idx_cohorts_open
    ON cohorts(campaign_id)
    WHERE state NOT IN ('closed', 'cancelled');

CREATE TABLE cohort_members (
    cohort_id     INTEGER NOT NULL REFERENCES cohorts(id) ON DELETE CASCADE,
    subscriber_id INTEGER NOT NULL,
    email         TEXT    NOT NULL,
    added_at      TEXT    NOT NULL,
    PRIMARY KEY (cohort_id, subscriber_id)
);

CREATE TABLE ledger (
    campaign_uuid TEXT    NOT NULL,
    subscriber_id INTEGER NOT NULL,
    campaign_id   INTEGER NOT NULL,
    email         TEXT    NOT NULL,
    served_via    TEXT    NOT NULL CHECK (served_via IN ('original', 'resend', 'backfill')),
    recorded_at   TEXT    NOT NULL,
    PRIMARY KEY (campaign_uuid, subscriber_id)
);
