# Pixel Ecosystem
#
#   make start          start everything with the small map (default)
#   make start small    start everything with configs/small.json
#   make start large    start everything with configs/large.json
#   make stop           stop everything, keep the world
#   make stop wipe      stop everything and delete the world, data, and event log

SIZE := $(or $(filter small large,$(MAKECMDGOALS)),small)
WIPE := $(filter wipe,$(MAKECMDGOALS))
IMAGE := pixel-ecosystem:local

.PHONY: start stop small large wipe

start:
	@docker image inspect $(IMAGE) >/dev/null 2>&1 || WORLD_ID=pending docker compose build worldgen
	@WORLD_ID=$${WORLD_ID:-pending} docker compose up -d nats
	@sleep 2
	@echo "creating world from configs/$(SIZE).json"
	@id=$$(WORLD_ID=pending docker compose run --rm worldgen --config /configs/$(SIZE).json --out /worlds 2>/dev/null | tail -1); \
	  if [ -z "$$id" ]; then echo "worldgen failed"; WORLD_ID=pending docker compose run --rm worldgen --config /configs/$(SIZE).json --out /worlds; exit 1; fi; \
	  echo "WORLD_ID=$$id" > .env; \
	  echo "world $$id"; \
	  docker compose up -d
	@echo
	@echo "open http://localhost:3000"

stop:
	@docker compose down $(if $(WIPE),-v)
	@if [ -n "$(WIPE)" ]; then rm -rf data worlds .env; echo "wiped data and worlds and the event log"; fi

# Argument words are targets that do nothing.
small large wipe:
	@:
