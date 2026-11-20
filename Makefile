MM ?= ../ebac-messenger/mattermost/server
CLIENT ?= ../ebac-messenger/src

.PHONY: build test compat map stand stand-down

# Builds run at the lowest priority so the stand, the clients and the desktop stay responsive.
CARGO ?= nice -n 19 cargo

build:
	$(CARGO) build --release

test:
	$(CARGO) test --release

# Tests against the Go server; needs `make stand`.
compat:
	$(CARGO) test --release --test compat_model

# Regenerate docs/map/*.tsv from the Go source, EBAChat and docs/prod-traffic.tsv.
map:
	cd tools/mmmap && go run . -mm ../../$(MM) -client ../../$(CLIENT) -traffic ../../docs/prod-traffic.tsv \
		-migrations ../../migrations/postgres -out ../../docs/map

# Postgres (:5433) and the Go server 12.1.0 (:8065) on one database.
stand:
	docker compose -f dev/docker-compose.yml up -d

stand-down:
	docker compose -f dev/docker-compose.yml down
