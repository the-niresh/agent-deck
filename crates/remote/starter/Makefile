# crates/remote/starter — local self-host operations.
#
# Primary entry points:
#   make start     → docker up + launch frontend app (prompts to run setup if missing)
#   make up        → just docker (no frontend app); use from a second terminal
#   make stop      → docker compose down
#   make restart   → stop + up (docker only, no frontend relaunch)
#   make rebuild   → up with --build (pulls in upstream changes)
#   make logs      → tail remote-server logs
#   make status    → docker compose ps
#   make backup    → pg_dump to $SELFHOST_DATA_DIR/backups/<timestamp>.sql
#   make clean     → tear down and wipe volumes + data dir (destructive)

SHELL := /bin/bash

# Resolved at parse time from this Makefile's location (crates/remote/starter/).
VK_REPO_ROOT := $(abspath $(CURDIR)/../../..)
COMPOSE_DIR := $(abspath $(CURDIR)/..)
ENV_FILE := $(VK_REPO_ROOT)/.env.remote

# Traefik exposure is opt-in: the overlay is layered on only when
# PUBLIC_HOSTNAME is set in .env.remote. Without it the stack stays bound to
# loopback exactly as before.
TRAEFIK_OVERLAY := $(shell grep -qsE '^PUBLIC_HOSTNAME=.+' $(ENV_FILE) && printf '%s' "-f $(CURDIR)/docker-compose.traefik.yml")

# Setting RELAY_HOSTNAME turns on the relay profile, which is what lets the
# remote UI reach workspaces on your local machine.
RELAY_PROFILE := $(shell grep -qsE '^RELAY_HOSTNAME=.+' $(ENV_FILE) && printf '%s' "--profile relay")

COMPOSE := docker compose \
  -f $(COMPOSE_DIR)/docker-compose.yml \
  -f $(CURDIR)/docker-compose.override.yml \
  -f $(CURDIR)/docker-compose.ports.yml \
  -f $(CURDIR)/docker-compose.no-ssh.yml \
  $(TRAEFIK_OVERLAY) \
  $(RELAY_PROFILE) \
  --env-file $(ENV_FILE)

.DEFAULT_GOAL := help
.PHONY: help setup start up stop restart rebuild logs status backup clean _ensure_setup

help:
	@awk 'NR==1,/^$$/ { if (/^#/) { sub(/^#[[:space:]]?/, ""); print } }' $(MAKEFILE_LIST)

setup:
	@./setup.sh

# Guard: if .env.remote isn't there yet, offer to run setup. Safe to chain
# before any recipe that touches docker.
_ensure_setup:
	@if [ ! -f $(ENV_FILE) ]; then \
	  read -rp "Stack isn't configured yet. Run 'make setup' now? [Y/n] " ans; \
	  case "$$ans" in n|N) echo "aborted"; exit 1 ;; esac; \
	  ./setup.sh; \
	fi

up: _ensure_setup
	@cd "$(COMPOSE_DIR)" && \
	  if docker images -q remote-remote-server 2>/dev/null | grep -q .; then \
	    $(COMPOSE) up -d; \
	  else \
	    echo "first start — building images (~10-15 min)..."; \
	    $(COMPOSE) up -d --build; \
	  fi
	@. "$(ENV_FILE)" && \
	  echo "" && \
	  echo "Backend up at http://localhost:$${REMOTE_SERVER_PORT:-13000}" && \
	  grep -E '^SELF_HOST_LOCAL_AUTH_(EMAIL|PASSWORD)=' "$(ENV_FILE)" | sed 's/^/  /'

# Runs THIS checkout's client, not the published npm package. The upstream
# package is pinned at the sunset release, so `npx agent-deck@latest` would
# serve the export-only page and none of this fork's changes.
start: up
	@. "$(ENV_FILE)" && \
	  echo "" && \
	  echo "Launching frontend app at http://localhost:$${FRONTEND_PORT:-13333} (Ctrl+C exits; docker stays up — use 'make stop' after)..." && \
	  cd "$(VK_REPO_ROOT)" && \
	  AGENT_DECK_SHARED_API_BASE=http://localhost:$${REMOTE_SERVER_PORT:-13000} \
	    FRONTEND_PORT=$${FRONTEND_PORT:-13333} \
	    pnpm run dev

stop: _ensure_setup
	@cd "$(COMPOSE_DIR)" && $(COMPOSE) down

restart: stop up

rebuild: _ensure_setup
	@cd "$(COMPOSE_DIR)" && $(COMPOSE) up -d --build

logs: _ensure_setup
	@cd "$(COMPOSE_DIR)" && $(COMPOSE) logs -f remote-server

status: _ensure_setup
	@cd "$(COMPOSE_DIR)" && $(COMPOSE) ps

backup: _ensure_setup
	@. "$(ENV_FILE)" && \
	  dest="$$SELFHOST_DATA_DIR/backups" && \
	  mkdir -p "$$dest" && \
	  f="$$dest/backup-$$(date +%Y%m%d-%H%M%S).sql" && \
	  PGPASSWORD=remote pg_dump -h localhost -p "$${REMOTE_DB_PORT:-15433}" -U remote remote > "$$f" && \
	  echo "wrote $$f"

clean: _ensure_setup
	@. "$(ENV_FILE)" && \
	  read -rp "Destroy all data (volumes + $$SELFHOST_DATA_DIR + $(ENV_FILE))? [y/N] " ans; \
	  case "$$ans" in y|Y) ;; *) echo "aborted"; exit 1 ;; esac; \
	  cd "$(COMPOSE_DIR)" && $(COMPOSE) down -v; \
	  rm -rf "$$SELFHOST_DATA_DIR"; \
	  rm -f "$(ENV_FILE)"; \
	  echo "wiped $$SELFHOST_DATA_DIR, removed $(ENV_FILE)"
