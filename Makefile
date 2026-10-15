.PHONY: all web build release test check clean install publish

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

# Cut a release: `make publish VERSION=0.2.0 [TITLE="what this one is about"]`. Tags it and pushes;
# GitHub Actions builds the binaries and publishes the release, and running servers offer it.
publish:
	@test -n "$(VERSION)" || { echo "usage: make publish VERSION=x.y.z [TITLE=...]"; exit 1; }
	@echo "$(VERSION)" | rg -q '^[0-9]+\.[0-9]+\.[0-9]+$$' || { echo "VERSION must look like 1.2.3"; exit 1; }
	@test -z "$$(git status --porcelain)" || { echo "the working tree is not clean"; exit 1; }
	@test "$$(git branch --show-current)" = main || { echo "release from main"; exit 1; }
	@git fetch -q origin main && test "$$(git rev-parse HEAD)" = "$$(git rev-parse origin/main)" || { echo "main is not in sync with origin/main"; exit 1; }
	@! git rev-parse -q --verify "refs/tags/v$(VERSION)" >/dev/null || { echo "v$(VERSION) already exists"; exit 1; }
	sd '^version = ".*"' 'version = "$(VERSION)"' Cargo.toml
	cargo check --quiet
	git commit -am "Release v$(VERSION)"
	git tag -a "v$(VERSION)" -m "$${TITLE:-$$(git log -1 --format=%s HEAD~1)}"
	git push origin main "v$(VERSION)"

clean:
	cargo clean
	rm -rf web/dist web/node_modules
