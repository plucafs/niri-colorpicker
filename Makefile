PREFIX ?= /usr/local
BIN_DIR = $(PREFIX)/bin
SHARE_DIR = $(PREFIX)/share
APP_NAME = niri-colorpicker
DESKTOP_FILE = $(APP_NAME).desktop

.PHONY: build install uninstall clean

build:
	cargo build --release

install: build
	install -Dm755 target/release/$(APP_NAME) $(BIN_DIR)/$(APP_NAME)
	install -Dm644 $(DESKTOP_FILE) $(SHARE_DIR)/applications/$(DESKTOP_FILE)

uninstall:
	rm -f $(BIN_DIR)/$(APP_NAME)
	rm -f $(SHARE_DIR)/applications/$(DESKTOP_FILE)

clean:
	cargo clean
