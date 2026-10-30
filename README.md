# simple-session

Save the open application windows on GNOME (Wayland) as named sessions: which app, which monitor, which workspace, position, size and state. Later, bring them back.

Two parts:

- **`extension/`**: a thin GNOME Shell extension (`simple-session@dbensing.de`). On Wayland, only the compositor can see and move other apps' windows, so this shim exposes a small D-Bus API (`de.dbensing.SimpleSession.Shim1`). It holds no logic, UI or state.
- **the `simple-session` CLI** (Rust): everything else. It handles capture, storage, monitor identity and, later, launching and placing windows.

## Install

```sh
make install          # copies the extension and runs cargo install
```

GNOME on Wayland loads a newly installed extension only after you log out and back in. Then:

```sh
simple-session doctor # checks the extension, monitors and storage
```

## Usage

```sh
simple-session save work          # save the current windows as "work"
simple-session save work --force  # overwrite (the previous version goes to backups/)
simple-session list
simple-session show work          # table view; --json prints the file
simple-session delete work        # a copy is kept in backups/
```

Sessions are stored in `~/.local/share/simple-session/sessions/` (override with `SIMPLE_SESSION_DIR`).

Restoring (`apply` for windows that are already open, `restore` to relaunch apps) is in progress.

## Development

```sh
cargo test
make link-extension              # symlink extension/ into the shell's extension dir
scripts/nested.sh start          # nested GNOME Shell (needs mutter-devkit)
scripts/nested.sh exec scripts/shim.sh windows
scripts/nested.sh exec cargo run -- doctor
scripts/nested.sh restart        # reloads extension code in about 1 s
```

Extension code is only reloaded when the shell starts. Use the nested shell instead of logging out after each change.
