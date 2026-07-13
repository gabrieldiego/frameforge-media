CARGO ?= cargo
PYTHON ?= python3
CARGO_FEATURES ?= all
CARGO_FLAGS := $(if $(filter all,$(strip $(CARGO_FEATURES))),--all-features,$(if $(strip $(CARGO_FEATURES)),--features "$(CARGO_FEATURES)",))
ARGS ?=
CODEC ?= av2
TEST_VECTOR_SET ?= smoke
VALIDATION_SET ?= $(TEST_VECTOR_SET)
VALIDATION_STOP_ON_FAIL ?= 1
VALIDATION_LIMIT ?=
VALIDATION_SET_DIR ?= verification/test_vector_sets
VALIDATION_OUT_DIR ?= verification/generated/test_vectors
VALIDATION_ENCODED_DIR ?= verification/generated/encoded
VALIDATION_LOG_DIR ?= verification/generated/validation_logs
VALIDATION_SOURCE_FILTERS ?= 0
VALIDATION_REFERENCE_MODE ?= auto
COMPRESSION_SET ?= $(VALIDATION_SET)
COMPRESSION_OUT_DIR ?= verification/generated/compression_compare
COMPRESSION_LOG_DIR ?= verification/generated/compression_compare_logs
COMPRESSION_LIMIT ?=
COMPRESSION_REFERENCE_BACKEND ?= reference
COMPRESSION_REFERENCE_PRESET ?= fast
COMPRESSION_REFERENCE_THREADS ?= auto
COMPRESSION_REFERENCE_ARGS ?=
COMPRESSION_AVM_TILE_COLUMNS ?= auto
COMPRESSION_AVM_TILE_ROWS ?= 0
COMPRESSION_REFRESH_REFERENCE ?= 0
COMPRESSION_DIRECT_SOURCE_FILES ?= 0
REFERENCE_CODEC ?= all
VALIDATION_STOP_FLAG := $(if $(filter 1 true yes,$(VALIDATION_STOP_ON_FAIL)),--stop-on-fail,)
VALIDATION_LIMIT_FLAG := $(if $(strip $(VALIDATION_LIMIT)),--limit "$(VALIDATION_LIMIT)",)
VALIDATION_SOURCE_FLAG := $(if $(filter 1 true yes,$(VALIDATION_SOURCE_FILTERS)),--source-filters,)
COMPRESSION_LIMIT_FLAG := $(if $(strip $(COMPRESSION_LIMIT)),--limit "$(COMPRESSION_LIMIT)",)
COMPRESSION_REFERENCE_BACKEND_FLAG := --reference-backend "$(COMPRESSION_REFERENCE_BACKEND)"
COMPRESSION_REFERENCE_PRESET_FLAG := --reference-preset "$(COMPRESSION_REFERENCE_PRESET)"
COMPRESSION_REFERENCE_THREADS_FLAG := --reference-threads "$(COMPRESSION_REFERENCE_THREADS)"
COMPRESSION_AVM_TILE_COLUMNS_FLAG := --avm-tile-columns "$(COMPRESSION_AVM_TILE_COLUMNS)"
COMPRESSION_AVM_TILE_ROWS_FLAG := --avm-tile-rows "$(COMPRESSION_AVM_TILE_ROWS)"
COMPRESSION_REFERENCE_ARGS_FLAG := $(if $(strip $(COMPRESSION_REFERENCE_ARGS)),--reference-args "$(COMPRESSION_REFERENCE_ARGS)",)
COMPRESSION_REFRESH_REFERENCE_FLAG := $(if $(filter 1 true yes,$(COMPRESSION_REFRESH_REFERENCE)),--refresh-reference,)
COMPRESSION_DIRECT_SOURCE_FILES_FLAG := $(if $(filter 1 true yes,$(COMPRESSION_DIRECT_SOURCE_FILES)),--direct-source-files,)

.PHONY: help check-tools fmt check test build debug run reference-list reference-setup test-vector-sets test-vectors validate-set compare-compression regression clean release-check

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
		'  make reference-list   List declared external reference tools' \
		'  make reference-setup  Clone/build declared references, REFERENCE_CODEC=all' \
		'  make test-vector-sets List generated-vector manifests' \
		'  make test-vectors     Generate TEST_VECTOR_SET=smoke vectors' \
		'  make validate-set     Encode VALIDATION_SET=smoke with CODEC=av2' \
		'                         Add VALIDATION_SOURCE_FILTERS=1 to skip input files' \
		'                         Use VALIDATION_REFERENCE_MODE=auto|required|off' \
		'  make compare-compression' \
		'                         Compare FrameForge and reference encoder sizes' \
		'                         Uses CODEC=av2 COMPRESSION_SET=$(VALIDATION_SET)' \
		'                         Set COMPRESSION_REFERENCE_BACKEND=rav1e for lossy AV1 baseline' \
		'                         Set COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom for AV1 libaom baseline' \
		'                         Uses COMPRESSION_REFERENCE_PRESET=fast by default' \
		'                         Set COMPRESSION_REFERENCE_PRESET=realtime-screen for libaom screen-share settings' \
		'                         Set COMPRESSION_REFERENCE_PRESET=default for legacy args' \
		'                         Set COMPRESSION_DIRECT_SOURCE_FILES=1 to feed source_file inputs directly' \
		'                         Set COMPRESSION_REFRESH_REFERENCE=1 to ignore cache' \
		'  make regression       Run smoke validation for AV2 and VVC' \
		'  make release-check    Run the default local quality gate' \
		'  make clean            Remove Cargo build outputs' \
		'' \
		'Optional build-time selection:' \
		'  make build CARGO_FEATURES=all    Build all optional stages' \
		'  make build CARGO_FEATURES="codec-av2 filter-scale"' \
		'  make build CARGO_FEATURES=        Build without optional stages'

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

reference-list:
	$(PYTHON) scripts/reference_tools.py list --codec "$(REFERENCE_CODEC)"

reference-setup:
	$(PYTHON) scripts/reference_tools.py setup --codec "$(REFERENCE_CODEC)"

test-vector-sets:
	$(PYTHON) scripts/generate_test_vectors.py --set-dir "$(VALIDATION_SET_DIR)" --list-sets

test-vectors:
	$(PYTHON) scripts/generate_test_vectors.py "$(TEST_VECTOR_SET)" --set-dir "$(VALIDATION_SET_DIR)" --out-dir "$(VALIDATION_OUT_DIR)"

validate-set: build
	$(PYTHON) scripts/run_validation_set.py --codec "$(CODEC)" "$(VALIDATION_SET)" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" $(VALIDATION_SOURCE_FLAG) $(VALIDATION_STOP_FLAG) $(VALIDATION_LIMIT_FLAG)

compare-compression: build
	$(PYTHON) scripts/compare_reference_compression.py --codec "$(CODEC)" "$(COMPRESSION_SET)" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --out-dir "$(COMPRESSION_OUT_DIR)" --log-dir "$(COMPRESSION_LOG_DIR)" $(COMPRESSION_LIMIT_FLAG) $(COMPRESSION_REFERENCE_BACKEND_FLAG) $(COMPRESSION_REFERENCE_PRESET_FLAG) $(COMPRESSION_REFERENCE_THREADS_FLAG) $(COMPRESSION_AVM_TILE_COLUMNS_FLAG) $(COMPRESSION_AVM_TILE_ROWS_FLAG) $(COMPRESSION_REFERENCE_ARGS_FLAG) $(COMPRESSION_REFRESH_REFERENCE_FLAG) $(COMPRESSION_DIRECT_SOURCE_FILES_FLAG)

regression: build
	$(PYTHON) scripts/run_validation_set.py --codec av2 smoke --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" --stop-on-fail
	$(PYTHON) scripts/run_validation_set.py --codec vvc smoke --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" --stop-on-fail

release-check: check-tools fmt check test build

clean:
	$(CARGO) clean
	rm -f ./ff
