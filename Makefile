CARGO ?= cargo
PYTHON ?= python3
PRODUCT_FEATURES ?= codec-av2 codec-vvc filter-pattern filter-identity filter-crop filter-scale
CARGO_FEATURES ?= all
AV2_SB_BITS ?= 0
AV2_LOSSY_STATS ?= 0
VVC_STATS ?= 0
AV2_SB_BITS_FEATURE := $(if $(filter 1 true yes,$(AV2_SB_BITS)),frameforge-codecs/av2-sb-bit-profile,)
AV2_LOSSY_STATS_FEATURE := $(if $(filter 1 true yes,$(AV2_LOSSY_STATS)),frameforge-codecs/av2-lossy-stats,)
VVC_STATS_FEATURE := $(if $(filter 1 true yes,$(VVC_STATS)),frameforge-codecs/vvc-stats,)
AV2_ANALYSIS_FEATURES := $(strip $(AV2_SB_BITS_FEATURE) $(AV2_LOSSY_STATS_FEATURE))
VVC_ANALYSIS_FEATURES := $(strip $(VVC_STATS_FEATURE))
CARGO_BASE_FEATURES := $(if $(filter all,$(strip $(CARGO_FEATURES))),$(PRODUCT_FEATURES),$(strip $(CARGO_FEATURES)))
CARGO_FLAGS := $(if $(strip $(CARGO_BASE_FEATURES)),--features "$(CARGO_BASE_FEATURES)",) $(if $(strip $(AV2_ANALYSIS_FEATURES)),--features "$(AV2_ANALYSIS_FEATURES)",) $(if $(strip $(VVC_ANALYSIS_FEATURES)),--features "$(VVC_ANALYSIS_FEATURES)",)
PROFILE ?=
GPROF_RUSTFLAGS ?= -C debuginfo=2 -C force-frame-pointers=yes -C symbol-mangling-version=v0 -C codegen-units=1 -C lto=no -C link-arg=-pg
GPROF_TARGET_DIR ?= target/gprof
GPROF_SAMPLE_RUNS ?= 200
GPROF_PROFILE_CODEC ?= av2
GPROF_PROFILE_NAME ?= scenecomposition_1_420_i_lossless_1f
GPROF_PROFILE_INPUT ?= /media/gabriel/storage/YUV/aomctc/b2_scc/SceneComposition_1.y4m
GPROF_PROFILE_FRAMES ?= 1
GPROF_PROFILE_SETTINGS ?= lossless
GPROF_PROFILE_OUT_DIR ?= verification/generated/profiling
GPROF_PROFILE_SAMPLE_DIR ?= $(GPROF_PROFILE_OUT_DIR)/$(GPROF_PROFILE_NAME)_samples
GPROF_PROFILE_OUTPUT ?= $(GPROF_PROFILE_OUT_DIR)/$(GPROF_PROFILE_NAME).obu
GPROF_PROFILE_RECON ?= $(GPROF_PROFILE_OUT_DIR)/$(GPROF_PROFILE_NAME)_recon.yuv
GPROF_PROFILE_REPORT ?= $(GPROF_PROFILE_OUT_DIR)/$(GPROF_PROFILE_NAME)_$(GPROF_SAMPLE_RUNS)x.gprof.txt
GPROF_PROFILE_RUN_LOG ?= $(GPROF_PROFILE_OUT_DIR)/$(GPROF_PROFILE_NAME).last-run.log
BUILD_TARGET_DIR := target
BUILD_BINARY := ./ff
BUILD_ENV :=
ifeq ($(strip $(PROFILE)),gprof)
BUILD_TARGET_DIR := $(GPROF_TARGET_DIR)
BUILD_BINARY := ./ff-gprof
BUILD_ENV := RUSTFLAGS="$(GPROF_RUSTFLAGS)" CARGO_TARGET_DIR="$(GPROF_TARGET_DIR)"
else ifneq ($(strip $(PROFILE)),)
$(error unsupported PROFILE '$(PROFILE)'; expected PROFILE=gprof)
endif
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
VALIDATION_SETTINGS ?=
COMPRESSION_SET ?= $(VALIDATION_SET)
COMPRESSION_OUT_DIR ?= verification/generated/compression_compare
COMPRESSION_LOG_DIR ?= verification/generated/compression_compare_logs
COMPRESSION_LIMIT ?=
COMPRESSION_REFERENCE_BACKEND ?= reference
COMPRESSION_REFERENCE_PRESET ?= fast
COMPRESSION_REFERENCE_THREADS ?= auto
COMPRESSION_REFERENCE_ARGS ?=
COMPRESSION_SETTINGS ?=
COMPRESSION_QP ?=
COMPRESSION_AVM_TILE_COLUMNS ?= auto
COMPRESSION_AVM_TILE_ROWS ?= 0
COMPRESSION_REFRESH_REFERENCE ?= 0
COMPRESSION_DIRECT_SOURCE_FILES ?= 0
ENCODE_MATRIX_SET ?= local-aomctc-b2-scc-1080p-lossless-50f
ENCODE_MATRIX_OUT_DIR ?= verification/generated/encode_matrix
ENCODE_MATRIX_RUN ?=
ENCODE_MATRIX_CODECS ?=
ENCODE_MATRIX_MODES ?=
ENCODE_MATRIX_BASELINE ?=
ENCODE_MATRIX_LIMIT ?=
ENCODE_MATRIX_AV2_LOSSY_QP ?= 24
ENCODE_MATRIX_AV2_PREDICTIVE ?= 1
ENCODE_MATRIX_DIRECT_SOURCE_FILES ?= 1
GEOMETRY_SWEEP_SETS ?= screenshot-sweep-444 screenshot-sweep-444-10bit screenshot-sweep-420-10bit-canary
GEOMETRY_SWEEP_CODECS ?= av2 vvc
GEOMETRY_SWEEP_MODES ?= lossless lossy
GEOMETRY_SWEEP_REFERENCE_MODE ?= off
GEOMETRY_SWEEP_AV2_LOSSY_QP ?= 24
GEOMETRY_SWEEP_AV2_SETTINGS ?= predictive
LIBAOM_SB_BITS ?= 0
LIBAOM_SB_BITS_BUILD_DIR ?= verification/references/libaom/libaom/build-sb-bits
LIBAOM_SB_BITS_ENCODER ?= $(LIBAOM_SB_BITS_BUILD_DIR)/aomenc
LIBAOM_SB_BITS_REFERENCE_ENV := $(if $(filter 1 true yes,$(LIBAOM_SB_BITS)),FRAMEFORGE_LIBAOM_SB_BITS_BUILD=1 FRAMEFORGE_LIBAOM_BUILD_DIR="$(abspath $(LIBAOM_SB_BITS_BUILD_DIR))" FRAMEFORGE_LIBAOM_ENCODER="$(abspath $(LIBAOM_SB_BITS_ENCODER))",)
AVM_SB_BITS ?= 0
AVM_SB_BITS_BUILD_DIR ?= verification/references/av2/avm/build-sb-bits
AVM_SB_BITS_ENCODER ?= $(AVM_SB_BITS_BUILD_DIR)/avmenc
AVM_SB_BITS_REFERENCE_ENV := $(if $(filter 1 true yes,$(AVM_SB_BITS)),FRAMEFORGE_AVM_SB_BITS_BUILD=1 FRAMEFORGE_AVM_BUILD_DIR="$(abspath $(AVM_SB_BITS_BUILD_DIR))" FRAMEFORGE_AVM_ENCODER="$(abspath $(AVM_SB_BITS_ENCODER))",)
REFERENCE_ENV := $(LIBAOM_SB_BITS_REFERENCE_ENV) $(AVM_SB_BITS_REFERENCE_ENV)
REFERENCE_CODEC ?= all
VALIDATION_STOP_FLAG := $(if $(filter 1 true yes,$(VALIDATION_STOP_ON_FAIL)),--stop-on-fail,)
VALIDATION_LIMIT_FLAG := $(if $(strip $(VALIDATION_LIMIT)),--limit "$(VALIDATION_LIMIT)",)
VALIDATION_SOURCE_FLAG := $(if $(filter 1 true yes,$(VALIDATION_SOURCE_FILTERS)),--source-filters,)
VALIDATION_SETTINGS_FLAG := $(foreach setting,$(VALIDATION_SETTINGS),--setting "$(setting)")
COMPRESSION_LIMIT_FLAG := $(if $(strip $(COMPRESSION_LIMIT)),--limit "$(COMPRESSION_LIMIT)",)
COMPRESSION_REFERENCE_BACKEND_FLAG := --reference-backend "$(COMPRESSION_REFERENCE_BACKEND)"
COMPRESSION_REFERENCE_PRESET_FLAG := --reference-preset "$(COMPRESSION_REFERENCE_PRESET)"
COMPRESSION_REFERENCE_THREADS_FLAG := --reference-threads "$(COMPRESSION_REFERENCE_THREADS)"
COMPRESSION_AVM_TILE_COLUMNS_FLAG := --avm-tile-columns "$(COMPRESSION_AVM_TILE_COLUMNS)"
COMPRESSION_AVM_TILE_ROWS_FLAG := --avm-tile-rows "$(COMPRESSION_AVM_TILE_ROWS)"
COMPRESSION_REFERENCE_ARGS_FLAG := $(if $(strip $(COMPRESSION_REFERENCE_ARGS)),--reference-args "$(COMPRESSION_REFERENCE_ARGS)",)
COMPRESSION_SETTINGS_FLAG := $(foreach setting,$(COMPRESSION_SETTINGS),--setting "$(setting)")
COMPRESSION_QP_FLAG := $(if $(strip $(COMPRESSION_QP)),--qp "$(COMPRESSION_QP)",)
COMPRESSION_REFRESH_REFERENCE_FLAG := $(if $(filter 1 true yes,$(COMPRESSION_REFRESH_REFERENCE)),--refresh-reference,)
COMPRESSION_DIRECT_SOURCE_FILES_FLAG := $(if $(filter 1 true yes,$(COMPRESSION_DIRECT_SOURCE_FILES)),--direct-source-files,)
ENCODE_MATRIX_RUN_FLAG := $(if $(strip $(ENCODE_MATRIX_RUN)),--run-name "$(ENCODE_MATRIX_RUN)",)
ENCODE_MATRIX_CODECS_FLAG := $(foreach codec,$(ENCODE_MATRIX_CODECS),--codec "$(codec)")
ENCODE_MATRIX_MODES_FLAG := $(foreach mode,$(ENCODE_MATRIX_MODES),--mode "$(mode)")
ENCODE_MATRIX_BASELINE_FLAG := $(if $(strip $(ENCODE_MATRIX_BASELINE)),--baseline-json "$(ENCODE_MATRIX_BASELINE)",)
ENCODE_MATRIX_LIMIT_FLAG := $(if $(strip $(ENCODE_MATRIX_LIMIT)),--limit "$(ENCODE_MATRIX_LIMIT)",)
ENCODE_MATRIX_AV2_PREDICTIVE_FLAG := $(if $(filter 1 true yes,$(ENCODE_MATRIX_AV2_PREDICTIVE)),--av2-predictive,--no-av2-predictive)
ENCODE_MATRIX_DIRECT_SOURCE_FILES_FLAG := $(if $(filter 1 true yes,$(ENCODE_MATRIX_DIRECT_SOURCE_FILES)),--direct-source-files,--no-direct-source-files)
GEOMETRY_SWEEP_AV2_SETTINGS_FLAG := $(foreach setting,$(GEOMETRY_SWEEP_AV2_SETTINGS),--setting $(setting))
GPROF_PROFILE_SETTINGS_FLAG := $(foreach setting,$(GPROF_PROFILE_SETTINGS),--set "$(setting)")

.PHONY: help check-tools fmt check test build debug run reference-list reference-setup test-vector-sets test-vectors validate-set compare-compression benchmark-encode-matrix validate-geometry-sweep profile-av2-i-lossless regression clean release-check

help:
	@printf '%s\n' \
		'FrameForge targets:' \
		'  make check-tools      Verify required local tools are available' \
		'  make fmt              Format the Rust workspace' \
		'  make check            Type-check the Rust workspace' \
		'  make test             Run Rust tests' \
		'  make build            Build release CLI and copy it to ./ff' \
		'                         Set AV2_SB_BITS=1 to compile AV2 per-superblock bit JSONL support' \
		'                         Set AV2_LOSSY_STATS=1 to compile AV2 lossy mode/TXB stats' \
		'                         Set VVC_STATS=1 to compile VVC stage timing JSONL support' \
		'  make build PROFILE=gprof' \
		'                         Build gprof sampling-friendly ./ff-gprof under target/gprof' \
		'  make profile-av2-i-lossless' \
		'                         Aggregate gprof samples for the first lossless AV2 I-frame' \
		'                         Override GPROF_SAMPLE_RUNS, GPROF_PROFILE_INPUT, or GPROF_PROFILE_SETTINGS' \
		'  make debug            Build the debug workspace artifacts' \
		'  make run ARGS="..."   Run the ff CLI' \
		'  make reference-list   List declared external reference tools' \
		'  make reference-setup  Clone/build declared references, REFERENCE_CODEC=all' \
		'  make test-vector-sets List generated-vector manifests' \
		'  make test-vectors     Generate TEST_VECTOR_SET=smoke vectors' \
		'  make validate-set     Encode VALIDATION_SET=smoke with CODEC=av2' \
		'                         Add VALIDATION_SOURCE_FILTERS=1 to skip input files' \
		'                         Use VALIDATION_REFERENCE_MODE=auto|required|off' \
		'                         Pass extra --set values with VALIDATION_SETTINGS="key ..."' \
		'  make compare-compression' \
		'                         Compare FrameForge and reference encoder sizes' \
		'                         Uses CODEC=av2 COMPRESSION_SET=$(VALIDATION_SET)' \
		'                         Set COMPRESSION_REFERENCE_BACKEND=rav1e for lossy AV1 baseline' \
		'                         Set COMPRESSION_REFERENCE_BACKEND=ffmpeg-libaom for AV1 libaom baseline' \
		'                         Uses COMPRESSION_REFERENCE_PRESET=fast by default' \
		'                         Set COMPRESSION_REFERENCE_PRESET=realtime-screen for libaom screen-share settings' \
		'                         Set COMPRESSION_REFERENCE_PRESET=default for legacy args' \
		'                         Pass FrameForge --set values with COMPRESSION_SETTINGS="key ..."' \
		'                         Set COMPRESSION_QP=24 for AV2 lossy QP comparisons' \
		'                         Set COMPRESSION_REFERENCE_BACKEND=libaom for direct aomenc' \
		'                         Set LIBAOM_SB_BITS=1 for instrumented direct libaom builds' \
		'                         Set AVM_SB_BITS=1 for instrumented AVM reference builds' \
		'                         Set COMPRESSION_DIRECT_SOURCE_FILES=1 to feed source_file inputs directly' \
		'                         Set COMPRESSION_REFRESH_REFERENCE=1 to ignore cache' \
		'  make benchmark-encode-matrix' \
		'                         Time AV2/VVC lossy/lossless encodes over ENCODE_MATRIX_SET' \
		'  make validate-geometry-sweep' \
		'                         Run small geometry sweeps for AV2/VVC lossy/lossless modes' \
		'  make regression       Run smoke validation for AV2 and VVC' \
		'  make release-check    Run the default local quality gate' \
		'  make clean            Remove Cargo build outputs' \
		'' \
		'Optional build-time selection:' \
		'  make build CARGO_FEATURES=all    Build all normal product stages' \
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
	$(BUILD_ENV) $(CARGO) build --release -p frameforge-cli $(CARGO_FLAGS)
	cp $(BUILD_TARGET_DIR)/release/ff $(BUILD_BINARY)
	chmod 755 $(BUILD_BINARY)

debug:
	$(CARGO) build --workspace $(CARGO_FLAGS)

run:
	$(CARGO) run -p frameforge-cli $(CARGO_FLAGS) -- $(ARGS)

reference-list:
	$(PYTHON) scripts/reference_tools.py list --codec "$(REFERENCE_CODEC)"

reference-setup:
	$(REFERENCE_ENV) $(PYTHON) scripts/reference_tools.py setup --codec "$(REFERENCE_CODEC)"

test-vector-sets:
	$(PYTHON) scripts/generate_test_vectors.py --set-dir "$(VALIDATION_SET_DIR)" --list-sets

test-vectors:
	$(PYTHON) scripts/generate_test_vectors.py "$(TEST_VECTOR_SET)" --set-dir "$(VALIDATION_SET_DIR)" --out-dir "$(VALIDATION_OUT_DIR)"

validate-set: build
	$(PYTHON) scripts/run_validation_set.py --ff "$(abspath $(BUILD_BINARY))" --codec "$(CODEC)" "$(VALIDATION_SET)" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" $(VALIDATION_SOURCE_FLAG) $(VALIDATION_STOP_FLAG) $(VALIDATION_LIMIT_FLAG) $(VALIDATION_SETTINGS_FLAG)

compare-compression: build
	$(REFERENCE_ENV) $(PYTHON) scripts/compare_reference_compression.py --ff "$(abspath $(BUILD_BINARY))" --codec "$(CODEC)" "$(COMPRESSION_SET)" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --out-dir "$(COMPRESSION_OUT_DIR)" --log-dir "$(COMPRESSION_LOG_DIR)" $(COMPRESSION_LIMIT_FLAG) $(COMPRESSION_REFERENCE_BACKEND_FLAG) $(COMPRESSION_REFERENCE_PRESET_FLAG) $(COMPRESSION_REFERENCE_THREADS_FLAG) $(COMPRESSION_AVM_TILE_COLUMNS_FLAG) $(COMPRESSION_AVM_TILE_ROWS_FLAG) $(COMPRESSION_REFERENCE_ARGS_FLAG) $(COMPRESSION_SETTINGS_FLAG) $(COMPRESSION_QP_FLAG) $(COMPRESSION_REFRESH_REFERENCE_FLAG) $(COMPRESSION_DIRECT_SOURCE_FILES_FLAG)

benchmark-encode-matrix: build
	$(PYTHON) scripts/benchmark_encode_matrix.py "$(ENCODE_MATRIX_SET)" --ff "$(abspath $(BUILD_BINARY))" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --out-dir "$(ENCODE_MATRIX_OUT_DIR)" --av2-lossy-qp "$(ENCODE_MATRIX_AV2_LOSSY_QP)" $(ENCODE_MATRIX_RUN_FLAG) $(ENCODE_MATRIX_CODECS_FLAG) $(ENCODE_MATRIX_MODES_FLAG) $(ENCODE_MATRIX_BASELINE_FLAG) $(ENCODE_MATRIX_LIMIT_FLAG) $(ENCODE_MATRIX_AV2_PREDICTIVE_FLAG) $(ENCODE_MATRIX_DIRECT_SOURCE_FILES_FLAG)

validate-geometry-sweep: build
	for codec in $(GEOMETRY_SWEEP_CODECS); do \
		for mode in $(GEOMETRY_SWEEP_MODES); do \
			for set in $(GEOMETRY_SWEEP_SETS); do \
				extra=""; \
				settings=""; \
				if [ "$$codec" = "av2" ]; then settings='$(GEOMETRY_SWEEP_AV2_SETTINGS_FLAG)'; fi; \
				if [ "$$mode" = "lossy" ]; then extra="--force-lossy"; fi; \
				if [ "$$codec" = "av2" ] && [ "$$mode" = "lossy" ]; then extra="$$extra --qp $(GEOMETRY_SWEEP_AV2_LOSSY_QP)"; fi; \
				$(PYTHON) scripts/run_validation_set.py --ff "$(abspath $(BUILD_BINARY))" --codec "$$codec" "$$set" --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(GEOMETRY_SWEEP_REFERENCE_MODE)" --stop-on-fail $$settings $$extra; \
			done; \
		done; \
	done

profile-av2-i-lossless:
	$(MAKE) build PROFILE=gprof
	mkdir -p "$(GPROF_PROFILE_SAMPLE_DIR)"
	rm -f "$(GPROF_PROFILE_SAMPLE_DIR)"/gmon.* "$(GPROF_PROFILE_REPORT)" "$(GPROF_PROFILE_OUTPUT)" "$(GPROF_PROFILE_RECON)" "$(GPROF_PROFILE_RUN_LOG)"
	for i in $$(seq 1 $(GPROF_SAMPLE_RUNS)); do \
		if ! GMON_OUT_PREFIX="$(GPROF_PROFILE_SAMPLE_DIR)/gmon" ./ff-gprof encode "$(GPROF_PROFILE_INPUT)" --frames "$(GPROF_PROFILE_FRAMES)" --encode "$(GPROF_PROFILE_CODEC):$(GPROF_PROFILE_OUTPUT)" --recon "$(GPROF_PROFILE_RECON)" $(GPROF_PROFILE_SETTINGS_FLAG) >"$(GPROF_PROFILE_RUN_LOG)" 2>&1; then \
			cat "$(GPROF_PROFILE_RUN_LOG)"; \
			exit 1; \
		fi; \
	done
	gprof -b ./ff-gprof "$(GPROF_PROFILE_SAMPLE_DIR)"/gmon.* > "$(GPROF_PROFILE_REPORT)"
	@printf 'wrote %s from %s first-frame run(s)\n' "$(GPROF_PROFILE_REPORT)" "$(GPROF_SAMPLE_RUNS)"
	@head -40 "$(GPROF_PROFILE_REPORT)"

regression: build
	$(PYTHON) scripts/run_validation_set.py --ff "$(abspath $(BUILD_BINARY))" --codec av2 smoke --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" --stop-on-fail
	$(PYTHON) scripts/run_validation_set.py --ff "$(abspath $(BUILD_BINARY))" --codec vvc smoke --set-dir "$(VALIDATION_SET_DIR)" --vector-dir "$(VALIDATION_OUT_DIR)" --encoded-dir "$(VALIDATION_ENCODED_DIR)" --log-dir "$(VALIDATION_LOG_DIR)" --reference-mode "$(VALIDATION_REFERENCE_MODE)" --stop-on-fail

release-check: check-tools fmt check test build

clean:
	$(CARGO) clean
	rm -f ./ff ./ff-gprof gmon.out gprof.txt
