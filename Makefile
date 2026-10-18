.PHONY: ci fmt lint test build install bench-release bench-calibrate
ci:      ; ./scripts/ci.sh
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
ARMS     ?= all
JOBS     ?= 6
LOAD_MAX ?= 16
BUDGET   ?= 20
REPEATS  ?= 1
COUNT    ?= 200
BENCH_FLAGS = --jobs $(JOBS) --load-max $(LOAD_MAX) --budget $(BUDGET) --repeats $(REPEATS) \
	$(if $(ALLOW_OFF_TARGET),--allow-off-target) $(if $(EARLY_STOP),--early-stop)

bench-release:
	$(if $(OLD),,$(error set OLD to the previous release's aether binary))
	$(PYTHON) evals/release_bench.py run --set "$(SET)" --old "$(OLD)" --new "$(NEW)" --arms "$(ARMS)" \
		$(BENCH_FLAGS) $(if $(ARCHON_BIN),--archon-bin "$(ARCHON_BIN)") $(EXTRA)

bench-calibrate:
	$(if $(FREEZE),,$(error set FREEZE to the new set's directory, such as bench/release-set/v3))
	$(PYTHON) evals/release_bench.py calibrate --count $(COUNT) --price-binary "$(NEW)" --freeze "$(FREEZE)" \
		--jobs $(JOBS) --load-max $(LOAD_MAX) --budget $(or $(CAL_BUDGET),25) --repeats $(or $(CAL_REPEATS),3) \
		$(if $(TEMPLATES),--templates "$(TEMPLATES)") $(if $(ACCEPT_OFF_TARGET),--accept-off-target) $(EXTRA)
