-- Deterministic dev fixture for the Encore Docker Compose stack.
--
-- Run after `listmonk --install --yes` (which creates the Super Admin role).
-- Safe to re-run: demo rows are removed first, then recreated.

-- Remove Encore artifacts from earlier dev runs so cohort names and tags are
-- reusable after the local ledger is wiped.
DELETE FROM campaigns WHERE 'encore:resend' = ANY(tags);
DELETE FROM lists WHERE name LIKE 'encore/%';

-- Clean the previous fixture. Cascades handle campaign_lists, views, clicks,
-- and subscriber_lists.
DELETE FROM campaigns WHERE id = 420;
DELETE FROM links WHERE id = 100;
DELETE FROM lists WHERE id = 50;
DELETE FROM subscribers WHERE id BETWEEN 101 AND 109;

-- Fixed API user. The token is hashed exactly like listmonk's auth.HashAPIToken.
DO $$
DECLARE
    admin_role_id INTEGER;
BEGIN
    SELECT id INTO admin_role_id
    FROM roles
    WHERE type = 'user' AND name = 'Super Admin';

    IF admin_role_id IS NULL THEN
        RAISE EXCEPTION 'Super Admin role not found; run listmonk --install first';
    END IF;

    INSERT INTO users (
        username, password_login, password, email, name, type, user_role_id, status
    ) VALUES (
        'encore', false,
        encode(digest('encore-dev-token', 'sha256'), 'hex'),
        'encore@dev.local', 'Encore Dev', 'api', admin_role_id, 'enabled'
    )
    ON CONFLICT (username) DO UPDATE SET
        password_login = false,
        password = EXCLUDED.password,
        email = EXCLUDED.email,
        name = EXCLUDED.name,
        type = EXCLUDED.type,
        user_role_id = EXCLUDED.user_role_id,
        status = 'enabled';
END $$;

-- Mailpit is the SMTP sink. In listmonk v6.2 this lives in the DB, not
-- config.toml, and is read at startup (the Makefile restarts listmonk).
UPDATE settings
SET value = '[
  {
    "host": "mailpit",
    "port": 1025,
    "enabled": true,
    "username": "",
    "password": "",
    "auth_protocol": "",
    "tls_type": "none",
    "tls_skip_verify": false,
    "from_addresses": ["noreply@example.com"],
    "email_headers": [],
    "max_conns": 10,
    "idle_timeout": "15s",
    "wait_timeout": "5s",
    "hello_hostname": "",
    "max_msg_retries": 2,
    "msg_retry_delay": "10ms"
  }
]'::jsonb
WHERE key = 'smtp';

-- Demo audience.
INSERT INTO subscribers (id, uuid, email, name, status) VALUES
    (101, gen_random_uuid(), 'old@example.com',      'Old Subscriber', 'enabled'),
    (102, gen_random_uuid(), 'clicker@example.com',  'Clicker',        'enabled'),
    (103, gen_random_uuid(), 'viewer@example.com',   'Viewer',         'enabled'),
    (104, gen_random_uuid(), 'unsub@example.com',    'Unsubscribed',   'enabled'),
    (105, gen_random_uuid(), 'disabled@example.com', 'Disabled',       'disabled'),
    (106, gen_random_uuid(), 'new1@example.com',     'New One',        'enabled'),
    (107, gen_random_uuid(), 'new2@example.com',     'New Two',        'enabled'),
    (108, gen_random_uuid(), 'late-new@example.com', 'Late New',       'enabled'),
    (109, gen_random_uuid(), 'late-old@example.com', 'Late Old',       'enabled');

-- Double opt-in: 108 starts unconfirmed and is confirmed by the e2e after the
-- first send. 109 signed up before the campaign but confirmed after it, so it
-- must stay out of every cohort.
INSERT INTO lists (id, uuid, name, type, optin, status)
VALUES (50, gen_random_uuid(), 'Newsletter', 'public', 'double', 'active');

-- Cutoff is campaign 420's updated_at (2026-10-02T10:00:00Z).
-- Only 106 and 107 are confirmed, post-cutoff signups with no view/click.
INSERT INTO subscriber_lists (subscriber_id, list_id, status, created_at, updated_at) VALUES
    (101, 50, 'confirmed',    '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z'),
    (102, 50, 'confirmed',    '2026-10-05T00:00:00Z', '2026-10-05T00:00:00Z'),
    (103, 50, 'confirmed',    '2026-10-06T00:00:00Z', '2026-10-06T00:00:00Z'),
    (104, 50, 'unsubscribed', '2026-09-01T00:00:00Z', '2026-09-01T00:00:00Z'),
    (105, 50, 'confirmed',    '2026-10-07T00:00:00Z', '2026-10-07T00:00:00Z'),
    (106, 50, 'confirmed',    '2026-10-08T00:00:00Z', '2026-10-08T00:00:00Z'),
    (107, 50, 'confirmed',    '2026-10-09T00:00:00Z', '2026-10-09T00:00:00Z'),
    (108, 50, 'unconfirmed',  '2026-10-10T00:00:00Z', '2026-10-10T00:00:00Z'),
    (109, 50, 'confirmed',    '2026-09-01T00:00:00Z', '2026-10-20T00:00:00Z');

-- The campaign Encore will resend.
INSERT INTO campaigns (
    id, uuid, name, subject, from_email, body, content_type, status, type,
    messenger, to_send, sent, started_at, created_at, updated_at
) VALUES (
    420, gen_random_uuid(), 'October Dispatch', 'Hello from October',
    'noreply@example.com', '<p>October body</p>', 'richtext', 'finished',
    'regular', 'email', 2, 2,
    '2026-10-01T10:00:00Z', '2026-10-01T09:00:00Z', '2026-10-02T10:00:00Z'
);

INSERT INTO campaign_lists (campaign_id, list_id, list_name)
VALUES (420, 50, 'Newsletter');

INSERT INTO campaign_views (campaign_id, subscriber_id) VALUES (420, 103);

INSERT INTO links (id, uuid, url)
VALUES (100, gen_random_uuid(), 'https://example.com/october');

INSERT INTO link_clicks (campaign_id, link_id, subscriber_id)
VALUES (420, 100, 102);
