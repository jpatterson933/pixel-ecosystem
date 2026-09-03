# Pixel Ecosystem
#
#   make start [small|large] [action,action,...]   start everything
#   make stop [wipe]                                stop everything; wipe also deletes the world
#   make actions                                    list every action module
#   make help
#
# Examples:
#   make start                      small map, every action enabled
#   make start large                large map, every action enabled
#   make start small move,build     small map, only move and build
#
# `make` reserves --help for itself, so use `make help` and `make actions`.

RESERVED := start stop small large wipe actions help
SIZE     := $(or $(filter small large,$(MAKECMDGOALS)),small)
WIPE     := $(filter wipe,$(MAKECMDGOALS))
ACTIONS  := $(strip $(filter-out $(RESERVED),$(MAKECMDGOALS)))
IMAGE    := pixel-ecosystem:local
WG       := WORLD_ID=$${WORLD_ID:-pending} docker compose run --rm --no-deps worldgen

.PHONY: start stop actions help image $(RESERVED)

start: image
	@WORLD_ID=$${WORLD_ID:-pending} docker compose up -d nats
	@sleep 2
	@echo "creating world from configs/$(SIZE).json$(if $(ACTIONS), with actions: $(ACTIONS), with every action)"
	@id=$$($(WG) --config /configs/$(SIZE).json --out /worlds $(if $(ACTIONS),--actions $(ACTIONS)) 2>/dev/null | tail -1); \
	  if [ -z "$$id" ]; then echo "worldgen failed:"; $(WG) --config /configs/$(SIZE).json --out /worlds $(if $(ACTIONS),--actions $(ACTIONS)); exit 1; fi; \
	  echo "WORLD_ID=$$id" > .env; \
	  echo "world $$id"; \
	  docker compose up -d
	@echo
	@echo "open http://localhost:3000"

stop:
	@docker compose down $(if $(WIPE),-v)
	@if [ -n "$(WIPE)" ]; then rm -rf data worlds .env; echo "wiped data and worlds and the event log"; fi

actions: image
	@$(WG) --list-actions 2>/dev/null

help:
	@sed -n '2,14p' Makefile | sed 's/^# \{0,1\}//'

image:
	@docker image inspect $(IMAGE) >/dev/null 2>&1 || WORLD_ID=pending docker compose build worldgen

# Size, wipe, and action-list words are goals that do nothing on their own.
small large wipe:
	@:
%:
	@:
