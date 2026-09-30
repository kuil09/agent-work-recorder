PREFIX ?= $(HOME)/.local
BIN := $(PREFIX)/bin

.PHONY: all rec rec-capture test install skill-package clean

all: rec rec-capture

rec:
	cargo build --locked --release

rec-capture:
	swift build -c release --package-path macos/RecCapture

test:
	cargo test --locked

install: all skill-package
	mkdir -p "$(BIN)"
	cp target/release/rec "$(BIN)/rec"
	cp macos/RecCapture/.build/release/rec-capture "$(BIN)/rec-capture"
	chmod 755 "$(BIN)/rec" "$(BIN)/rec-capture"
	python3 tools/package_info.py manifest --rec "$(BIN)/rec" --helper "$(BIN)/rec-capture" --output "$(BIN)/agent-work-recorder-build.json"
	@echo installed $(BIN)/rec $(BIN)/rec-capture

skill-package:
	python3 tools/package_info.py skill

clean:
	cargo clean
	rm -rf macos/RecCapture/.build
