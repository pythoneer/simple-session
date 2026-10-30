UUID := simple-session@dbensing.de
EXT_DIR := $(HOME)/.local/share/gnome-shell/extensions/$(UUID)

.PHONY: install-extension link-extension uninstall-extension zip enable logs

# Copy the extension into place. GNOME on Wayland only picks up a newly
# installed extension after logging out and back in.
install-extension:
	rm -rf "$(EXT_DIR)"
	mkdir -p "$(EXT_DIR)"
	cp -r extension/. "$(EXT_DIR)/"

# Development: symlink so edits only need a re-login, not a reinstall.
link-extension:
	rm -rf "$(EXT_DIR)"
	ln -s "$(CURDIR)/extension" "$(EXT_DIR)"

uninstall-extension:
	-gnome-extensions disable $(UUID)
	rm -rf "$(EXT_DIR)"

enable:
	gnome-extensions enable $(UUID)

zip:
	cd extension && zip -r "../$(UUID).zip" .

logs:
	journalctl --user -b -f -o cat /usr/bin/gnome-shell

.PHONY: build test install-cli install

build:
	cargo build --release

test:
	cargo test

install-cli:
	cargo install --path . --locked

install: install-extension install-cli
