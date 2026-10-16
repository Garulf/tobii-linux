# Rust core and its Python extension. The extension is copied into the source tree so
# PYTHONPATH=src runs (tests, oracles, tools) import it without installing the package.

CARGO ?= cargo
PYTHON ?= python3
CARGO_TARGET_DIR ?= $(CURDIR)/target
export CARGO_TARGET_DIR
NATIVE := src/tobii_linux/_native.abi3.so
PYTEST_PATH := .local/inference:src
ORACLE_PATH := .local/python:.local/inference:src
RUNTIME := evidence/private/binaries/platform_runtime_IS5LEYETRACKER5_service.exe
ORACLES := ttp_commands ttp_hwinfo ttp_reply gaze presence gaze_calibration gaze_device display_setup
BINARY_ORACLES := usb_framing ttp_encoder

.PHONY: native native-debug rust-test rust-lint test oracles check clean-native

native:
	$(CARGO) build --release -p tobii-py
	install -m 755 $(CARGO_TARGET_DIR)/release/lib_native.so $(NATIVE)

native-debug:
	$(CARGO) build -p tobii-py
	install -m 755 $(CARGO_TARGET_DIR)/debug/lib_native.so $(NATIVE)

rust-test:
	$(CARGO) test --workspace

rust-lint:
	$(CARGO) fmt --all --check
	$(CARGO) clippy --workspace --all-targets -- -D warnings

test: native
	PYTHONPATH=$(PYTEST_PATH) OPENBLAS_NUM_THREADS=2 $(PYTHON) -m unittest discover -s tests

oracles: native
	@for oracle in $(ORACLES); do \
		echo "== $$oracle"; PYTHONPATH=$(ORACLE_PATH) $(PYTHON) tools/$${oracle}_oracle.py | tail -n 1 || exit 1; \
	done
	@for oracle in $(BINARY_ORACLES); do \
		echo "== $$oracle"; PYTHONPATH=$(ORACLE_PATH) $(PYTHON) tools/$${oracle}_oracle.py $(RUNTIME) > /dev/null || exit 1; \
	done

check: rust-lint rust-test test

clean-native:
	rm -f $(NATIVE)
