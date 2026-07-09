CARGO ?= cargo
CARGO_FEATURES ?=
CARGO_FLAGS := $(if $(strip $(CARGO_FEATURES)),--features "$(CARGO_FEATURES)",)
ARGS ?=

.PHONY: help check-tools fmt check test build debug run clean release-check

help:
	@printf '%s\n' \
		'FrameForge targets:' \
		'  make check-tools      Verify required local tools are available' \
		'  make fmt              Format the Rust workspace' \
		'  make check            Type-check the Rust workspace' \
		'  make test             Run Rust tests' \
		'  make build            Build release CLI and copy it to ./ff' \
		'  make debug            Build the debug workspace artifacts' \
		'  make run ARGS="..."   Run the ff CLI' \
		'  make release-check    Run the default local quality gate' \
		'  make clean            Remove Cargo build outputs' \
		'' \
		'Optional build-time selection:' \
		'  make build CARGO_FEATURES="codec-av2 filter-scale"'

check-tools:
	@command -v $(CARGO) >/dev/null || { echo 'error: cargo not found'; exit 1; }
	@$(CARGO) --version

fmt:
	$(CARGO) fmt --all

check:
	$(CARGO) check --workspace $(CARGO_FLAGS)

test:
	$(CARGO) test --workspace $(CARGO_FLAGS)

build:
	$(CARGO) build --release -p frameforge-cli $(CARGO_FLAGS)
	cp target/release/ff ./ff
	chmod 755 ./ff

debug:
	$(CARGO) build --workspace $(CARGO_FLAGS)

run:
	$(CARGO) run -p frameforge-cli $(CARGO_FLAGS) -- $(ARGS)

release-check: check-tools fmt check test build

clean:
	$(CARGO) clean
	rm -f ./ff
