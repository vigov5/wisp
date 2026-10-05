.PHONY: help check test fmt fmt-check clippy server send send-file send-dir send-files send-nearby send-multiple send-large receive trimui-preview trimui-build trimui-pak trimui-install trimui-probe

SERVER_URL ?= https://rendezvous.wisp.mooo.com
SERVER_ADDR ?= 0.0.0.0:8787
FILE ?= Cargo.toml
DIR ?=
OUT ?= downloads
FILES ?= $(FILE)
TRACE ?= 1

ifneq ($(strip $(SERVER_URL)),)
RENDEZVOUS_ENV := WISP_RENDEZVOUS_URL=$(SERVER_URL)
endif

ifeq ($(TRACE),0)
TRACE_ENV := RUST_LOG=off
else
TRACE_ENV :=
endif

# Synthetic file generation sizes for the send-multiple/send-large targets (overridable).
MULTIPLE_COUNT ?= 5
MULTIPLE_SIZE_MB ?= 100
LARGE_SIZE_MB ?= 1024
NEARBY_SIZE_MB ?= 10

# mDNS scan duration for send-nearby (seconds).
NEARBY_TIMEOUT_SECS ?= 7

# --- TrimUI Brick Pro (TG4040) port ------------------------------------------
# Statically linked against musl so one binary runs on stock Tina Linux as well
# as MinUI/Knulli, with no dependency on the device's libc or on an SDL2 build
# that TrimUI has not published an SDK for.
TRIMUI_TARGET ?= aarch64-unknown-linux-musl
TRIMUI_HOST ?= root@trimui
TRIMUI_APP_DIR ?= /mnt/SDCARD/Apps/Wisp
TRIMUI_PAK ?= target/trimui/Wisp
# Overridable so a binary built elsewhere — an aarch64 Linux box builds it
# natively, with no cross toolchain — can be packaged straight from here.
TRIMUI_BIN ?= target/$(TRIMUI_TARGET)/release/wisp-trimui

help:
	@echo "Wisp Makefile targets"
	@echo ""
	@echo "Rust workflow:"
	@echo "  check           — cargo check"
	@echo "  test            — cargo test"
	@echo "  fmt             — cargo fmt"
	@echo "  fmt-check       — cargo fmt --check"
	@echo "  clippy          — cargo clippy --all-targets --all-features"
	@echo ""
	@echo "  server          — wisp-server on $(SERVER_ADDR) (override SERVER_ADDR)"
	@echo "  receive         — receiver → $(OUT)/ (override OUT; SERVER_URL for rendezvous)"
	@echo "Send via short code (receiver must show CODE):"
	@echo "  send-file       — CODE=… FILE=…"
	@echo "  send-files      — CODE=… FILES=\"path1 path2\""
	@echo "  send-dir        — CODE=… DIR=path/"
	@echo "  send-multiple   — CODE=…  (temp dir of $(MULTIPLE_COUNT) x $(MULTIPLE_SIZE_MB)MB files)"
	@echo "  send-large      — CODE=…  (temp $(LARGE_SIZE_MB)MB file)"
	@echo "  send            — same as send-file if CODE is set; else prints this help"
	@echo ""
	@echo "Send via LAN (mDNS; receiver must run receive on same network):"
	@echo "  send-nearby     — generates a fresh $(NEARBY_SIZE_MB)MB random file; NEARBY_TIMEOUT_SECS=$(NEARBY_TIMEOUT_SECS)"
	@echo ""
	@echo ""
	@echo "TrimUI Brick Pro (TG4040) receiver:"
	@echo "  trimui-preview  — render every screen to target/preview/*.png"
	@echo "  trimui-build    — cross-build a static $(TRIMUI_TARGET) binary (needs 'cross' + Docker)"
	@echo "  trimui-pak      — assemble $(TRIMUI_PAK)/ (binary + launch.sh + config.json + icon)"
	@echo "  trimui-install  — copy the pak to $(TRIMUI_HOST):$(TRIMUI_APP_DIR) (override TRIMUI_HOST)"
	@echo "  trimui-probe    — run the device probe over SSH and save probe.txt"
	@echo ""
	@echo "Env: SERVER_URL=$(if $(SERVER_URL),$(SERVER_URL),<CLI default>)"
	@echo "     TRACE=$(TRACE) (set TRACE=0 to disable CLI tracing logs)"

check:
	cargo check

test:
	cargo test

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

clippy:
	cargo clippy --all-targets --all-features

server:
	$(TRACE_ENV) cargo run -p wisp-server -- serve --listen $(SERVER_ADDR)

# With CODE: delegates to send-file. Without CODE: lists send targets (see help).
send:
ifndef CODE
	@$(MAKE) help
	@exit 1
else
	@$(MAKE) send-file
endif

send-file:
	@if [ -z "$(CODE)" ]; then echo "usage: make send-file CODE=AB2CD3 FILE=path"; exit 1; fi
	$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send -c "$(CODE)" "$(FILE)"

send-dir:
	@if [ -z "$(CODE)" ]; then echo "usage: make send-dir CODE=AB2CD3 DIR=photos/"; exit 1; fi
	@if [ -z "$(DIR)" ]; then echo "usage: make send-dir CODE=AB2CD3 DIR=photos/"; exit 1; fi
	$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send -c "$(CODE)" "$(DIR)"

send-files:
	@if [ -z "$(CODE)" ]; then echo "usage: make send-files CODE=AB2CD3 FILES=\"path1 path2\""; exit 1; fi
	$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send -c "$(CODE)" $(FILES)

send-nearby:
	@set -e; \
		TMP_DIR=$$(mktemp -d); \
		NEARBY_FILE="$$TMP_DIR/nearby-$$(date +%s)-$$(LC_ALL=C tr -dc 'a-z0-9' </dev/urandom | head -c 8).bin"; \
		trap 'rm -rf "$$TMP_DIR"' EXIT INT TERM; \
		echo "Generating $$NEARBY_FILE ($(NEARBY_SIZE_MB)MB) ..."; \
		dd if=/dev/urandom of="$$NEARBY_FILE" bs=1m count="$(NEARBY_SIZE_MB)" status=none 2>/dev/null || \
		dd if=/dev/urandom of="$$NEARBY_FILE" bs=1m count="$(NEARBY_SIZE_MB)" >/dev/null 2>&1; \
		$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send --nearby --nearby-timeout-secs $(NEARBY_TIMEOUT_SECS) "$$NEARBY_FILE"

send-multiple:
	@if [ -z "$(CODE)" ]; then echo "usage: make send-multiple CODE=AB2CD3"; exit 1; fi
	@set -e; \
		TMP_DIR=$$(mktemp -d); \
		TMD_DIR="$$TMP_DIR/tmd"; \
		trap 'rm -rf "$$TMP_DIR"' EXIT INT TERM; \
		mkdir -p "$$TMD_DIR"; \
		i=1; \
		while [ $$i -le $(MULTIPLE_COUNT) ]; do \
			F="$$TMD_DIR/file-$$i.bin"; \
			echo "Generating $$F ($(MULTIPLE_SIZE_MB)MB) ..."; \
			dd if=/dev/urandom of="$$F" bs=1m count="$(MULTIPLE_SIZE_MB)" status=none 2>/dev/null || \
			dd if=/dev/urandom of="$$F" bs=1m count="$(MULTIPLE_SIZE_MB)" >/dev/null 2>&1; \
			i=$$((i+1)); \
		done; \
		$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send -c "$(CODE)" "$$TMD_DIR"

send-large:
	@if [ -z "$(CODE)" ]; then echo "usage: make send-large CODE=AB2CD3"; exit 1; fi
	@set -e; \
		TMP_DIR=$$(mktemp -d); \
		LARGE_FILE="$$TMP_DIR/large.bin"; \
		trap 'rm -rf "$$TMP_DIR"' EXIT INT TERM; \
		echo "Generating $$LARGE_FILE ($(LARGE_SIZE_MB)MB) ..."; \
		dd if=/dev/urandom of="$$LARGE_FILE" bs=1m count="$(LARGE_SIZE_MB)" status=none 2>/dev/null || \
		dd if=/dev/urandom of="$$LARGE_FILE" bs=1m count="$(LARGE_SIZE_MB)" >/dev/null 2>&1; \
		$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- send -c "$(CODE)" "$$LARGE_FILE"

receive:
	$(RENDEZVOUS_ENV) $(TRACE_ENV) cargo run -p wisp -- receive --out "$(OUT)"

# --- TrimUI Brick Pro (TG4040) -----------------------------------------------

trimui-preview:
	cargo run -p wisp-trimui --example preview -- target/preview

# `cross` runs the build in a container that already carries the aarch64 musl
# toolchain, which ring/iroh need. A bare `cargo build --target` only works if
# that toolchain is installed on the host.
trimui-build:
	cross build -p wisp-trimui --release --target $(TRIMUI_TARGET)

trimui-pak:
	@test -f "$(TRIMUI_BIN)" || { \
		echo "missing $(TRIMUI_BIN)"; \
		echo "  cross-build here:  make trimui-build      (needs 'cross' + Docker)"; \
		echo "  or build natively on any aarch64 Linux host and package it with:"; \
		echo "      make trimui-pak TRIMUI_BIN=/path/to/wisp-trimui"; \
		exit 1; }
	@rm -rf "$(TRIMUI_PAK)"
	@mkdir -p "$(TRIMUI_PAK)"
	cp "$(TRIMUI_BIN)" "$(TRIMUI_PAK)/wisp-trimui"
	cp crates/trimui/pak/launch.sh "$(TRIMUI_PAK)/launch.sh"
	cp crates/trimui/pak/config.json "$(TRIMUI_PAK)/config.json"
	cp flutter/assets/wisp_rounded_logo.png "$(TRIMUI_PAK)/icon.png"
	chmod +x "$(TRIMUI_PAK)/launch.sh" "$(TRIMUI_PAK)/wisp-trimui"
	@echo "pak ready: $(TRIMUI_PAK)"
	@ls -lh "$(TRIMUI_PAK)"

# MainUI only scans Apps/ at startup, so it is restarted to pick up a new or
# changed app. It ignores SIGTERM, hence -9.
trimui-install: trimui-pak
	ssh $(TRIMUI_HOST) 'mkdir -p $(TRIMUI_APP_DIR)'
	scp -r "$(TRIMUI_PAK)/." $(TRIMUI_HOST):$(TRIMUI_APP_DIR)/
	ssh $(TRIMUI_HOST) 'chmod +x $(TRIMUI_APP_DIR)/launch.sh $(TRIMUI_APP_DIR)/wisp-trimui && killall -9 MainUI' || true
	@echo "installed to $(TRIMUI_HOST):$(TRIMUI_APP_DIR)"

trimui-probe:
	scp tools/trimui-probe.sh $(TRIMUI_HOST):/tmp/
	ssh $(TRIMUI_HOST) 'sh /tmp/trimui-probe.sh' > probe.txt
	@echo "wrote probe.txt"
