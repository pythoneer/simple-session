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
simple-session apply work         # move open windows back to where "work" has them
simple-session apply work -n      # dry run: only print the plan
simple-session restore work       # launch what is not open, then place everything
simple-session restore work -n    # dry run
simple-session diff work          # what changed since "work" was saved
simple-session diff work home     # compare two sessions
simple-session rename work office
simple-session delete work        # a copy is kept in backups/
```

`apply` only moves windows that are already open; it launches nothing. Open windows are matched to saved ones by app, then by title, then by order. If the monitors changed, each saved monitor is mapped to a current one (same monitor → same connector → same size → primary) and the window rectangles are scaled to fit.

`restore` first launches the apps whose windows are missing (from their desktop file, or the command line they were started with), waits until their windows appear (`--timeout`, default 30 s), and then places all windows like `apply`. Apps that reopen their own windows (browsers, JetBrains IDEs, VS Code, Sublime, Zed) are launched once instead of once per window, and not at all if they are already running.

Sessions are stored in `~/.local/share/simple-session/sessions/` (override with `SIMPLE_SESSION_DIR`).

## Configuration

Optional, in `~/.config/simple-session/config.toml` (override with `SIMPLE_SESSION_CONFIG`). Apps are keyed like in the session files (`simple-session show <name>` lists them). Rules also apply to sessions saved before the rule was added.

```toml
[apps."firefox.desktop"]
launch_mode = "per_window"   # or "once"

[apps."org.gnome.Settings.desktop"]
ignore = true                # never saved, launched or placed

[apps."com.mitchellh.ghostty"]
argv = ["ghostty"]           # launch with this command instead
```

## Development

```sh
cargo test
make link-extension              # symlink extension/ into the shell's extension dir
scripts/nested.sh start          # nested GNOME Shell (needs mutter-devkit)
scripts/nested.sh exec scripts/shim.sh windows
scripts/nested.sh exec cargo run -- doctor
scripts/nested.sh restart        # reloads extension code in about 1 s
scripts/smoke.sh apply|restore   # end-to-end in a nested shell: arrange, save, scramble or close, apply or restore, compare
```

Extension code is only reloaded when the shell starts. Use the nested shell instead of logging out after each change.
