# screen-ruler

A smart, edge-detection-based screen ruler for Linux, macOS and Windows.

Move your mouse over any UI element and read the **width** and **height** of the
space between the nearest edges — buttons, panels, windows, icons — with no
clicking or dragging required.

Ships as a **single self-contained executable**. No runtime, no interpreter, no
toolkit to install: download it and run it.

## How it works

At launch, screen-ruler captures **every monitor separately** and builds a Canny
edge map per display, at that display's own native resolution. Each monitor is
then covered by its own borderless fullscreen window showing its frozen
screenshot. Measurements are cast against the static edge map rather than the
live screen, so readings stay stable while the cursor moves.

Each monitor keeps its own resolution and scale factor, so a 2x laptop panel
beside a 1x external display measures correctly on both.

## Install

Download the executable for your platform and run it. Nothing else is required.

To build from source you need a Rust toolchain (1.88 or newer):

```bash
cargo build --release
# binary at target/release/screen-ruler
```

### Global keyboard shortcut

screen-ruler is designed to be ephemeral — summon, measure, quit — so it pairs
well with a global hotkey. The install scripts copy the binary somewhere
permanent and register one for you. Each picks up a binary sitting next to the
script, or one at `target/release/`.

```bash
./install-linux.sh -d ~/bin      # Super+Shift+R on GNOME, KDE, Hyprland or Sway
./install-macos.sh -d ~/bin      # creates a Quick Action to bind in System Settings
```

```powershell
.\install-windows.ps1 -InstallDir "$env:LOCALAPPDATA\screen-ruler"   # Ctrl+Shift+R
```

For manual setup on any platform, see
[docs/manual-shortcut-setup.md](docs/manual-shortcut-setup.md).

## Usage

```bash
screen-ruler [OPTIONS]
```

| Option | Default | Description |
|---|---|---|
| `--sensitivity N` | 85 | Edge-detection sensitivity, 0-100. Higher finds more edges. |
| `--threshold-low N` | — | Lower Canny threshold, 0-255. Overrides `--sensitivity`. |
| `--threshold-high N` | — | Upper Canny threshold, 0-255. Overrides `--sensitivity`. |
| `--debug-edge-overlay` | off | Keep the detected edge map visible, for alignment debugging. |
| `-h`, `--help` | | Show help. |
| `-V`, `--version` | | Show the version. |

### Modes

| Key | Mode | What it measures |
|---|---|---|
| `1` | Crosshair | Distance to the nearest edge in each direction |
| `2` | Drag rectangle | A rectangle you drag, snapping to nearby edges |
| `3` | Container | The enclosing UI container under the cursor |
| `4` | Shrink-to-fit | A dragged rectangle, tightened onto the content inside it |
| `5` | Color picker | The colour under the cursor, optionally averaged over a disc |
| `6` | Point distance | The distance between two placed points |

### Controls

| Input | Action |
|---|---|
| Left click | Copy the measurement and quit, or place an annotation in session mode |
| `Enter` | Copy the current drag/shrink selection and quit |
| `Ctrl+C` | Copy the measurement and quit / export annotations as Markdown |
| `Tab` | Toggle session mode (a persistent annotation workspace) |
| `Ctrl+Z` / `Ctrl+Shift+Z` | Undo / redo an annotation |
| `Ctrl+Shift+C` | Then drag a region to copy it as an image with annotations |
| Mouse wheel | Adjust the active mode's control (sensitivity / snap / averaging) |
| `?` or `H` | Toggle the shortcut overlay |
| `Esc` | Cancel the current step, leave session mode, or quit |
| `Q` | Quit |

A shortcut overlay appears briefly at launch and fades; press `?` or `H` to
bring it back.

In session mode, leaving the session or quitting asks for confirmation before
discarding placed annotations — press the same key again to confirm.

## Platform notes

**Linux / Wayland.** Screen capture uses the `wlr-screencopy` protocol directly,
which covers wlroots-based compositors (Hyprland, Sway, river, Wayfire). GNOME
and KDE do not implement that protocol; there the generic backend is tried
instead. Window placement uses fullscreen-on-a-named-output, because a Wayland
client cannot position its own windows.

**Linux clipboard.** On X11 and Wayland the clipboard is owned by the source
application and is emptied when it exits — a problem for a tool whose job is
"measure, copy, quit". screen-ruler therefore hands the copy to `wl-copy`,
`xclip` or `xsel` when one is installed, since those fork a helper that keeps
the selection alive. Without any of them the copy still happens but will not
outlive the process.

**macOS.** Screen capture requires the Screen Recording permission
(System Settings → Privacy & Security). Grant it, then relaunch.

**Windows.** No extra setup.

## Dependencies

The binary is fully self-contained. At build time it uses `winit` + `glutin` for
windowing, `egui` for the interface, `xcap` for capture on X11/macOS/Windows,
and `arboard` for the clipboard. Edge detection, region labelling, colour
sampling, PNG encoding, argument parsing and Wayland capture are implemented
in-tree rather than pulled in as dependencies.

## Development

```bash
cargo test     # unit tests, no display required
cargo clippy   # lint
```

The measurement logic is deliberately free of any windowing or drawing types, so
it can be tested headlessly:

| Module | Responsibility |
|---|---|
| `edges.rs` | Canny edge detection and the sensitivity mapping |
| `measure.rs` | Ray casting, edge snapping, shrink-to-fit |
| `regions.rs` | Connected-component labelling for container detection |
| `color.rs` | Gaussian-weighted colour sampling, hex/rgb/hsl |
| `geometry.rs` | The three coordinate spaces and conversions between them |
| `surface.rs` | Per-monitor analysed surfaces, and the desktop that owns them |
| `state.rs` | Modes, selections, annotations, undo/redo, confirmations |
| `ui/` | Rendering and input, split from the state it draws |
| `window.rs` | winit/glutin shell, one window per monitor |

### Coordinate spaces

Mixing these up is the easiest way to break multi-monitor support, so they are
kept distinct throughout:

- **virtual device px** — the whole desktop in physical pixels; what the window
  system reports for monitor placement.
- **monitor logical px** — one monitor, origin at its top-left, divided by that
  monitor's scale factor. What the UI draws in and what measurements report.
- **monitor image px** — one monitor's screenshot in physical pixels. Where the
  edge map and region map live.

`geometry.rs` also carries the virtual-desktop helpers (`virtual_bounds`,
`monitor_at_virtual`, `logical_to_virtual`). Nothing consumes them yet: they are
the seam for cross-monitor features, such as dragging a measurement from one
screen onto another.

## License

MIT. See [LICENSE](LICENSE).
