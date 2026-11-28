.PHONY: ci ci-checks fmt lint test build install bench-release bench-calibrate bench-memory
# `ci` holds one of the host-wide CI slots (scripts/ci_slot.sh, CI_HOST_SLOTS,
# default 1) while scripts/ci.sh runs; `ci-checks` runs it without one.
ci:      ; sh scripts/ci_slot.sh ./scripts/ci.sh
ci-checks: ; ./scripts/ci.sh
fmt:     ; cargo fmt --all
lint:    ; cargo clippy --workspace --all-targets -- -D warnings
test:    ; cargo test --workspace
build:   ; cargo build --release -p aether
install: ; cargo install --path crates/aether --locked

# Release benchmark (docs/release-bench.md): run it only before a release, never in CI.
# OLD is the last release's aether binary; NEW defaults to this tree's release build.
# ALLOW_OFF_TARGET=1 runs a set whose calibration missed its targets; its verdict is
# then at best INCONCLUSIVE.
PYTHON   ?= python3
NEW      ?= $(or $(CARGO_TARGET_DIR),target)/release/aether
SET      ?= bench/release-set/v2
# ARCHON_OLD / ARCHON_NEW (ARCHON_BIN is ARCHON_NEW's old name) add the archon-old / archon-new arms,
# each over its own side's aether, so one run gates both an Aether and an Archon release.
# JOBS and LOAD_MAX default to the host's size (release_bench_host.py).
ARMS     ?= all
BUDGET   ?= 20
REPEATS  ?= 1
COUNT    ?= 200
ARCHON_NEW ?= $(ARCHON_BIN)
POOL_FLAGS = $(if $(JOBS),--jobs $(JOBS)) $(if $(LOAD_MAX),--load-max $(LOAD_MAX))
BENCH_FLAGS = $(POOL_FLAGS) --budget $(BUDGET) --repeats $(REPEATS) \
	$(if $(ALLOW_OFF_TARGET),--allow-off-target) $(if $(EARLY_STOP),--early-stop)
ARCHON_FLAGS = $(if $(ARCHON_OLD),--archon-old "$(ARCHON_OLD)") $(if $(ARCHON_NEW),--archon-new "$(ARCHON_NEW)") \
	$(if $(ARCHON_PIPELINE),--archon-pipeline $(ARCHON_PIPELINE)) $(if $(ARCHON_REVIEW),--archon-review $(ARCHON_REVIEW))

bench-release:
	$(if $(OLD),,$(error set OLD to the previous release's aether binary))
	$(PYTHON) evals/release_bench.py run --set "$(SET)" --old "$(OLD)" --new "$(NEW)" --arms "$(ARMS)" \
		$(BENCH_FLAGS) $(ARCHON_FLAGS) $(EXTRA)

bench-calibrate:
	$(if $(FREEZE),,$(error set FREEZE to the new set's directory, such as bench/release-set/v3))
	$(PYTHON) evals/release_bench.py calibrate --count $(COUNT) --price-binary "$(NEW)" --freeze "$(FREEZE)" \
		$(POOL_FLAGS) --budget $(or $(CAL_BUDGET),25) --repeats $(or $(CAL_REPEATS),3) \
		$(if $(TEMPLATES),--templates "$(TEMPLATES)") $(if $(ACCEPT_OFF_TARGET),--accept-off-target) $(EXTRA)

# Memory benchmark (docs/memory/BASELINE.md): deterministic, no model calls. Fails
# when the deterministic metrics differ from bench/memory-set/v1/baseline.json.
bench-memory:
	$(PYTHON) evals/memory_bench.py run --check $(EXTRA)
