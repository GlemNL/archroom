<div align="center">

# Archroom

**A Linux-native, non-destructive photo library and raw developer.**

*A darkroom with the lights on, built on Arch.*

Catalog thousands of photos, cull them from the keyboard, and develop raws on the GPU, without ever touching your originals.

[![License: GPL-3.0-or-later](https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg)](LICENSE)
![Rust](https://img.shields.io/badge/rust-1.85%2B-orange.svg)
![Platform](https://img.shields.io/badge/platform-Linux-lightgrey.svg)
![Status](https://img.shields.io/badge/status-pre--release-yellow.svg)

</div>

> [!NOTE]
> Archroom is **pre-release (v0.1 in progress)**. Import, organizing, Develop and Export work. Expect rough edges: keep your originals backed up (Archroom never modifies them) and see [Status](#status).

## Screenshots

![The Library module: folder tree, filter bar, virtualized grid, metadata panel and filmstrip](images/library.png)
*Library: browse, filter and cull a catalog from the grid.*

![The Develop module: navigator, presets and history on the left, histogram and adjustment panels on the right](images/develop.png)
*Develop: non-destructive raw editing with presets, history, histogram and the full set of panels.*

## Why Archroom

Linux has excellent raw tools, but few that pair a fast catalog with a develop workflow you already know. Archroom is a Library and Develop pair with a filmstrip, panels and sliders, driven from the keyboard.

If you've ever spent an evening in a well-known room full of light, your fingers will find their way around here: the panel order, the slider names and the shortcuts will feel familiar. Everything else is new, open source, and yours.

- **Non-destructive.** Originals are opened read-only. Edits are stored as parameters in the catalog, so every edit can be undone or changed later.
- **GPU-first.** Development runs on a scene-referred, linear-light float pipeline in WGSL compute shaders (via `wgpu`/Vulkan). Sliders re-render in well under a millisecond on a modern GPU.
- **Built for large catalogs.** The Grid is virtualized, work runs as background jobs, and the UI never waits on decoding or disk.
- **Plays well with others.** Ratings, labels and keywords are written to standard XMP sidecars that darktable and digiKam can read.
- **Wayland and X11.** Native on both. Nothing here is tied to one compositor.

## Features

### Library
- Import from a folder (add in place, or copy), skipping duplicates, with progress and cancel
- Raw (via LibRaw), JPEG, PNG and TIFF, with embedded ICC profile handling
- Virtualized Grid, Loupe and filmstrip, plus a folder tree
- Ratings, flags and color labels, with undo/redo for every change
- Hierarchical keywords with autocomplete, plus batch metadata editing
- Collections, collection sets, Quick Collection and smart collections
- Filter bar with full-text search
- XMP sidecar read/write that preserves fields written by other apps

### Export
- JPEG, TIFF (8/16-bit; none, LZW or ZIP) and PNG (8/16-bit), with the ICC profile embedded
- sRGB, Display P3, Adobe RGB and ProPhoto output, full-resolution 16-bit render on the GPU
- Resize by long edge, short edge, box, megapixels or percent (never enlarges)
- File-name templates (`{filename}`, `{seq:4}`, `{date:%Y%m%d}`, `{title}`, `{custom}`), rename/overwrite/skip on conflict
- Metadata: everything, copyright only, or none; optional location removal; keyword hierarchy
- Presets, and a background batch with progress and cancel while you keep working
- `archroom-cli export` does the same without the UI

<p align="center">
  <img src="images/export.png" alt="The Export dialog: format, color space, resize, destination, file name template and metadata options" width="360">
</p>

### Develop
- White balance, exposure, contrast, highlights, shadows, whites and blacks
- Tone curve (parametric and point), HSL and B&W mixer
- Clarity, sharpening, and luminance and color noise reduction
- Crop and straighten, with overlays
- Vignette
- Histogram, clipping display, before/after, eyedropper, Auto Tone and Auto WB
- Persistent history, copy/paste settings, and presets (with `.arpreset` import/export)
- Navigator with zoom

## Keyboard shortcuts

| Key | Action |
|---|---|
| `0`–`5` | Set rating |
| `6`–`9` | Set color label |
| `P` / `X` / `U` | Pick / Reject / Unflag |
| `B` | Add to Quick Collection |
| `Ctrl+K` | Focus keyword entry |
| `Ctrl+[` / `Ctrl+]` | Rotate left / right |
| `Ctrl+S` | Write XMP sidecars |
| `Ctrl+Shift+E` | Export the selection |
| `Ctrl+Alt+Shift+E` | Export again with the last settings |
| `Ctrl+,` | Preferences |
| `Ctrl+Z` / `Ctrl+Y` | Undo / Redo |
| `Ctrl+A` / `Ctrl+D` | Select all / none |
| `←` / `→` | Previous / next photo |
| `Tab` / `Shift+Tab` | Toggle side panels / all panels |
| `J` | Show clipping (Develop) |
| `W` | White balance eyedropper (Develop) |
| `V` | Switch color / B&W (Develop) |
| `\` | Before / after (Develop) |
| `O` | Cycle crop overlay (Develop) |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste settings (Develop) |

## Building

Archroom is a Rust workspace. You need a recent stable Rust toolchain (1.85 or newer) and a few system libraries.

### System dependencies

| Library | Why |
|---|---|
| `libraw` (≥ 0.20; 0.22 recommended) | Raw decoding |
| `lcms2` | Color management |
| `exiv2` / `gexiv2` | Metadata |
| `clang` / libclang | Generating the LibRaw bindings |
| A Vulkan loader and driver | GPU rendering (Mesa's `lavapipe` works as a software fallback) |

**Arch Linux**

```sh
sudo pacman -S --needed rust libraw lcms2 exiv2 libgexiv2 clang \
    vulkan-icd-loader vulkan-radeon vulkan-intel vulkan-swrast
```

Other distributions need the equivalent development packages (for example `libraw-dev`, `liblcms2-dev`, `libgexiv2-dev` and `libclang-dev` on Debian/Ubuntu). Packaging for more distros is [planned](#roadmap).

### Install (Arch Linux)

`packaging/PKGBUILD` builds the package from a release tag:

```sh
cd packaging && makepkg -si
```

### Run

```sh
git clone https://github.com/GlemNL/archroom
cd archroom
cargo run --release -p archroom-app
```

If you have [`just`](https://github.com/casey/just), `just run` does the same.

### Develop

```sh
just test    # all tests
just lint    # rustfmt, clippy (warnings denied), dependency-rule check
just deny    # license and advisory check (needs cargo-deny)
just bless   # re-bless golden images after an intentional rendering change
```

The workspace is split so that the engine, catalog, jobs and services crates never depend on the UI toolkit. A headless CLI uses them too:

```sh
cargo run -p archroom-cli -- render --help
```

## Where things live

| What | Where |
|---|---|
| Config | `~/.config/archroom/config.toml` |
| Presets | `~/.local/share/archroom/presets/` |
| Catalog | `~/Pictures/Archroom/Archroom.arcat` (SQLite) |
| Previews | Next to the catalog, in `Archroom Previews/` (trimmed to the cache budget at startup) |
| Backups | Next to the catalog, in `Archroom Backups/` (daily, newest few kept) |
| Caches | `~/.cache/archroom/` (safe to delete) |
| Logs | `~/.local/state/archroom/logs/` |

## Architecture

```
crates/
  core/            ids, settings, events, errors
  color/           color math, chromatic adaptation, lcms2 wrapper
  libraw-sys/      LibRaw bindings
  io/              decoders, metadata, XMP
  catalog/         SQLite schema, queries, undoable commands
  jobs/            priority scheduler with cancellation
  engine/          GPU pipeline, ops and WGSL shaders
  preview/         thumbnail and preview cache
  services/        import, presets, develop sessions
  ui-kit/          shared egui widgets and theme
  module-library/  Library module
  module-develop/  Develop module
  shell/, app/     window, module switcher, the `archroom` binary
  cli/             headless `archroom-cli`
```

Design decisions are recorded as ADRs in [`docs/adr/`](docs/adr), and the full plan is in [`PLAN.md`](PLAN.md).

## Status

| Milestone | State |
|---|---|
| M0 Foundations | Done |
| M1 Import and browse | Done |
| M2 Organize | Nearly done (remove-from-catalog and missing-file handling pending) |
| M3 Develop foundation | Done |
| M4 Develop tools | Mostly done (image-quality tuning pass pending) |
| M5 Export and v0.1 release | Export, backups, Preferences and packaging done; performance and the release checklist pending |

## Roadmap

- Own GPU demosaic (a full-size export of a 24 MP raw is bound by LibRaw's ~1.1 s CPU decode), 1:1 preview cache
- Flatpak (after the AUR package)
- After v0.1: local adjustments (gradients, brushes, masks), spot removal, lens corrections, monitor ICC profiles

## Contributing

Issues and pull requests are welcome. Please run `just lint` and `just test` before opening a PR. For rendering changes, review any golden-image diffs by eye before blessing them.

## License

Archroom is licensed under the [GPL-3.0-or-later](LICENSE).

Archroom is an independent project. It is not affiliated with or endorsed by Adobe. Lightroom is a trademark of Adobe Inc.
