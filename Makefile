COMPOSE := docker compose -f docker-compose.dev.yml
ROOT    := $(CURDIR)
ARGS    ?=

.DEFAULT_GOAL := help

.PHONY: help dev-init dev-up dev-down dev-reset dev-seed dev-logs encore e2e

help:
	@echo "Encore local development"
	@echo ""
	@echo "  make dev-init    first run (wipes the listmonk DB) and seed demo data"
	@echo "  make dev-up      start an already-initialized stack"
	@echo "  make dev-down    stop the stack, keep volumes"
	@echo "  make dev-reset   wipe volumes and .dev/, then dev-init"
	@echo "  make dev-seed    reset the demo fixture (and Encore artifacts)"
	@echo "  make dev-logs    follow listmonk, db, and mailpit logs"
	@echo ""
	@echo "  make encore ARGS=\"run --campaign 420 --dry-run\""
	@echo "  make e2e         live run -> send -> close assertion"
	@echo ""
	@echo "  listmonk  http://localhost:$${LISTMONK_PORT:-9001}  (admin / adminpass)"
	@echo "  mailpit   http://localhost:$${MAILPIT_UI_PORT:-8025}"
	@echo "  api user  encore / encore-dev-token"

dev-init:
	$(COMPOSE) stop listmonk >/dev/null 2>&1 || true
	$(COMPOSE) up -d --wait db mailpit
	$(COMPOSE) --profile init run --rm -T install
	$(COMPOSE) --profile init run --rm -T seed
	$(COMPOSE) up -d --wait listmonk
	@echo ""
	@echo "listmonk  http://localhost:$${LISTMONK_PORT:-9001}  (admin / adminpass)"
	@echo "mailpit   http://localhost:$${MAILPIT_UI_PORT:-8025}"
	@echo "api user  encore / encore-dev-token"

dev-up:
	$(COMPOSE) up -d --wait db mailpit listmonk

dev-down:
	$(COMPOSE) down

dev-reset:
	$(COMPOSE) down -v --remove-orphans
	rm -rf .dev
	$(MAKE) dev-init

dev-seed:
	$(COMPOSE) --profile init run --rm -T seed
	$(COMPOSE) restart listmonk
	$(COMPOSE) up -d --wait listmonk

dev-logs:
	$(COMPOSE) logs -f listmonk db mailpit

encore:
	@cd $(ROOT) && . ./dev/encore.env && cargo run --quiet -- $(ARGS)

e2e:
	@cd $(ROOT) && sh dev/e2e.sh
