PREFIX ?= $(HOME)/.local
BIN := $(PREFIX)/bin

.PHONY: all rec rec-capture test install package release-package clean

all: rec rec-capture

rec:
	cargo build --release

rec-capture:
	swift build -c release --package-path macos/RecCapture

test:
	cargo fmt --check
	cargo clippy --all-targets -- -D warnings
	cargo test --all-targets
	swift test --package-path macos/RecCapture
	cargo build
	python3 tests/e2e.py
	python3 tests/check_session_liveness.py

install: all
	mkdir -p $(BIN)
	cp target/release/rec $(BIN)/rec
	cp macos/RecCapture/.build/release/rec-capture $(BIN)/rec-capture
	chmod 755 $(BIN)/rec $(BIN)/rec-capture
	@echo installed $(BIN)/rec $(BIN)/rec-capture

package:
	bash scripts/package-macos.sh development

release-package:
	bash scripts/package-macos.sh release

clean:
	cargo clean
	rm -rf macos/RecCapture/.build
