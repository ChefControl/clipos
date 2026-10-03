# Local development shortcuts. Run `make help` for the list.
DATABASE_URL ?= postgres://clipos:clipos@localhost:5432/clipos
export DATABASE_URL

.PHONY: help deps api worker web test coverage lint fmt gen-api images

help: ## Show this help
	@grep -E '^[a-z-]+:.*?## ' $(MAKEFILE_LIST) | awk 'BEGIN {FS = ":.*?## "} {printf "  %-10s %s\n", $$1, $$2}'

deps: ## Start Postgres + Azurite
	docker compose up -d --wait postgres azurite

api: ## Run the API on :8080 (reads .env)
	cargo run -p clipos-api

worker: ## Run the worker (health on :8081)
	cargo run -p clipos-worker

web: ## Run the Vite dev server on :5173 (proxies /api to :8080)
	pnpm --dir web dev

test: ## Run all tests (needs `make deps` and ffmpeg; ORT_DYLIB_PATH for the killfeed models)
	CLIPOS_AZURITE=1 cargo test --workspace
	pnpm --dir web test
	node --test infra/auth0/actions/*.test.mjs
	pnpm --dir web e2e

coverage: ## Line coverage of the Rust and web tests; fails below 95 %, as CI does
	CLIPOS_AZURITE=1 cargo llvm-cov --workspace --summary-only --fail-under-lines 95
	pnpm --dir web coverage

lint: ## Everything CI checks, minus tests
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings
	pnpm --dir web lint
	pnpm --dir web typecheck
	tofu fmt -check -recursive infra

fmt: ## Format Rust, TypeScript and OpenTofu
	cargo fmt --all
	pnpm --dir web format
	tofu fmt -recursive infra

gen-api: ## Regenerate web/openapi.json and the TypeScript API types
	cargo run -q -p clipos-api --bin openapi > web/openapi.json
	pnpm --dir web gen:api

images: ## Build the api and worker container images
	docker compose --profile app build
