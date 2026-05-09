CARGO ?= cargo

.DEFAULT_GOAL := release

.PHONY: help release debug cli gui run dev app icons bench test fmt clippy clean install-tauri-cli

help:
	@printf 'targets:\n'
	@printf '  release            release build of the whole workspace\n'
	@printf '  debug              debug build\n'
	@printf '  cli                build the hgr CLI (target/release/hgr)\n'
	@printf '  gui                build the GUI binary (target/release/hangar)\n'
	@printf '  run                run the GUI in release mode\n'
	@printf '  dev                run the GUI in debug mode (faster compile)\n'
	@printf '  app                bundle a native app via cargo tauri build\n'
	@printf '                     (requires cargo-tauri-cli; .app/.dmg on macOS,\n'
	@printf '                      .msi/.exe on Windows, .deb/.AppImage on Linux)\n'
	@printf '  icons              regenerate platform icon variants from icons/icon.png\n'
	@printf '  bench              run the head-to-head benchmark\n'
	@printf '  test               cargo test --workspace\n'
	@printf '  fmt                cargo fmt --all\n'
	@printf '  clippy             cargo clippy --workspace --all-targets\n'
	@printf '  clean              cargo clean\n'
	@printf '  install-tauri-cli  one-time tauri-cli install (needed for app/icons)\n'

release:
	$(CARGO) build --release

debug:
	$(CARGO) build

cli:
	$(CARGO) build --release -p hangar-cli

gui:
	$(CARGO) build --release -p hangar-gui

run: gui
	./target/release/hangar

dev:
	$(CARGO) run -p hangar-gui

app:
	cd crates/gui && $(CARGO) tauri build

icons:
	cd crates/gui && $(CARGO) tauri icon icons/icon.png

bench:
	bash bench/run.sh

test:
	$(CARGO) test --workspace

fmt:
	$(CARGO) fmt --all

clippy:
	$(CARGO) clippy --workspace --all-targets

clean:
	$(CARGO) clean

install-tauri-cli:
	$(CARGO) install tauri-cli --version "^2.0"
