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
GAME_DLL := evidence/private/binaries/tobii_gameintegration_x64.dll
ORACLES := ttp_commands ttp_hwinfo ttp_reply gaze presence gaze_calibration gaze_device display_setup sesp
ORACLES += lbf_coeff lbf_tree d1_records pose_mesh
ORACLES += d1_input d1_geometry d1_transform d1_warp d1_crop d1_quality face_detect
ORACLES += pose_inverse pose_projection pose_jacobian pose_euler pose_geometry pose_filter pose_initialization pose_observations pose_fit pose_deformation_jacobian pose_deformation_fit pose_bounds pose_selection pose_configured pose_modes pose_policy pose_caller pose_temporal lbf_gather lbf_predictor eye_refine landmark_smoothing
ORACLES += face_tracking
BINARY_ORACLES := usb_framing ttp_encoder
GAME_ORACLES := prp_encoder prp_session

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
		echo "== $$oracle"; \
		output=$$(PYTHONPATH=$(ORACLE_PATH) $(PYTHON) tools/$${oracle}_oracle.py) || { echo "$$output"; exit 1; }; \
		echo "$$output" | tail -n 1; \
	done
	@for oracle in $(BINARY_ORACLES); do \
		echo "== $$oracle"; PYTHONPATH=$(ORACLE_PATH) $(PYTHON) tools/$${oracle}_oracle.py $(RUNTIME) > /dev/null || exit 1; \
	done
	@for oracle in $(GAME_ORACLES); do \
		echo "== $$oracle"; PYTHONPATH=$(ORACLE_PATH) $(PYTHON) tools/$${oracle}_oracle.py $(GAME_DLL) > /dev/null || exit 1; \
	done

check: rust-lint rust-test test

clean-native:
	rm -f $(NATIVE)

# The GTK 4 setup app (crates/tobii-setup, feature gtk). The default targets above never
# build it, so they work without GTK. Without a host GTK these run in nix/setup-shell.nix,
# through nix-shell or nix-portable, with nixpkgs' Rust toolchain in target/gtk.
NIX_PORTABLE ?= $(HOME)/.local/bin/nix-portable
NIXPKGS ?= $(HOME)/np-src/nixpkgs
SETUP_ARGS ?= --simulate unmapped
ifeq ($(shell pkg-config --exists gtk4 2>/dev/null && echo yes),yes)
GTK_TARGET_DIR ?= $(CARGO_TARGET_DIR)
GTK_RUN ?= sh -c
else ifneq ($(shell command -v nix-shell 2>/dev/null),)
GTK_TARGET_DIR ?= $(CARGO_TARGET_DIR)/gtk
GTK_RUN ?= nix-shell $(CURDIR)/nix/setup-shell.nix --run
else
GTK_TARGET_DIR ?= $(CARGO_TARGET_DIR)/gtk
GTK_RUN ?= mkdir -p $(CURDIR)/build/nix-tmp && TMPDIR=$(CURDIR)/build/nix-tmp $(NIX_PORTABLE) nix-shell \
	-I nixpkgs=$(NIXPKGS) $(CURDIR)/nix/setup-shell.nix --run
endif
GTK_CARGO := CARGO_TARGET_DIR=$(GTK_TARGET_DIR) cargo

.PHONY: setup-app setup-test setup-run

setup-app:
	$(GTK_RUN) '$(GTK_CARGO) build --release -p tobii-setup --features gtk'
	@echo $(GTK_TARGET_DIR)/release/tobii-linux-setup

setup-test:
	$(GTK_RUN) '$(GTK_CARGO) test -p tobii-setup --features gtk && \
		$(GTK_CARGO) clippy -p tobii-setup --features gtk --all-targets -- -D warnings'

setup-run: setup-app
	$(GTK_RUN) '$(GTK_TARGET_DIR)/release/tobii-linux-setup $(SETUP_ARGS)'
