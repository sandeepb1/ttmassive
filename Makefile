.PHONY: all web build release test check clean install

PREFIX ?= $(HOME)/.local

all: release

# The web UI is embedded into the binary, so build it first.
web:
	cd web && npm ci && npm run build

build: web
	cargo build

release: web
	cargo build --release

test:
	cargo test
	cd web && npm test

check:
	cargo clippy --all-targets -- -D warnings
	cd web && npx tsc --noEmit

install: release
	install -d $(PREFIX)/bin
	install -m 0755 target/release/ttm $(PREFIX)/bin/ttm

clean:
	cargo clean
	rm -rf web/dist web/node_modules
