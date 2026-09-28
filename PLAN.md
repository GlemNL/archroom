# Archroom: MVP Plan

*A Linux-native, non-destructive photo library and raw developer, modeled on Lightroom Classic's **Library** and **Develop** modules.*

Status: Draft 2 · 2026-09-28 · Decisions D1–D9 accepted (§15)

---

## 0. Summary

- **What.** A catalog-based photo manager (import, browse, cull, rate, keyword, collect, filter), a non-destructive raw editor (white balance, tone, curves, HSL, detail, crop, presets), and export. The layout, panel order, slider names and shortcuts follow Lightroom Classic.
- **How.** A Rust workspace. The UI uses `egui`/`eframe` on `wgpu`. Editing runs as a GPU compute pipeline (WGSL) in scene-referred linear Rec.2020 float. LibRaw decodes raws, exiv2 handles metadata, lcms2 handles ICC, and SQLite stores the catalog.
- **Shape.** A small core plus registries. Top-level modules (Library, Develop, later Map/Print…), develop operations, decoders, encoders, library filters, canvas tools and side panels are each one trait with one registration. Adding a feature means adding a file.
- **Path.** Six milestones (M0 to M5). After M0, the Library track and the Develop track can move forward in parallel. M5 ships v0.1, the MVP.
- **Decisions.** All settled (§15): egui, GPL-3.0-or-later, linear Rec.2020, CAT16, XMP auto-write off with Adobe naming, file-based previews, one catalog at a time, hue-preserving tone mapping, AUR first.

---

## 1. Scope

### 1.1 Goals

1. Handle catalogs of 50k+ photos smoothly on Linux (Wayland and X11).
2. Develop raws from mainstream cameras at a quality comparable to Lightroom and darktable defaults. The target is a pleasing result out of the box, not pixel-identical output to Adobe.
3. Keep Lightroom muscle memory: same layout, panel order, slider names and ranges, and keyboard shortcuts.
4. Never modify originals. Edits are stored as parameters in the catalog, plus optional XMP sidecars.
5. Build an architecture that makes the next 20 features cheap to add.

### 1.2 Out of scope for the MVP

- Lightroom modules other than Library and Develop: Map, Book, Slideshow, Print, Web.
- Local adjustments (masks, brushes, gradients), spot healing and red-eye. **These come first after the MVP**, and the architecture already accounts for them (§6.12).
- AI features (subject/sky masks, AI denoise, generative remove) and face recognition.
- HDR and panorama merge, tethered capture, cloud sync, mobile.
- Reading Lightroom `.lrcat` catalogs or reproducing Adobe's rendering. Reading standard XMP metadata that Lightroom wrote (ratings, labels, keywords) **is** in scope.
- A plugin runtime. The internal extension points are the future plugin contract.

### 1.3 Tier tags used in this document

| Tag | Meaning |
|---|---|
| **MVP** | Required for v0.1 |
| **MVP+** | Wanted for v0.1, can slip to v0.2 without blocking the release |
| **Later** | Post-MVP backlog. The design must not rule it out. |

---

## 2. Principles

1. **Non-destructive and parameter-based.** Originals are opened read-only. The rendered result is a function of (original, parameters, process version).
2. **The catalog is the source of truth; sidecars are for interop.** The SQLite catalog is authoritative. XMP sidecars mirror metadata (and optionally develop settings) for other apps and for disaster recovery.
3. **Scene-referred float pipeline, GPU first.** Data stays in linear light until the tone mapper, in a wide-gamut working space. Interactive editing runs on a screen-sized proxy.
4. **The UI thread never waits.** Decoding, database writes, preview generation and export run as background jobs. The UI always shows the best image available right now: embedded preview, then rendered preview, then live render.
5. **Registries instead of switch statements.** Every kind of thing (module, op, decoder, encoder, filter, tool, panel) is a trait plus a registry.
6. **Headless engine.** The engine, catalog and services crates never depend on the UI toolkit. A CLI uses them too (tests, batch jobs, benchmarks), which also keeps the UI toolkit replaceable.
7. **Deterministic, versioned rendering.** Every op carries an algorithm version, so an old edit renders the same after upgrades. This is Lightroom's "Process Version" idea.

---

## 3. Technology choices

| Concern | Choice | Why | Alternatives considered |
|---|---|---|---|
| Language | Rust (stable, edition 2024), Cargo workspace | Performance and safety in heavily threaded code; workspace crates give modularity for free | C++20 + Qt 6 (mature, slower iteration); Python (too slow for the pipeline) |
| UI | `egui` + `eframe` (wgpu backend), `egui_extras`, an icon font | Immediate mode suits dense tool UIs. It shares the **same wgpu device** as the engine, so the develop view draws the pipeline's output texture with zero copies. Dark theming is trivial and iteration is fast. | Qt 6/QML via cxx-qt (best native feel, heavy FFI); Slint (declarative, GPU interop less direct); iced (wgpu-based, less mature widgets) |
| GPU compute | `wgpu` + WGSL compute shaders | Vulkan on Linux (RADV, ANV, NVIDIA), portable, and runs on lavapipe for CI | Raw Vulkan via `ash` (more control, much more code); OpenCL (driver pain on Linux) |
| Raw decoding | LibRaw ≥ 0.22 through our own `libraw-sys` (bindgen) plus a safe wrapper | Widest camera coverage (CR3, X-Trans…), stable C API, packaged in Arch | `rawler` (pure Rust, from dnglab) as a second backend behind the same trait (Later) |
| Other formats | `zune-jpeg` / `image`, `tiff`, `png`; `libheif` (Later) | | |
| Metadata read/write | exiv2 via gexiv2 (`rexiv2`) | Reads and writes EXIF/IPTC/XMP for almost every format, and preserves XMP fields written by other apps | `kamadak-exif` / `nom-exif` (pure Rust, read-only) as a fallback |
| Color management | `lcms2` for ICC, plus our own matrix and CAT code | ICC for export and (later) monitor profiles; the hot path lives in shaders | |
| Catalog | SQLite (`rusqlite` bundled, WAL, FTS5) + `rusqlite_migration` | A single-file catalog like `.lrcat`; fast and robust | |
| Resize / encode | `fast_image_resize` (Lanczos3), `mozjpeg` or `turbojpeg`, `tiff`, `png` | Quality and speed | |
| Concurrency | `rayon` for CPU data parallelism, plus our own job scheduler on `crossbeam-channel` | Needs priorities and cancellation; no async runtime required | tokio (not needed) |
| Desktop integration | `rfd` (xdg-desktop-portal dialogs), `trash` (freedesktop trash), `open` (show in file manager), `directories` (XDG paths), `notify` (FS watching, Later) | Works well on Wayland | |
| Serialization | `serde`, `serde_json` (edit params), `toml` (config) | | |
| Diagnostics | `tracing`, `puffin` + `puffin_egui`, wgpu timestamp queries | | Tracy |
| Errors | `thiserror` in libraries, `anyhow` in binaries | | |
| Testing | `insta`, `proptest`, `criterion`, `egui_kittest` | | |

**System packages (Arch):** `libraw lcms2 exiv2 libgexiv2 clang vulkan-icd-loader vulkan-radeon vulkan-intel vulkan-swrast`, plus `lensfun libheif` later. `clang` is needed for bindgen, and `vulkan-swrast` provides lavapipe for GPU tests. On this machine libraw 0.22.2, lcms2 and exiv2 0.28 are already installed; `libgexiv2`, `vulkan-intel` and `vulkan-swrast` are not. M0 must also confirm that Arch's exiv2 build reads CR3 (BMFF). LibRaw's EXIF fields are the fallback.

**Hardware targets on this dev box.** The reference GPU is the Radeon RX 7900 (RADV). The low-end budget GPU is the Intel UHD 630 iGPU (ANV): if the app is usable there, it is usable almost anywhere. The NVIDIA RTX 2070 with the proprietary driver must be tested before release.

**License.** The project is licensed **GPL-3.0-or-later** (D2). exiv2 (GPL-2.0-or-later) and LibRaw (LGPL-2.1/CDDL) are both compatible with it.

---

## 4. Architecture

### 4.1 Layers

```
┌──────────────────────────────────────────────────────────────────────┐
│ app (binary)     shell · module switcher · menus · shortcuts · prefs │
│  ├─ module-library   grid · loupe · import/export dialogs · panels   │
│  └─ module-develop   canvas · tools · adjustment panels · presets    │
│ ui-kit           sliders · histogram · curve editor · filmstrip · …  │
├──────────────────────────────────────────────────────────────────────┤
│ services         import · export · sidecar sync · auto tone/WB       │
│ preview          thumbnail & preview cache                           │
│ catalog · jobs   SQLite repository · background job scheduler        │
├──────────────────────────────────────────────────────────────────────┤
│ engine           op registry · pipeline graph · stage cache · wgpu   │
│ color            spaces · CAT · temp/tint · transfer fns · ICC       │
│ io               decoders (LibRaw, image) · metadata (exiv2) · enc.  │
├──────────────────────────────────────────────────────────────────────┤
│ core             ids · shared types · settings · events · errors     │
└──────────────────────────────────────────────────────────────────────┘
   cli (headless binary) uses every layer below the UI line
```

### 4.2 Workspace layout

```
archroom/
├─ Cargo.toml               # [workspace]: shared deps, lints, profiles
├─ crates/
│  ├─ core/                 # archroom-core: ids, types, settings, events, errors
│  ├─ color/                # archroom-color: color math, CAT, temp/tint, lcms2 wrapper
│  ├─ libraw-sys/           # bindgen FFI to the system LibRaw
│  ├─ io/                   # archroom-io: decoders, metadata, encoders
│  ├─ catalog/              # archroom-catalog: schema, migrations, queries, criteria
│  ├─ jobs/                 # archroom-jobs: scheduler, priorities, cancellation
│  ├─ engine/               # archroom-engine: ops, pipeline, GPU; ops/*.rs + ops/*.wgsl
│  ├─ preview/              # archroom-preview: thumbnail/preview generation & cache
│  ├─ services/             # archroom-services: import, export, sidecars, analysis
│  ├─ ui-kit/               # archroom-ui: reusable egui widgets and theme
│  ├─ module-library/
│  ├─ module-develop/
│  ├─ app/                  # the `archroom` binary
│  └─ cli/                  # the `archroom-cli` binary
├─ assets/                  # icons, fonts, built-in presets, profiles
├─ tests/fixtures/          # small images; the big raw set is downloaded by a script
├─ docs/adr/                # architecture decision records
├─ packaging/               # PKGBUILD, .desktop file (Flatpak later)
└─ justfile                 # run, test, lint, bench, fixtures, bless
```

**Dependency rules.** A small `xtask` check enforces these in CI.
- `core` depends on no other internal crate.
- `color`, `io`, `catalog` and `jobs` depend only on `core` (and `io` on `color`).
- `engine` depends on `core`, `color` and the decoded-image types from `io`. It never depends on a UI crate.
- `preview` and `services` may use everything above except UI crates.
- `ui-kit` depends only on `egui` and `core`.
- Modules depend on `services` and `ui-kit`. **Modules never call each other.** They communicate only through `AppCx`: shared state, commands and events.

### 4.3 Extension points

| Extension point | Trait | Registered in | Adding one means… |
|---|---|---|---|
| Top-level module (Library, Develop; later Map, Print) | `Module` | `app` | A crate implementing `Module`, plus one line in the registry |
| Develop operation (Exposure, HSL…) | `Op` + WGSL kernel | `engine::ops::registry()` | One `.rs` file and one `.wgsl` file. Persistence, defaults, preset/sync grouping, caching and a default slider panel come with it. |
| Develop panel (custom UI for an op group) | `DevelopPanel` | `module-develop` | Only needed when the auto-generated sliders aren't enough (curves, crop, HSL tabs) |
| Canvas tool (Crop, WB picker; later Mask, Spot) | `CanvasTool` | `module-develop` | Pointer handling, overlay drawing and a panel |
| Side panel (Keywording, Metadata, Histogram…) | `SidePanel` | Each module | A widget plus its default position and open state |
| Decoder (raw, JPEG, TIFF; later HEIF, JXL) | `Decoder` | `io::decoders()` | Probe, metadata, embedded preview and decode functions |
| Metadata backend | `MetadataReader` / `MetadataWriter` | `io` | exiv2 by default; a pure-Rust fallback |
| Export encoder (JPEG, TIFF, PNG; later AVIF, JXL) | `Encoder` | `services::export` | An encode function and an options schema |
| Library filter or smart-collection rule | `Criterion` | `catalog::criteria` | A SQL fragment builder and a UI descriptor |
| Background work | `Job` | `jobs` | A run function, a priority and cancellation checks |
| Undoable action | `Command` | Anywhere | apply/revert; it becomes undoable automatically (and scriptable later) |

A future plugin API (Lua or WASM) would wrap `Command`, `Criterion`, `Encoder` and possibly WGSL `Op`s. These internal traits are the contract.

### 4.4 Key interfaces (sketches, not final)

```rust
/// A top-level workspace: Library, Develop; later Map, Print…
pub trait Module {
    fn id(&self) -> ModuleId;                          // "library", "develop"
    fn title(&self) -> &str;
    fn on_enter(&mut self, _cx: &mut AppCx) {}
    fn on_leave(&mut self, _cx: &mut AppCx) {}
    fn left_panel(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn right_panel(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn toolbar(&mut self, _ui: &mut egui::Ui, _cx: &mut AppCx) {}
    fn center(&mut self, ui: &mut egui::Ui, cx: &mut AppCx);
    /// Module-scoped actions, resolved from the keymap (e.g. `R` → crop in Develop).
    fn handle_action(&mut self, _action: Action, _cx: &mut AppCx) -> bool { false }
}

/// One develop adjustment. Written once; the registry derives persistence,
/// preset/sync grouping, default UI and cache keys from it.
pub trait Op: Send + Sync + 'static {
    type Params: Serialize + DeserializeOwned + Default + Clone + PartialEq;

    const ID: &'static str;        // stable key stored in catalogs, e.g. "exposure"
    const VERSION: u32;            // algorithm version → process versioning
    const STAGE: Stage;            // Geometry | Scene | ToneMap | Display | Output
    const ORDER: u16;              // position inside the stage
    const GROUP: SettingsGroup;    // granularity for presets, copy/paste and sync

    fn specs() -> &'static [ParamSpec];   // label, range, default, steps, unit, track style
    fn is_identity(p: &Self::Params) -> bool { *p == Self::Params::default() }
    fn apron(_p: &Self::Params, _scale: f32) -> u32 { 0 } // neighborhood radius for ROI/tiles
    fn kernel(&self) -> &dyn GpuKernel<Self::Params>;     // WGSL + uniform packing (+ optional mask input)
}
// The registry stores type-erased `Box<dyn DynOp>` values (params as serde_json::Value)
// produced by a blanket impl, so the pipeline and UI stay generic.

pub trait Decoder: Send + Sync {
    fn id(&self) -> &'static str;                                   // "libraw", "image"
    fn probe(&self, path: &Path, header: &[u8]) -> Option<Priority>;
    fn metadata(&self, path: &Path) -> Result<ImageMetadata>;
    fn embedded_preview(&self, path: &Path) -> Result<Option<EncodedImage>>;
    fn decode(&self, path: &Path, opts: &DecodeOptions) -> Result<DecodedImage>;
}

pub enum DecodedImage {
    /// Raws: linear camera RGB + camera matrix + as-shot white balance.
    SceneLinear { rgb: ImageF32, camera: CameraColor },
    /// JPEG/TIFF/PNG: display-referred pixels + embedded ICC profile.
    Rendered { rgb: ImageF32, icc: Option<Vec<u8>> },
}

/// Every mutation of the catalog is a command, so it is undoable (and scriptable later).
pub trait Command: Send {
    fn label(&self) -> Cow<'static, str>;              // "Set rating 3 (12 photos)"
    fn apply(&mut self, tx: &mut CatalogTx) -> Result<ChangeSet>;
    fn revert(&mut self, tx: &mut CatalogTx) -> Result<ChangeSet>;
}
```

`ParamSpec` drives the auto-generated slider UI. It holds the label, min, max, default, fine and coarse steps, unit (`EV`, `K`, `%`), display precision, response curve (linear or mired for Temp), and track style (plain, Temp/Tint gradient, hue gradient).

### 4.5 State, commands and events

- Modules receive an `AppCx`. It holds `catalog`, `selection` (active photo, selected set, current source, filter, sort), `jobs`, `engine`, `previews`, `settings`, `commands` and event subscriptions.
- Every mutation goes through a `Command`. The command runs a catalog transaction, which emits a `CatalogEvent` (`PhotosChanged { ids, fields }`, `CollectionsChanged`, `ImportProgress`…). View models then invalidate and call `ctx.request_repaint()`.
- **Undo.** Library keeps a session-scoped command stack. Develop keeps a persisted history per photo, where each slider drag becomes one coalesced history step. `Ctrl+Z` goes to the active module's stack.
- Background threads never touch egui state. They send events over channels that the UI drains at the start of each frame.

### 4.6 Threading and jobs

| Thread / pool | Work |
|---|---|
| UI thread | egui frame, input, cheap state changes, recording GPU commands for interactive renders |
| Job workers (cores − 1) | Import scan, metadata, preview decode/encode, XMP writes, export encoding |
| rayon pool | Data-parallel loops inside jobs (raw prep, resize) |
| DB writer | One connection that serializes writes in batched transactions; readers use a small pool of read-only WAL connections |
| GPU | One `wgpu::Device`/`Queue` shared by UI and engine. Background renders (previews, export) submit in small chunks so they never stall interactive frames. |

Job priorities run `Interactive > Visible (on-screen thumbnails, loupe) > UserBatch (import, export) > Background (preview pre-render, sidecar sync)`. Every job has a cancellation token, so scrolling away cancels stale thumbnail jobs. Progress shows in the top-bar activity indicator.

### 4.7 Files on disk

| What | Where |
|---|---|
| Config | `~/.config/archroom/config.toml` |
| User presets | `~/.local/share/archroom/presets/**/*.json` (one file per preset, so they can be shared) |
| Catalog | User-chosen. Default: `~/Pictures/Archroom/Archroom.arcat` (SQLite). One catalog is open at a time (D7); "Open Catalog…" to switch is MVP+. |
| Rendered previews | Next to the catalog in `Archroom Previews/` (JPEG files plus a `previews.db` index, D6), so the catalog stays portable like Lightroom's |
| Volatile caches (1:1 previews) | `~/.cache/archroom/`, size-capped and safe to purge |
| Logs | `~/.local/state/archroom/logs/` |
| Catalog backups | `~/Pictures/Archroom/Backups/` via the SQLite online-backup API, on a schedule or at exit |

---

## 5. Data model

### 5.1 Catalog schema (v1 sketch)

```sql
CREATE TABLE folders (
  id        INTEGER PRIMARY KEY,
  parent_id INTEGER REFERENCES folders(id),
  path      TEXT NOT NULL UNIQUE,
  name      TEXT NOT NULL
);

-- One row per physical file on disk.
CREATE TABLE files (
  id INTEGER PRIMARY KEY,
  folder_id INTEGER NOT NULL REFERENCES folders(id),
  filename TEXT NOT NULL, ext TEXT NOT NULL,
  kind TEXT NOT NULL,                        -- raw | jpeg | tiff | png | heif …
  size INTEGER, mtime INTEGER, quick_hash BLOB,   -- duplicate detection
  width INTEGER, height INTEGER, orientation INTEGER,
  capture_time TEXT, camera_make TEXT, camera_model TEXT, lens TEXT,
  focal_length REAL, aperture REAL, shutter REAL, iso INTEGER,
  gps_lat REAL, gps_lon REAL,
  sidecar_mtime INTEGER, missing INTEGER NOT NULL DEFAULT 0,
  UNIQUE (folder_id, filename)
);

-- One row per catalog "photo": the master, or a virtual copy of the same file.
CREATE TABLE photos (
  id INTEGER PRIMARY KEY,
  file_id INTEGER NOT NULL REFERENCES files(id),
  copy_name TEXT,                            -- NULL = master
  rating INTEGER NOT NULL DEFAULT 0,         -- 0..5
  flag INTEGER NOT NULL DEFAULT 0,           -- -1 reject, 0 none, 1 pick
  color_label TEXT,
  title TEXT, caption TEXT, creator TEXT, copyright TEXT,
  user_orientation INTEGER NOT NULL DEFAULT 0,
  import_id INTEGER REFERENCES imports(id),
  imported_at TEXT, edited_at TEXT
);

CREATE TABLE develop_settings (
  photo_id INTEGER PRIMARY KEY REFERENCES photos(id),
  process_version INTEGER NOT NULL,
  params TEXT NOT NULL,                      -- JSON, see 5.2
  params_hash BLOB NOT NULL                  -- keys previews and caches
);
CREATE TABLE history   (id INTEGER PRIMARY KEY, photo_id INTEGER NOT NULL, seq INTEGER NOT NULL,
                        label TEXT NOT NULL, params TEXT NOT NULL, created_at TEXT);
CREATE TABLE snapshots (id INTEGER PRIMARY KEY, photo_id INTEGER NOT NULL, name TEXT NOT NULL,
                        params TEXT NOT NULL, created_at TEXT);

CREATE TABLE keywords (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES keywords(id),
                       name TEXT NOT NULL, include_on_export INTEGER NOT NULL DEFAULT 1,
                       UNIQUE (parent_id, name));
CREATE TABLE photo_keywords (photo_id INTEGER, keyword_id INTEGER, PRIMARY KEY (photo_id, keyword_id));

CREATE TABLE collections (id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES collections(id),
                          kind TEXT NOT NULL,          -- set | regular | smart | quick
                          name TEXT NOT NULL, rules TEXT, sort TEXT);
CREATE TABLE collection_photos (collection_id INTEGER, photo_id INTEGER, position REAL,
                                PRIMARY KEY (collection_id, photo_id));

CREATE TABLE imports (id INTEGER PRIMARY KEY, started_at TEXT, source TEXT, mode TEXT, count INTEGER);

CREATE VIRTUAL TABLE photo_fts USING fts5(filename, title, caption, keywords);  -- kept in sync by triggers
```

Indexes cover `capture_time`, `rating`, `flag`, `color_label`, `folder_id`, `camera_model` and `lens`. History stores the full params at each step. That is simple, and each step is only a few KB. History is compacted beyond a configurable number of steps.

### 5.2 Develop settings format

```json
{
  "process_version": 1,
  "ops": {
    "profile":       { "v": 1, "name": "standard", "treatment": "color" },
    "white_balance": { "v": 1, "mode": "custom", "temp": 5600, "tint": 8 },
    "exposure":      { "v": 1, "ev": 0.35 },
    "tone":          { "v": 1, "contrast": 12, "highlights": -40, "shadows": 35, "whites": 10, "blacks": -8 },
    "tone_curve":    { "v": 1, "rgb": [[0, 0], [0.25, 0.21], [0.75, 0.80], [1, 1]] },
    "crop":          { "v": 1, "rect": [0.04, 0.02, 0.97, 0.95], "angle": -1.2, "aspect": "3:2" }
  },
  "local": []
}
```

- Only non-identity ops are stored. Defaults live in code, per op version and per file kind (raw vs rendered).
- Each op entry carries `v`, its algorithm version. Rendering uses the stored version. "Update to current process version" is an explicit user action, as in Lightroom.
- Unknown op IDs survive a load/save round trip, which gives forward compatibility and room for future plugins.
- `params_hash` is an xxh3 hash of the canonical JSON.
- `local` is reserved for masks and local adjustments (Later, §6.12).
- **Settings groups** decide the granularity of presets, copy/paste and sync: Treatment & Profile, White Balance, Basic Tone, Presence, Tone Curve, HSL/Color, B&W Mix, Sharpening, Noise Reduction, Lens Corrections, Crop, Straighten, Effects, Process Version.

### 5.3 XMP sidecars

| Catalog field | XMP property |
|---|---|
| Rating (a reject can optionally be written as −1, darktable-style) | `xmp:Rating` |
| Color label | `xmp:Label` |
| Keywords, flat | `dc:subject` |
| Keywords, hierarchical | `lr:hierarchicalSubject` (e.g. Places > France > Paris, stored pipe-separated) |
| Title / caption | `dc:title` / `dc:description` |
| Creator / copyright | `dc:creator` / `dc:rights` |
| Pick flag | `archroom:Flag` |
| Develop settings (optional) | `archroom:DevelopSettings` (JSON) + `archroom:ProcessVersion` |

- Writes are read-modify-write through exiv2, so fields written by other apps (Adobe `crs:`, darktable history) are preserved.
- Naming follows Adobe (D5): `IMG_0001.xmp`. When a RAW+JPEG pair shares a basename, the name falls back to `IMG_0001.CR3.xmp`. Both are configurable.
- `Ctrl+S` writes the sidecar manually. Automatic writing is a preference, **off by default** as in Lightroom (D5).
- On folder sync, a sidecar newer than the recorded `sidecar_mtime` gets a "metadata changed on disk" badge, with import and overwrite actions (MVP+).
- Existing sidecars are read on import (MVP), so ratings and keywords from Lightroom, darktable or digiKam come along.

### 5.4 Previews

| Level | Source | Size | Stored | Used for |
|---|---|---|---|---|
| L0 embedded | JPEG inside the raw (LibRaw `unpack_thumb`), or the JPEG itself | As-is | Not stored; used to generate L1 | First paint right after import |
| L1 thumbnail | L0, or L2 downscaled | 320 px long edge (640 on HiDPI) | JPEG files + `previews.db` index | Grid, filmstrip |
| L2 standard | Full pipeline with current settings | Configurable, default 2048 px | JPEG q90 files + index | Loupe, compare, Develop placeholder while decoding |
| L3 1:1 | Full-resolution render | Full size | `~/.cache/archroom`, LRU budget (e.g. 5 GB) | Loupe zoom (MVP+) |

- Previews are keyed by `(photo_id, level, params_hash)`. A params change marks L1/L2 stale, and a background re-render follows, visible photos first.
- The option "keep embedded previews until edited" defaults to on. Import stays fast, and the camera's look remains until the photo is touched.
- Thumbnail textures live in VRAM in an LRU bounded by a budget (e.g. 512 MB).

---

## 6. Develop engine

### 6.1 Flow

```
 original file (read-only)
   │   CPU, once per image, result cached in RAM
   ▼
 [Raw prep] LibRaw unpack → black/white levels → D65 reference WB → highlight clip
            → demosaic → linear camera RGB (f32)
            (rendered files: decode → embedded ICC → linear)
   │   upload once to GPU
   ▼
 [Geometry] orientation · rotate/straighten · crop · (lens distortion, Later)
            one warp pass that also produces the proxy (fit), the ROI (1:1) or a tile (export)
   ▼
 ┌─ Scene-referred · linear Rec.2020 (D65) ──────────────────────────────────────┐
 │ camera matrix → white balance (CAT) → exposure → noise reduction →            │
 │ highlights/shadows (local) → clarity/texture → (Later: dehaze, local masks)   │
 └───────────────────────────────────────────────────────────────────────────────┘
   ▼
 [Tone map / profile] filmic sigmoid base curve with contrast, whites, blacks
   ▼
 ┌─ Display-referred · perceptual (OkLab for color ops) ─────────────────────────┐
 │ tone curve → HSL / color mixer → vibrance / saturation → B&W mix →            │
 │ (color grading) → sharpening → post-crop vignette → grain                     │
 └───────────────────────────────────────────────────────────────────────────────┘
   ▼
 [Output] gamut mapping → display transform (sRGB now, monitor ICC later)
          or export color space + encoding
   ▼
 histogram (GPU) · clipping overlay · before/after split
```

### 6.2 Stages and caching

| # | Stage | Space | Ops (MVP) | Runs on | Cache key adds… |
|---|---|---|---|---|---|
| 0 | Raw prep | Camera RGB, linear | Decode, levels, reference WB, highlight clip, demosaic | CPU (LibRaw) | File + raw-prep version |
| 1 | Geometry | Camera RGB, linear | Orientation, rotate, crop, resample to proxy/ROI/tile | GPU | Geometry params + view (scale, ROI) |
| 2 | Scene | Linear Rec.2020 | Camera matrix, WB, exposure, NR, highlights/shadows, clarity | GPU | Stage params |
| 3 | Tone map | Linear to display-referred | Profile base curve, contrast, whites, blacks | GPU | Stage params |
| 4 | Display | Perceptual | Tone curve, HSL, vibrance/saturation, B&W, sharpening, vignette, grain | GPU | Stage params |
| 5 | Output | Display or export space | Gamut map, transfer function, dither; histogram, clipping | GPU | Output profile |

Each stage's output texture is cached, and a slider change re-runs only its own stage and the ones after it. Dragging Vibrance never re-runs noise reduction.

### 6.3 Raw prep

The MVP uses LibRaw's own processing, configured as a neutral "scanner":
- `output_color = 0` (raw camera space), `gamm = {1, 1}` (linear), `no_auto_bright = 1`, `output_bps = 16`, `user_mul = pre_mul` (D65 reference multipliers), `highlight = 0` (clip, so clipped highlights stay neutral instead of magenta), demosaic `user_qual` = AHD or DCB. LibRaw also handles X-Trans.
- The camera matrix comes from LibRaw's `cam_xyz` (the Adobe DNG ColorMatrix for D65), normalized so that the reference-WB camera white maps to D65.
- As-shot WB goes `cam_mul` → camera neutral → XYZ → xy → Temp/Tint, so the UI can show "As Shot 5230 K / +7".
- Cost is about 1–2 s for 24–45 MP on CPU. The result goes into a RAM LRU of 2–3 decoded images, and Develop prefetches neighboring images (MVP+). The L2 or embedded preview shows immediately with a "Loading…" badge.
- **Later:** our own GPU demosaic (RCD for Bayer, Markesteijn for X-Trans), our own highlight reconstruction and raw-level CA correction. These sit behind the same `RawPrep` trait, so the change is a swap.

**Why demosaic at a fixed D65 white balance:** if white balance were applied before demosaicing, every Temp/Tint change would force a new demosaic, costing about 1 s per slider move. Instead the image is demosaiced once at a fixed reference white, and the actual white balance is applied later as a chromatic adaptation (CAT16) on linear data. This is the approach of darktable's modern workflow. It is also colorimetrically better under unusual illuminants.

### 6.4 White balance and color

- Temp/Tint to xy uses Robertson isotemperature lines, as in the DNG SDK. Temp spans 2000–50000 K, and the slider is linear in mireds (like Lightroom), which gives fine control at low K. Tint spans −150 to +150.
- White balance is applied as a CAT16 chromatic adaptation (D4) from the chosen illuminant to D65. Bradford stays available behind a debug switch for comparisons on the golden set.
- **Presets:** As Shot, Auto, Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash, Custom.
- **Eyedropper (`W`):** takes a 5×5 median at the click and solves for the Temp/Tint that makes it neutral.
- **Auto WB:** a robust gray-world estimate on the linear proxy, excluding clipped and very dark pixels.
- **Rendered files (JPEG/TIFF):** Temp/Tint become relative (−100 to +100) as in Lightroom, applied as a CAT around the source white.

### 6.5 Tone

The goal is Lightroom-like behavior for Exposure, Contrast, Highlights, Shadows, Whites and Blacks on scene-referred data, with no halos and the same look at every zoom level.

- **Exposure:** linear values are multiplied by 2^EV, from −5 to +5.
- **Highlights / Shadows (local):**
  1. Compute log-luminance `L`, then a base layer `B` by edge-aware smoothing of `L` (guided filter), and a detail layer `D = L − B`.
  2. Apply a smooth gain curve to `B`: Shadows raises low values, Highlights compresses high values.
  3. Recombine `L' = B' + D` and scale RGB by `2^(L' − L)`, which preserves hue and saturation.
- **Scale invariance:** filter radii are fractions of the image diagonal. The base layer is computed on a fixed-size downscale (~1024 px) of the *whole* frame, then upsampled. Fit-to-screen, 1:1 and export therefore look the same, which avoids a classic pitfall.
- **Clarity / Texture:** the same decomposition at medium and small scales, boosting the detail layer with a midtone weighting. Clarity is MVP; Texture is MVP+.
- **Tone mapper (the Profile's base curve):** a parametric filmic sigmoid in log space around middle gray (0.18). Contrast sets the slope, Whites the shoulder and white point, Blacks the toe and black point. It applies to a luminance norm with a controlled "path to white" (chroma compression near white), which avoids the hue shifts of per-channel curves (D8). Per-channel application stays behind a debug switch for the M3 image-quality comparisons.
  - Profiles are parameter sets: *Standard* (default: moderate contrast, a little saturation), *Neutral* (flat), *Linear* (no curve), *Monochrome*.
  - For rendered files, the base curve is identity, so default settings reproduce the file unchanged.
- **Auto Tone:** uses the luminance histogram of the proxy. It sets exposure so the median lands near a target, whites and blacks to about 0.1% clipping, and shadows/highlights from the histogram's skew. The results are ordinary slider values the user can keep editing.

### 6.6 Color adjustments (display-referred)

- Color ops work in OkLab/OkLCh, which has perceptually uniform hues and makes hue-band selection clean.
- **HSL / Color Mixer:** 8 bands (Red, Orange, Yellow, Green, Aqua, Blue, Purple, Magenta) with smooth overlapping weights (raised cosine on hue, faded out at low chroma). Each band has a hue shift, chroma scale and lightness shift.
- **Vibrance and Saturation:** Vibrance boosts chroma weighted by (1 − current chroma), with less effect on skin-tone hues. Saturation scales chroma uniformly.
- **B&W:** a treatment switch. Luminance comes from a weighted mix of the same 8 bands, and the B&W mixer sliders replace the HSL panel.
- **Tone curve:** a parametric curve (Highlights, Lights, Darks, Shadows, plus 3 region split points) and a point curve (RGB composite plus R, G and B). Interpolation is monotone cubic (Fritsch–Carlson, no overshoot), baked into a 4096-entry 1D LUT texture.
- **Color grading** (MVP+): shadow, midtone, highlight and global wheels, plus blending and balance.

### 6.7 Detail

- **Sharpening:** an unsharp mask on luminance. Sliders: Amount, Radius (0.5–3 px), Detail (halo suppression) and Masking (an edge mask from gradient magnitude). Alt-drag mask preview is MVP+.
- **Noise reduction:** runs in the scene stage.
  - Luminance: edge-aware bilateral or guided filter on variance-stabilized luma, with Detail and Contrast sliders.
  - Color: chroma smoothing guided by luma, with Detail and Smoothness sliders.
- Both are pixel-scale ops, so they are exact only at 1:1 or on export. At fit they are approximated with scaled radii, and the panel shows a "zoom to 1:1 for an accurate preview" hint, as Lightroom does.
- **Later:** wavelet or NLM denoising, profiled noise models, AI denoise.

### 6.8 Geometry

- **Params:** orientation (90° steps and flips), straighten angle (±45°), crop rect (normalized, in the rotated frame) and aspect lock.
- **Implementation:** one GPU warp pass maps each output pixel through the inverse transform and samples the source bicubically, using prefiltered mip levels when downscaling.
- While the crop tool is active, the pipeline renders uncropped with an overlay (like Lightroom), and "constrain to image" keeps the crop inside the rotated frame.
- **Later:** lens profiles through lensfun (distortion, TCA, vignetting) and manual perspective/Upright. Both fit into the same warp pass.

### 6.9 Output and color management

- **Working to target:** a matrix transform, then gamut compression in OkLCh (MVP: simple chroma reduction toward the gamut boundary; better mapping Later), the transfer function, and dithering for 8-bit output.
- **Display:** the MVP assumes an sRGB monitor. The display transform is built as a **3D LUT (from lcms2) from day one**, so monitor ICC support later (colord, or Wayland `wp_color_management_v1`) is just a different LUT.
- **Export:** exact matrix and transfer math (no LUT), with an embedded ICC profile: sRGB, Display P3, an Adobe RGB (1998)-compatible profile, or ProPhoto.
- **Histogram:** computed on the GPU (atomic bins, 256 × RGB + luma) from the proxy's output and read back asynchronously. One frame of latency is acceptable.
- **Clipping indicators (`J`):** a shader overlay shows red where any output channel is ≥ 1 and blue where any is ≤ 0.

### 6.10 Performance strategy

1. **Proxy editing:** interactive renders happen at viewport resolution (at most about 3–4 MP), never full resolution.
2. **Stage cache:** only the stages downstream of the changed slider are recomputed.
3. **Latest-wins scheduling:** parameter updates coalesce while a slider is dragged. At most one render is in flight, and stale ones are dropped.
4. **ROI rendering at 1:1:** only the visible viewport (plus an apron for neighborhood ops) is rendered, and panning renders tiles incrementally.
5. **Tiled export:** images larger than the device limits (`max_texture_dimension_2d`, VRAM budget) render in overlapping tiles. Global analyses, such as the local-tone base layer and histograms, are computed once on a whole-frame downscale and shared by all tiles.
6. **Precision:** intermediates are stored as `rgba16float` where precision allows; accumulations use f32. f16 support is feature-detected, with an f32 fallback.
7. **Prefetch:** Develop pre-decodes the next and previous images (MVP+).

### 6.11 Process versions and determinism

- Every op has a `VERSION`. An old edit renders with the version it was made with: old kernels are kept, or params are migrated by an explicit, tested migration.
- Golden-image tests pin the look of each version (§13).

### 6.12 Designed now, built later: local adjustments

Lightroom's local adjustments are a subset of the global adjustments applied through a mask. To keep them cheap to add later:
- Every WGSL kernel takes an optional mask texture binding (default 1.0), and blends its result with its input by that mask.
- `EditParams.local` will hold `{ mask: MaskDef, params: {op_id → params} }` entries. `MaskDef` covers linear and radial gradients, brush strokes, and luminance/color range.
- Masks rasterize on the GPU at the resolution of the current view, in the same geometry frame as the crop.

---

## 7. Library module spec

### 7.1 Import

| Feature | Details | Tier |
|---|---|---|
| Import dialog | Full-window overlay: sources on the left (portal picker, recent folders), candidate grid in the middle with checkboxes and an already-imported dimming, options on the right | MVP |
| Add | Reference files in place | MVP |
| Copy | To a destination with a folder template (`{YYYY}/{YYYY-MM-DD}`), hash-verified before registering, optional rename template | MVP |
| Move | | Later |
| Duplicate detection | Skip suspected duplicates (filename + size + capture time, then quick hash) | MVP |
| Apply during import | Keywords and metadata (creator, copyright): MVP. Develop preset: MVP+. | MVP / MVP+ |
| Preview building | Embedded (fast, default) or standard rendered | MVP |
| Read existing XMP sidecars | Ratings, labels, keywords and titles from other apps | MVP |
| RAW+JPEG pairs | MVP imports them as separate photos. JPEG-as-sidecar mode is MVP+. | MVP / MVP+ |
| Import progress | Cancellable, runs in the background, and photos appear in the grid as they land | MVP |
| Card detection (udisks2, DCIM), eject after import, watched folders | | Later |

### 7.2 Source panels (left)

| Panel | Details | Tier |
|---|---|---|
| Navigator | Preview of the photo under the cursor or selected | MVP+ |
| Catalog | All Photographs, Previous Import, Quick Collection | MVP |
| Folders | Tree with counts, Synchronize Folder (finds new and missing files), Show in File Manager, missing badge. Rename and move of folders are Later. | MVP |
| Collections | Regular collections, collection sets, smart collections (rule editor built on the `Criterion` registry). Target collection is MVP+. | MVP |
| Keyword List | Hierarchy with counts; clicking a keyword filters on it | MVP |

### 7.3 Views

| Feature | Details | Tier |
|---|---|---|
| Grid (`G`) | Virtualized (50k+ items), thumbnail-size slider, badges (rating, flag, label, edited, virtual copy, missing), optional cell extras (filename, dimensions, index) | MVP |
| Loupe (`E`) | Fit and 1:1 zoom, pan, previous/next | MVP |
| Loupe info overlay (`I`) | | MVP+ |
| Filmstrip | Shared selection, source indicator, thumbnail size | MVP |
| Compare (`C`) | Select and candidate, synced zoom | MVP+ |
| Survey (`N`), lights out (`L`), secondary window | | Later |
| Full-screen preview (`F`) | | MVP+ |

### 7.4 Selection and organizing

| Feature | Details | Tier |
|---|---|---|
| Selection | Click, Shift for a range, Ctrl to toggle, `Ctrl+A` / `Ctrl+D`; an active photo within the selection | MVP |
| Ratings, flags, color labels | Keyboard, toolbar and context menu; apply to the whole selection | MVP |
| Auto-advance | Shift + rating/flag key applies and moves to the next photo. Caps Lock toggle is MVP+. | MVP |
| Rotate 90° | Non-destructive (`user_orientation`) | MVP |
| Virtual copies | `Ctrl+'`; a new `photos` row sharing the same file | MVP |
| Sort | Capture time, import order, edit time, filename, rating, flag, label, file type. Custom drag order in regular collections is MVP+. | MVP |
| Remove / delete | Remove from catalog, or move the file to the freedesktop trash (with confirmation) | MVP |
| Missing files | Badge, plus "Locate…" to relink | MVP / MVP+ |
| Stacking, painter tool | | Later |

### 7.5 Library filter bar (`\`)

| Feature | Details | Tier |
|---|---|---|
| Text | Filename, title, caption and keywords via FTS5; contains, all, any, starts with | MVP |
| Attribute | Flag, rating (≥ / = / ≤), color label, edited or unedited, virtual copy | MVP |
| Metadata columns | Drill-down columns with counts. Date hierarchy, camera and lens are MVP; ISO, focal length, aperture and file type are MVP+. | MVP / MVP+ |
| Saved filter presets | | MVP+ |

### 7.6 Metadata and keywords (right)

| Feature | Details | Tier |
|---|---|---|
| Histogram | Of the L2 preview | MVP+ |
| Metadata panel | EXIF view (camera, lens, exposure, ISO, focal length, flash, dimensions, capture time, GPS) and editable IPTC core (title, caption, creator, copyright). Batch editing shows `<mixed>`. | MVP |
| Metadata presets | Creator and copyright bundles | MVP+ |
| Keywording | Text entry with autocomplete, recently used and suggestions, apply to selection. Keyword sets are MVP+. | MVP |
| Hierarchical keywords | `Parent > Child`, include-on-export flag. Synonyms are Later. | MVP |
| Save to XMP | `Ctrl+S`, plus an auto-write preference; see §5.3 | MVP |
| Edit capture time, Quick Develop panel | | Later |

---

## 8. Develop module spec

### 8.1 Left panel

| Panel | Details | Tier |
|---|---|---|
| Navigator | Mini view, zoom presets (Fit, 1:1, 2:1), click to jump, viewport rectangle. Fill is MVP+. | MVP |
| Presets | Built-in and user folders; apply; create from current with group checkboxes; update, rename, delete; import/export as files. Hover preview is MVP+. | MVP |
| Snapshots | Named full states, click to apply, `Ctrl+N` | MVP |
| History | Steps with values, click to revert, clear. Hover preview is MVP+. | MVP |
| Copy… / Paste | Group-selection dialog | MVP |

### 8.2 Right panel

| Panel / control | Details | Tier |
|---|---|---|
| Histogram | RGB overlay, clipping triangles (toggle), EXIF line (ISO, focal length, aperture, shutter). Dragging on the histogram to adjust is Later. | MVP |
| Tool strip | Crop (`R`) is MVP. Spot removal (`Q`), red-eye and masking are Later; the buttons are placeholders. | MVP / Later |
| Basic: Treatment | Color / B&W (`V`) | MVP |
| Basic: Profile | Standard, Neutral, Linear, Monochrome. Profile amount is MVP+. `.cube` creative profiles and DCP camera profiles are Later. | MVP |
| Basic: White Balance | Preset menu, eyedropper (`W`), Temp, Tint | MVP |
| Basic: Tone | Auto, Exposure, Contrast, Highlights, Shadows, Whites, Blacks | MVP |
| Basic: Presence | Clarity, Vibrance and Saturation are MVP; Texture and Dehaze are MVP+. | MVP / MVP+ |
| Tone Curve | Parametric and point, with RGB/R/G/B channels. Targeted adjustment (drag on image) is MVP+. | MVP |
| HSL / B&W | 8 bands × Hue, Sat, Lum, plus an All tab. The B&W mixer shows under B&W treatment. The per-band "Color" view and targeted adjustment are MVP+. | MVP |
| Color Grading | Three-way plus global, blending, balance | MVP+ |
| Detail | Sharpening (amount, radius, detail, masking); NR luminance (amount, detail, contrast) and color (amount, detail, smoothness) | MVP |
| Lens Corrections | lensfun profile and manual distortion/vignetting are MVP+. CA removal and defringe are Later. | MVP+ / Later |
| Transform (Upright, perspective) | | Later |
| Effects | Post-crop vignette (amount, midpoint, roundness, feather, highlights) is MVP; grain is MVP+. | MVP / MVP+ |
| Calibration | | Later |
| Previous / Reset | Previous pastes the previous photo's settings. Reset clears everything; double-clicking a panel title or slider label resets that part. | MVP |

### 8.3 Canvas and toolbar

| Feature | Details | Tier |
|---|---|---|
| Zoom and pan | Fit, 1:1, 2:1; `Z`/Space toggles at the cursor; Space-drag pans | MVP |
| Before/After | `\` toggles. Side-by-side and split views (`Y`) are MVP+. | MVP |
| Clipping overlay | `J` | MVP |
| Crop tool | Aspect presets (original, 1:1, 4:5, 3:2, 16:9, custom, free), lock (`A`, MVP+), straighten slider, level line, flip, rotate 90°, `X` swaps orientation, overlays (thirds, grid, golden ratio) cycled with `O` | MVP |
| WB eyedropper | Magnified loupe while picking; shows the Temp/Tint it would set | MVP |
| "Loading…" and "zoom to 1:1" hints | | MVP |
| Soft proofing | | Later |

### 8.4 Workflow

| Feature | Details | Tier |
|---|---|---|
| Copy and paste settings | `Ctrl+Shift+C` / `Ctrl+Shift+V`, with group checkboxes; `Ctrl+Alt+V` pastes from the previous photo | MVP |
| Sync settings | Apply the active photo's settings to the selection, with group checkboxes. Auto Sync is MVP+. | MVP / MVP+ |
| Virtual copies | Shared with Library | MVP |
| Auto Tone / Auto WB | `Ctrl+U` / `Ctrl+Shift+U` | MVP |
| Develop settings in XMP | Optional, via the `archroom:` namespace | MVP+ |
| Edit in an external editor (GIMP, Krita) | Export a TIFF, open the editor, re-import the result | Later |

---

## 9. Export (shared by both modules)

| Feature | Details | Tier |
|---|---|---|
| Export dialog and presets | Built-ins ("JPEG sRGB full size", "JPEG 2048 px for web", "TIFF 16-bit ProPhoto") plus user presets | MVP |
| Destination | Chosen folder, same folder as the original, or a subfolder. Conflict policy: ask, unique name, overwrite or skip. | MVP |
| File naming | Template tokens: `{filename}`, `{seq:4}`, `{date:%Y%m%d}`, `{title}`, `{custom}` | MVP |
| Formats | JPEG (quality), TIFF 8/16-bit (none, LZW or ZIP), PNG 8/16-bit, plus "Original" (copy) at MVP+. AVIF, JXL, WebP and DNG are Later. | MVP |
| Color space | sRGB, Display P3, Adobe RGB (1998)-compatible, ProPhoto; ICC embedded; dithering for 8-bit | MVP |
| Resize | Long edge, short edge, W×H, megapixels or percent; "don't enlarge"; PPI metadata | MVP |
| Output sharpening | Screen, matte or glossy × low, standard or high | MVP+ |
| Metadata | All, copyright only, or none; option to remove location; keywords with or without hierarchy | MVP |
| Watermark, JPEG file-size limit | | Later |
| Post-export action | Nothing, or show in file manager | MVP |
| Batch | Background job with progress and cancel; the user can keep working | MVP |
| Export with Previous | `Ctrl+Alt+Shift+E` | MVP+ |

---

## 10. UI and UX

### 10.1 Layouts

Library:

```
+----------------------------------------------------------------------------------------+
| Archroom                                    [ Library ]   Develop         (2 jobs)     |
+--------------------+----------------------------------------------+--------------------+
| > Navigator        | Text [_____]  Rating >= ***  Flag P  Meta v  | v Histogram        |
| v Catalog          | +------+ +------+ +------+ +------+ +------+ |   [ histogram ]    |
|   All Photographs  | |      | |      | |      | |      | |      | | v Quick Info       |
|   Previous Import  | |      | |      | |      | |      | |      | | v Keywording       |
|   Quick Coll. (4)  | +------+ +------+ +------+ +------+ +------+ |   [add keyword...] |
| v Folders          |   ***  P    **        X     *****    red     | v Keyword List     |
|   v /photos/2026   | +------+ +------+ +------+ +------+ +------+ | v Metadata         |
|       09-20  (312) | |      | |      | |      | |      | |      | |   Title   [     ]  |
|       09-27  (148) | |      | |      | |      | |      | |      | |   Caption [     ]  |
| v Collections      | +------+ +------+ +------+ +------+ +------+ |   Canon R6 II      |
|   > Trips          |                                              |   35mm f/1.8       |
|   Best (smart)     |                                              |   1/250 ISO 400    |
| [Import...]        |                                              | [Export...]        |
+--------------------+----------------------------------------------+--------------------+
| [Grid] [Loupe] [Compare]   Sort: Capture time v   Rate ***  Flag P X   Size --o--      |
+----------------------------------------------------------------------------------------+
| 09-20 / 312 photos / 3 selected                                                        |
| [ ][ ][#][#][#][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ]         |
+----------------------------------------------------------------------------------------+
```

Develop:

```
+----------------------------------------------------------------------------------------+
| Archroom                                      Library   [ Develop ]       (1 job)      |
+--------------------+--------------------------------------+----------------------------+
| v Navigator        |                                      | v Histogram                |
|   [ mini view ]    |                                      |   [ histogram ]  ^      ^  |
|   Fit Fill 1:1 2:1 |                                      |   ISO 400  35mm  f/1.8     |
| v Presets          |                                      | [Crop] [Spot] [Mask]       |
|   > Archroom B&W   |                                      | v Basic                    |
|   > Archroom Color |                                      |   Treatment  Color | B&W   |
|   > User Presets   |     (image canvas: zoom/pan,         |   Profile    Standard v    |
| v Snapshots        |      crop & tool overlays,           |   WB  As Shot v      [W]   |
|   Final v1         |      before/after split)             |   Temp      ---o---  5230  |
| v History          |                                      |   Tint      ---o---    +7  |
|   Shadows +40      |                                      |   Exposure  ----o-- +0.35  |
|   Exposure +0.35   |                                      |   Contrast / Highlights /  |
|   Import           |                                      |   Shadows/Whites/Blacks    |
|                    |                                      |   Clarity  Vibrance  Sat.  |
| [Copy...][Paste]   |                                      | > Tone Curve   > HSL / B&W |
|                    |                                      | > Detail       > Effects   |
|                    |                                      | [Previous]       [Reset]   |
+--------------------+--------------------------------------+----------------------------+
| [Y|Y] Before/After    Zoom: Fit v    Clipping [J]    Crop overlay: Thirds v            |
+----------------------------------------------------------------------------------------+
| 09-20 / 312 photos / 1 selected                                                        |
| [ ][ ][#][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ][ ]         |
+----------------------------------------------------------------------------------------+
```

The Spot and Mask buttons are placeholders for post-MVP tools.

### 10.2 Interaction details

- **Panels:** collapsible sections, with a solo mode (Alt-click, MVP+). `Tab` toggles the side panels and `Shift+Tab` toggles all panels. Panel widths and open states are remembered per module.
- **Sliders**, Lightroom-style:
  - Label on the left, value on the right.
  - Drag to change; double-click the label to reset; click the value to type.
  - Mouse wheel moves by a fine step, Shift+wheel by a coarse one.
  - Gradient tracks for Temp, Tint and the HSL bands.
  - One drag creates one history step.
  - `,` / `.` move between sliders and `+` / `−` adjust the focused one (MVP+).
- **Center background:** black, dark gray, medium gray or white (a preference). It matters when judging tonality.
- **Theme:** neutral dark grays, low-saturation chrome, one accent color, a bundled Inter font, HiDPI-aware.
- **Activity:** the top bar shows background jobs with progress. Clicking it opens the job list with cancel buttons.
- **Errors:** failed decodes show the reason on the thumbnail, never a chain of modal dialogs. Missing files get a `!` badge.

### 10.3 Keyboard shortcuts (Lightroom-compatible defaults; a remappable keymap is Later)

| Keys | Action | Scope | Tier |
|---|---|---|---|
| `G` / `E` / `D` | Grid / Loupe / Develop | Global | MVP |
| `C` | Compare | Library | MVP+ |
| `Tab` / `Shift+Tab` | Toggle side panels / all panels | Global | MVP |
| `F` | Full-screen preview | Global | MVP+ |
| `Ctrl+Shift+I` / `Ctrl+Shift+E` | Import / Export | Global | MVP |
| `←` / `→` | Previous / next photo | Global | MVP |
| `Ctrl+A` / `Ctrl+D` | Select all / none | Global | MVP |
| `0`–`5`, `[` / `]` | Set rating, decrease / increase rating | Global | MVP |
| `6` `7` `8` `9` | Red / yellow / green / blue label | Global | MVP |
| `P` / `X` / `U` | Pick / reject / unflag | Global | MVP |
| Shift + rating/flag key | Apply and advance to the next photo | Global | MVP |
| `B` | Toggle Quick Collection | Global | MVP |
| `Ctrl+'` | Create virtual copy | Global | MVP |
| `Ctrl+[` / `Ctrl+]` | Rotate left / right | Global | MVP |
| `Z` / `Space` | Toggle Fit and 1:1 | Loupe, Develop | MVP |
| `Ctrl+Z` / `Ctrl+Y` (`Ctrl+Shift+Z`) | Undo / redo | Global | MVP |
| `Ctrl+S` | Save metadata to XMP | Global | MVP |
| `Delete` | Remove / move to trash (dialog) | Library | MVP |
| `Ctrl+F`, `\` | Focus text filter, toggle filter bar | Library | MVP |
| `Ctrl+K` | Focus keyword entry | Library | MVP |
| `R` / `W` / `J` | Crop tool / WB eyedropper / clipping overlay | Develop | MVP |
| `\` / `Y` | Before/after toggle / side by side | Develop | MVP / MVP+ |
| `O`, `X` | Cycle crop overlay, swap crop orientation | Crop tool | MVP |
| `V` | Toggle B&W treatment | Develop | MVP |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` / `Ctrl+Alt+V` | Copy / paste / paste from previous | Develop | MVP |
| `Ctrl+Shift+S` | Sync settings | Develop | MVP |
| `Ctrl+Shift+R` | Reset all settings | Develop | MVP |
| `Ctrl+U` / `Ctrl+Shift+U` | Auto Tone / Auto WB | Develop | MVP |
| `Ctrl+N` / `Ctrl+Shift+N` | New snapshot / new preset | Develop | MVP |

---

## 11. Roadmap

```
M0 ──┬── M1 Import & browse ── M2 Organize ─────────┐
     │                                              ├── M5 Export, hardening → v0.1
     └── M3 Develop foundation ── M4 Develop tools ─┘
         (engine work starts right after M0 through the CLI;
          app integration needs M1's catalog and grid)
```

### M0: Foundations and tech spike

Prove the stack end to end and lay down the skeleton.
- [ ] `git init`; workspace; crate skeletons; shared lints; `justfile` (`run`, `test`, `lint`, `bench`, `fixtures`, `bless`); `cargo deny` config; `xtask` dependency-rule check
- [ ] `LICENSE` (GPL-3.0-or-later); record D1–D9 as ADRs in `docs/adr/`
- [ ] **Spike:** decode a CR3, a NEF and a RAF with LibRaw → linear f32 → wgpu upload → WGSL exposure and sRGB output → shown in an egui paint callback with a live slider. Measure latency on the RX 7900 and the UHD 630. This validates D1; the fallback if it fails is in §15.
- [ ] Check exiv2 reads CR3 on Arch; install `libgexiv2`, `vulkan-intel` and `vulkan-swrast`
- [ ] `core`: IDs, errors, settings (TOML + XDG), tracing, event bus
- [ ] `catalog`: create/open, migration v1, WAL, online backup
- [ ] `jobs`: priority scheduler, cancellation, progress events
- [ ] `app` shell: window, dark theme, top bar with the module switcher, panel layout (left, right, toolbar, filmstrip, `Tab`/`Shift+Tab`), `Module` trait and registry, activity indicator
- [ ] `cli` skeleton

**Exit:** the app creates and opens a catalog and switches modules; the spike renders a raw with a live slider within the §12 budget.

### M1: Import and browse (Library)

- [ ] `libraw-sys` plus a safe wrapper: open, metadata, embedded thumbnail, process
- [ ] Metadata through exiv2 (`MetadataReader`) normalized into `ExifSummary`; orientation handling
- [ ] Decoders: raw (LibRaw), JPEG/PNG/TIFF with ICC
- [ ] Import dialog (§7.1): portal source picker, recursive scan, candidate grid, Add and Copy modes, destination template, duplicate skip, keywords and metadata on import, reading existing XMP
- [ ] Import as jobs: scan → metadata → batched inserts → L0/L1 previews, with progress and cancel; "Previous Import"
- [ ] Preview cache: levels, `previews.db` index, invalidation, eviction budget
- [ ] Grid: virtualized, thumbnail size, selection model, badges, sort
- [ ] Loupe (fit/1:1, pan, previous/next) and filmstrip
- [ ] Catalog panel and Folders panel (tree, counts, Synchronize, Show in File Manager)

**Exit:** import 5,000 mixed files; first thumbnails within 2 s of starting; the grid scrolls at 60 fps on a 4K screen; a restart keeps everything.

### M2: Organize (Library)

- [ ] `Command` bus with Library undo/redo
- [ ] Ratings, flags and labels (keys, toolbar, badges), auto-advance, rotate
- [ ] Metadata panel: EXIF view, editable IPTC core, batch edit showing `<mixed>`
- [ ] Keywording and Keyword List panels: hierarchy, autocomplete, counts
- [ ] Collections: regular, sets, Quick Collection, and smart collections with the `Criterion` registry and a rule editor
- [ ] Filter bar: text (FTS5), attributes, metadata columns (date, camera, lens)
- [ ] XMP sidecars: write (`Ctrl+S` and the auto option), read on import and sync, foreign fields preserved
- [ ] Remove from catalog and move to trash; missing-file detection and badge

**Exit:** a 500-photo shoot can be culled and keyworded entirely from the keyboard, and its ratings and keywords show up in darktable/digiKam through XMP.

### M3: Develop foundation

- [ ] `color`: matrices, xy/XYZ, Temp/Tint (Robertson), CAT16 and Bradford, transfer functions, lcms2 wrapper, 3D LUT builder
- [ ] Raw prep (§6.3), the rendered-image path, and a RAM cache of decoded images
- [ ] Engine: `Op` trait and registry, `EditParams` serialization and hashing, stage graph and cache, texture pool, geometry pass producing the proxy or ROI, latest-wins render loop, GPU histogram
- [ ] Ops: white balance, exposure, tone mapper and profiles (contrast, whites, blacks), highlights/shadows (local), vibrance, saturation, output transform
- [ ] Develop module: canvas (zoom, pan, loading state), Basic panel with the `ui-kit` slider, histogram, clipping (`J`), before/after (`\`), eyedropper (`W`), Auto Tone and Auto WB
- [ ] History (persisted and coalesced), undo/redo, reset; copy/paste with the group dialog; Previous
- [ ] After an edit, L1/L2 re-render in the background and the grid shows an "edited" badge
- [ ] `archroom-cli render` and the golden-image suite running in CI

**Exit:** slider latency within budget; edits persist and render identically after a restart; the golden suite is green.

### M4: Develop tools

- [ ] Tone curve (parametric and point, channels) with its curve-editor widget
- [ ] HSL and the B&W mixer; treatment switch (`V`)
- [ ] Clarity (Texture and Dehaze if time allows)
- [ ] Detail: sharpening, luminance and color NR, the "zoom to 1:1" hint
- [ ] Crop and straighten as the first `CanvasTool`: aspects, straighten, level line, overlays, flip/rotate
- [ ] Effects: post-crop vignette (and grain as MVP+)
- [ ] Navigator; presets (built-ins, user presets, partial groups, file import/export); snapshots; virtual copies; sync settings
- [ ] **Image-quality tuning pass** on the golden set against reference renders from darktable and Lightroom

**Exit:** a typical raw goes from import to a finished, pleasing result without leaving the app.

### M5: Export, hardening, v0.1

- [ ] Export dialog and presets (§9): `Encoder` registry (JPEG, TIFF, PNG), ICC embedding, metadata options, naming templates, resize, conflict policy
- [ ] Full-resolution tiled render path; background batch export with progress and cancel
- [ ] L3 1:1 preview cache and Develop prefetch (MVP+)
- [ ] Robustness: integrity check on open, scheduled backups, crash-safe writes, clear error messages
- [ ] Preferences dialog: catalog path, preview size and quality, cache budgets, XMP behavior, GPU selection, center background
- [ ] Performance pass against §12 on the RX 7900 and the UHD 630; memory caps
- [ ] Packaging: PKGBUILD, `.desktop` file and icon; README and a shortcut cheat sheet

**Exit:** the definition of done below is met, and `v0.1.0` is tagged.

### 11.1 MVP definition of done

1. Import 2,000 mixed raws (CR3, NEF, ARW, RAF, DNG) plus JPEGs, and have them browsable within a minute.
2. Cull, rate, label, keyword and build collections entirely from the keyboard.
3. Edit with Basic, Tone Curve, HSL/B&W, Detail, Crop and Vignette. Use presets, snapshots, history and virtual copies. Sync an edit across 50 photos.
4. Export 100 photos as 2048 px sRGB JPEGs in the background while continuing to edit.
5. Quit and relaunch: everything is where it was, and digiKam and darktable can read the XMP sidecars.
6. No original file was modified. An automated test checks this by comparing hashes before and after a full session.

### 11.2 After v0.1 (ordered backlog)

1. **Local adjustments:** linear and radial gradients, brush, luminance/color range masks (§6.12)
2. **Spot removal:** clone and heal
3. Lens corrections (lensfun) and CA removal
4. Color grading, calibration, and Texture/Dehaze if they slipped
5. Monitor color management (ICC via colord or Wayland) and soft proofing
6. Compare and Survey views, stacking, painter tool, secondary window
7. Our own GPU demosaic (RCD, Markesteijn) and highlight reconstruction
8. HEIF, AVIF and JXL import/export; DNG export
9. Round-trip editing in GIMP or Krita
10. Importing Lightroom XMP develop settings (an approximate `crs:` mapping)
11. Plugin API (Lua or WASM) over `Command`, `Criterion`, `Encoder` and `Op`
12. Map module (GPS), then Print/Slideshow if ever

---

## 12. Performance budgets

| Scenario | RX 7900 (reference) | UHD 630 (floor) |
|---|---|---|
| Cold start to grid, 50k catalog | < 2 s | < 3 s |
| Grid scrolling, 4K screen, 200 px thumbnails | 60 fps | 60 fps |
| Cached thumbnail appears | < 50 ms | < 100 ms |
| Import 1,000 raws (Add, embedded previews), fully browsable | < 60 s | < 60 s (CPU- and disk-bound) |
| Open a 24 MP raw in Develop until editable (preview shown instantly) | < 1.5 s | < 2.5 s |
| Slider change to updated frame, fit on a 4K screen | < 16 ms | < 50 ms |
| 1:1 viewport render | < 50 ms | < 200 ms |
| Export a 24 MP raw to a full-size JPEG | < 1.5 s | < 4 s |
| Memory (RSS) while browsing a 50k catalog | < 1.5 GB | < 1.5 GB |

---

## 13. Testing and quality

- **Unit tests:**
  - Color math: matrix inverses, D65 round trips, Temp/Tint to xy against DNG SDK reference values, CAT is identity at D65.
  - Spline monotonicity, filename templates, filter-to-SQL generation.
  - Params serialization and version migrations.
- **Property tests (`proptest`):**
  - Identity params give a neutral render.
  - +1 EV doubles linear values before the tone mapper.
  - No op produces NaN or Inf for random params and extreme inputs (all black, all white, saturated primaries).
- **Golden images:**
  - About 30 raws from raw.pixls.us (CC0), covering Canon CR2/CR3, Nikon NEF, Sony ARW, Fuji RAF (X-Trans), Panasonic RW2, OM/Olympus ORF, Pentax and DNG (including phone DNGs), plus JPEG/TIFF 8/16-bit with ICC.
  - Rendered through `archroom-cli` with a fixed set of param files and compared with ΔE2000 (mean < 0.5, p99 < 2).
  - Updating references is an explicit `just bless`.
  - A dedicated test checks that a proxy render matches a downscaled full-resolution render, which guards scale invariance.
- **GPU in CI:** wgpu on lavapipe (Mesa software Vulkan). Golden tolerances absorb backend float differences.
- **Catalog tests:** migrations tested from a fixture of every past schema version; query tests on generated 50k-row catalogs to catch performance regressions.
- **UI tests:** `egui_kittest` for key flows such as switching modules, rating from the keyboard and resetting a slider.
- **Safety test:** hashes of every original are compared before and after a scripted session (DoD item 6).
- **Benchmarks:** `criterion` for CPU paths (raw prep, resize, import scan). GPU timestamp queries per op appear in a debug overlay.
- **Checks on every change:** `cargo fmt --check`, `clippy -D warnings`, tests, `cargo deny` (licenses and advisories).

---

## 14. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| Image quality below expectations ("looks worse than Lightroom") | High: people judge the app by its default rendering | Scene-referred pipeline, careful tone mapper, side-by-side comparisons on the golden set, a dedicated tuning pass in M4 |
| Local ops look different at fit, 1:1 and export | High | Scale-invariant design (§6.5) and a golden test for it |
| egui limits (complex dialogs, drag and drop, accessibility, typography) | Medium | UI isolated in modules and `ui-kit`; engine and services are UI-agnostic, so the toolkit can be swapped |
| GPU driver variance (f16 support, NVIDIA proprietary driver, iGPU memory) | Medium | Feature detection with f32 fallback; test on RADV, ANV, NVIDIA and lavapipe |
| Huge images (100 MP+, panoramas) exceed texture limits or VRAM | Medium | Tiling designed in M3 and implemented in M5 |
| Raw coverage for new cameras | Medium | Keep LibRaw up to date; DNG as the universal fallback (via `dnglab`) |
| Catalog corruption or data loss | High | WAL, transactions, integrity check on open, automatic backups, XMP as a second copy |
| Scope creep ("just one more Lightroom feature") | High | Tier tags; nothing marked Later enters before v0.1 ships |
| Trademark and assets | Low–medium | Own name, icons and text; no Adobe assets, profiles or presets |

---

## 15. Decisions

All nine decisions were accepted on 2026-09-28, and each becomes an ADR in `docs/adr/` during M0. A decision is only reopened if its "Revisit if" condition happens.

| # | Topic | Decision | Revisit if |
|---|---|---|---|
| D1 | UI stack | **egui + eframe** (wgpu backend), sharing the GPU device with the engine | The M0 spike can't show the pipeline texture in egui within the §12 latency budget. The fallback is Qt 6/QML via cxx-qt; engine and services stay unchanged. |
| D2 | License | **GPL-3.0-or-later** | exiv2 is replaced by a permissively licensed metadata backend |
| D3 | Working space | **Linear Rec.2020 (D65)**, darktable's choice, rather than Lightroom's linear ProPhoto | — |
| D4 | Chromatic adaptation | **CAT16**; Bradford only behind a debug switch | Bradford clearly wins on the golden set in M3 |
| D5 | XMP sidecars | **Auto-write off by default**, as in Lightroom. **Adobe-style naming** `IMG_0001.xmp`, falling back to `IMG_0001.CR3.xmp` when a RAW+JPEG pair clashes. | — |
| D6 | Preview storage | **JPEG files plus a `previews.db` index**, stored next to the catalog | Loading the grid from files misses the §12 budget |
| D7 | Catalogs | **One catalog open at a time**; "Open Catalog…" is MVP+ | — |
| D8 | Tone mapper application | **Luminance norm with a path to white** (hue-preserving); per-channel only behind a debug switch | Per-channel clearly wins in the M3 image-quality comparisons |
| D9 | Distribution | **AUR (PKGBUILD)** first, Flatpak later | — |

---

## 16. References

- darktable: scene-referred workflow, color calibration (CAT16), filmic and sigmoid tone mappers.
- RawTherapee and ART: RCD and AMaZE demosaicing.
- Adobe DNG Specification: camera color model, Temp/Tint conversion.
- LibRaw API documentation.
- Li et al., *Comprehensive color solutions: CAM16, CAT16, and CAM16-UCS* (2017).
- B. Ottosson, *A perceptual color space for image processing* (OkLab, 2020).
- K. He, J. Sun, X. Tang, *Guided Image Filtering*.
- S. Paris, S. Hasinoff, J. Kautz, *Local Laplacian Filters* (a possible upgrade path for Clarity and local tone).
- F. Fritsch, R. Carlson, *Monotone Piecewise Cubic Interpolation* (1980).
- raw.pixls.us: CC0 raw sample files.
- XMP specification and the IPTC Photo Metadata Standard.
