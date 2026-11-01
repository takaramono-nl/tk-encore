# Security policy

## Supported versions

Only the latest release on `main` receives security fixes.

## Reporting a vulnerability

Report vulnerabilities privately through GitHub's
[security advisory](https://github.com/takaramono-nl/tk-encore/security/advisories/new)
form. Do not open a public issue for a suspected vulnerability.

Please include the version, a description of the impact, and reproduction
steps. You can expect an initial response within seven days.

## Handling credentials

Encore authenticates to listmonk with an API token. Treat that token like a
password:

- Never paste a real listmonk URL, username, or token into an issue, pull
  request, or log excerpt. Redact it first.
- The credentials in `dev/`, `docker-compose.dev.yml`, and `Makefile`
  (`encore-dev-token`, `adminpass`, `listmonk`) are local-only fixtures for the
  Docker Compose stack. They are not secrets and must never be reused against a
  real instance.
- Prefer passing tokens through the environment (`ENCORE_LISTMONK_TOKEN`) or a
  `0600` config file. `encore init` writes the config file with restrictive
  permissions.
