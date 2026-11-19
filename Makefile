MM ?= ../ebac-messenger/mattermost/server
CLIENT ?= ../ebac-messenger/src

.PHONY: build test compat map stand stand-down

build:
	cargo build

test:
	cargo test

# Tests against the Go server; needs `make stand`.
compat:
	cargo test --test compat_model

# Regenerate docs/map/*.tsv from the Go source, EBAChat and docs/prod-traffic.tsv.
map:
	cd tools/mmmap && go run . -mm ../../$(MM) -client ../../$(CLIENT) -traffic ../../docs/prod-traffic.tsv \
		-migrations ../../migrations/postgres -out ../../docs/map

# Postgres (:5433) and the Go server 12.1.0 (:8065) on one database.
stand:
	docker compose -f dev/docker-compose.yml up -d

stand-down:
	docker compose -f dev/docker-compose.yml down
