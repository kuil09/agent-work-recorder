PREFIX ?= $(HOME)/.local
BIN := $(PREFIX)/bin

.PHONY: all rec rec-capture test install clean

all: rec rec-capture

rec:
	cargo build --release

rec-capture:
	swift build -c release --package-path macos/RecCapture

test:
	cargo test

install: all
	mkdir -p $(BIN)
	cp target/release/rec $(BIN)/rec
	cp macos/RecCapture/.build/release/rec-capture $(BIN)/rec-capture
	chmod 755 $(BIN)/rec $(BIN)/rec-capture
	@echo installed $(BIN)/rec $(BIN)/rec-capture

clean:
	cargo clean
	rm -rf macos/RecCapture/.build
