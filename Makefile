.PHONY: test test-api run build release lint fmt clean docker-build docker-run

# ═══ Development ═══

build:
	cargo build

release:
	cargo build --release

run:
	cargo run

test:
	cargo test

test-api:
	cargo run &
	sleep 3
	python3 test_api.py
	pkill banking-ledger || true

lint:
	cargo clippy -- -D warnings

fmt:
	cargo fmt --check

clean:
	cargo clean

# ═══ CI/CD ═══

ci: lint fmt test test-api
	@echo "✅ All CI checks passed"

# ═══ Docker ═══

docker-build:
	docker build -t banking-ledger:latest .

docker-run:
	docker run -p 3001:3001 --rm banking-ledger:latest

# ═══ Benchmarks ═══

bench:
	cargo bench 2>/dev/null || cargo test --release -- --nocapture

# ═══ Docs ═══

docs:
	cargo doc --no-deps --open
