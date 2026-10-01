# Viberoom v0.2.0: Plan

v0.2.0 adds four Develop tools:

1. **Red-eye correction**: place a circle on an eye, size it with a slider, and Apply. A **Pet Eye** mode handles the white, green or yellow glow in animals' eyes.
2. **Linear gradient**: drag a line and rotate it. The side below the line gets its own Exposure, Contrast, Highlights, Shadows, Whites and Blacks.
3. **Brush zone**: paint an area with a sized brush. The painted area gets the same six sliders.
4. **Rotate the crop box**: rotate directly on the canvas while cropping, not only with the Straighten slider.

The base plan is still `PLAN.md`. This file only covers what v0.2.0 adds and the order it lands in. Items 1–3 are the first slice of `PLAN.md` §11.2 #1 (local adjustments) plus the red-eye button reserved in §8.2. Radial gradients, range masks and spot removal are **not** part of v0.2.0.

---

## 1. Where the code is today

What the new features plug into:

| Area | State in v0.1.0 | What v0.2.0 needs |
|---|---|---|
| `engine::params::EditParams` | `local: Vec<Value>` is reserved and always empty (§5.2, §6.12). It is already part of `params_hash`. | A typed model for local adjustments. Unknown entries must still round-trip. |
| `engine::pipeline` | Five stages with a per-stage `Key::chain` cache. Stage 2 is scene matrix → NR → H/S (guided base) → clarity. Then stage 3 (tone map), stage 4 (display ops) and stage 5 (output). | A red-eye pass after the scene matrix, and a local-adjust pass between clarity and the tone map. |
| Masks in kernels | §6.12 planned a mask binding on every kernel. **No kernel has one.** | Not needed. v0.2.0 uses one dedicated local pass instead (see §3.2). |
| `engine::geometry` | `Geometry::output_to_source` (CPU twin of `geometry.wgsl`: crop → un-straighten → orientation). | The inverse, `source_to_output`, so the canvas can draw pins, lines and brush cursors that are stored in source coordinates. |
| `module-develop::crop_tool` | Move/resize handles, straighten slider, level line, aspects, flip/rotate 90°, overlays. Dragging **outside** the box starts a new crop. | A rotate gesture. It has to share the outside-the-box area with "start a new crop". |
| Canvas tools | `DevelopModule` holds `crop_tool: Option<CropTool>` directly. The `CanvasTool` trait was deferred "until a second tool needs it" (M4 note). | Four tools now, so the trait gets built. |
| `SettingsGroup` | 14 groups, used by presets, copy/paste and sync. | `RedEye` and `LocalAdjustments`. |
| CLI | `viberoom-cli render --params file.json` | Works unchanged once the engine reads `local` and `red_eye`. It is the headless test path for Phase A. |

---

## 2. Data model

### 2.1 Coordinates

Everything placed on the image is stored in **source-normalized coordinates**: 0..1 on the decoded image, before orientation, quarter turns, flip, straighten or crop. That way, re-cropping, straightening or rotating the photo later does not move a red-eye circle, a gradient or a brush stroke off the thing it was placed on. Lightroom works the same way.

- Lengths (radius, feather, brush size) are fractions of the **source diagonal**, so they do not depend on the image's aspect ratio.
- Angles are in degrees in the source pixel frame (y down). The UI converts them to and from screen angles through `Geometry`, taking orientation and flips into account.
- New helper: `Geometry::source_to_output(x, y) -> Option<[f64; 2]>`, plus `source_angle_to_output` and `output_angle_to_source`. A round-trip unit test checks them against `output_to_source` for every orientation, flip, and angles of ±45°.

### 2.2 Red eye: a normal op

```json
"red_eye": { "v": 1, "spots": [
  { "x": 0.412, "y": 0.337, "r": 0.006, "mode": "red", "darken": 50 },
  { "x": 0.655, "y": 0.341, "r": 0.009, "mode": "pet", "darken": 70, "catchlight": true } ] }
```

- `engine::ops::RedEye` with `STAGE = Scene`, a new `SettingsGroup::RedEye`, and identity when `spots` is empty.
- `r` is the circle radius (a fraction of the diagonal). `darken` (0–100, default 50) sets how dark the corrected pupil becomes; it is the Lightroom "Darken" slider.
- `mode` is `"red"` (the default when absent) or `"pet"`, and is set per spot, so a photo of a person holding a cat can have both.
- `catchlight` applies to pet mode only (default off). It adds a small soft specular dot so the corrected eye doesn't look flat, the way Lightroom's "Add Catchlight" does.
- There are many spots per photo, one per eye.

### 2.3 Local adjustments: `EditParams.local`

```json
"local": [
  { "v": 1, "id": "k3f9", "name": "Gradient 1", "enabled": true,
    "mask":   { "kind": "linear", "x": 0.5, "y": 0.42, "angle": 3.5, "feather": 0.15 },
    "adjust": { "exposure": -0.8, "contrast": 0, "highlights": -30, "shadows": 0, "whites": 0, "blacks": 0 } },
  { "v": 1, "id": "p0aa", "name": "Zone 1", "enabled": true,
    "mask":   { "kind": "brush", "strokes": [
                 { "size": 0.03, "feather": 0.5, "flow": 1.0, "erase": false,
                   "points": [[0.31, 0.62], [0.33, 0.61], [0.36, 0.60]] } ] },
    "adjust": { "exposure": 0.4, "shadows": 25 } }
]
```

- `engine::local`: `LocalAdjustment { id, name, enabled, mask: MaskDef, adjust: LocalTone }`, with `MaskDef::{Linear{..}, Brush{strokes}}`.
- `LocalTone` holds the six Basic tone sliders with the same ranges as the global ones: Exposure ±5 EV, the others ±100. Adjustments stored as all zeros are dropped, the same way identity ops are.
- **Linear mask.** The line goes through `(x, y)` at `angle`. The mask is 1 on the side the line's normal points to ("below" the line when angle = 0) and 0 on the other side. It ramps with a smoothstep over `feather` (the full transition width, centered on the line). Flipping which side is affected means adding 180° to the angle, so no extra field is needed.
- **Brush mask.** An ordered list of strokes. Each stroke stamps soft discs (`size` is the diameter, `feather` 0–1 is the soft fraction of the radius) along its polyline with `flow` as opacity. Painted strokes combine by max-over (not additive), so painting over an area again never goes above 1. Erase strokes multiply the mask by (1 − stamp). Points are thinned while painting (a new point only when it is ≥ ¼ of the brush radius from the last one), which keeps the JSON small.
- **Forward compatibility.** `local` stays a `Vec<Value>` on disk. The engine parses each entry into `LocalAdjustment` and skips (but keeps) any entry whose `v` or `mask.kind` it does not know. A radial mask added in v0.3 therefore survives a round trip through v0.2.
- **Limit:** 16 local adjustments per photo in v0.2.0, because of the texture array size (§3.2). The UI disables "New" at the limit.
- New `SettingsGroup::LocalAdjustments`.

### 2.4 Presets, copy/paste, sync, XMP

- Presets **never** include `RedEye` or `LocalAdjustments`, like Lightroom: they are tied to a specific photo. The preset dialog does not show those two checkboxes, and applying a preset leaves them untouched.
- Copy/paste and Sync show both groups, **unchecked by default**. Sync copies the masks as they are: the source-normalized coordinates land in the same relative place on the target photo.
- XMP (`viberoom:DevelopSettings`) already serializes all of `EditParams`, so no change is needed there. A test confirms the round trip.
- History labels: "Red Eye", "Add Gradient", "Edit Gradient", "Add Brush Zone", "Brush", "Delete Local Adjustment", "Rotate Crop".

---

## 3. Engine

### 3.1 Red-eye pass (stage 2, right after the scene matrix)

It runs on scene-linear Rec.2020 after white balance, so the result does not depend on later tone or color ops. Cache key: `Key::chain(k_scene)` plus the spot list. It is skipped when identity.

For each output pixel, the kernel maps back to source uv with the same `to_src` math as `geometry.wgsl` (the geometry uniform is shared) and loops over the spots, which are passed in a small uniform/storage array (≤ 64):

1. Pixels outside the circle are untouched. A 15 % soft edge avoids a visible ring.
2. Redness is `ρ = R / max(ε, (G + B) / 2)`. Pixels with `ρ < 1.5`, and very dark pixels, are left alone, so skin and iris inside the circle survive even when the circle is too big. This is what makes "just place a circle" work.
3. For red pixels: `R ← min(G, B)` (desaturate toward neutral), then scale all three channels by `lerp(1, 0.15, darken/100)`. Blend with the weight `smoothstep(1.5, 2.2, ρ) × edge`.

**Pet mode.** Animal eye glow (tapetum reflection) can be any hue and is usually very bright, so a redness test would miss it. In pet mode the circle is treated as the pupil, and the user sizes it to fit:

1. The pupil target is a neutral dark gray, `t = lerp(0.04, 0.005, darken/100)` in scene-linear terms (around −2 to −5 EV below middle gray).
2. Only pixels **brighter** than the target change. The weight is `smoothstep(t, 4t, luma) × edge`, with a wider 30 % soft edge. Each such pixel is replaced by the target, keeping 10 % of its original luma variation so the pupil keeps some texture instead of looking like a flat disc. Dark fur or iris already below the target stays as it is.
3. `catchlight`: a soft white dot (radius 0.15 r, at 1.5 EV above middle gray) placed up and to the left of the center at (−0.35 r, −0.35 r) in *output* orientation, so it stays upper-left after the photo is rotated.

Tests: an identity check (no spots gives bit-exact output); a synthetic red disc on a gray field becomes neutral while a skin-colored annulus inside the circle is unchanged; in pet mode a bright green disc and a bright white disc both become dark and neutral while a darker ring inside the circle is unchanged; the catchlight stays upper-left in all four orientations; results match between the proxy and a full-resolution render.

### 3.2 Local-adjust pass (stage 2d, between clarity and the tone map)

There is one pass for all adjustments, not a mask on every kernel. The six sliders are all scene-referred tone controls, so a single kernel can apply them in the linear domain before the global tone map, much as Lightroom's local Basic sliders behave. It is cheaper than re-running stages per mask and keeps the pipeline graph unchanged.

**Mask textures.** One `R16Float` texture array, one layer per enabled adjustment, at proxy size. It is produced by a mask kernel:

- Linear: evaluated analytically per output pixel (map to source px, signed distance to the line, smoothstep).
- Brush: strokes are rasterized **on the CPU** into a source-aligned `R8` mask (long edge = min(source, 2048) px) with rayon, cached in the session by `(adjustment id, strokes hash)`, and uploaded once. The mask kernel samples it bilinearly through `to_src`, the same way the geometry pass samples the source image. While painting, a new stroke is rasterized **incrementally** into the cached mask (only the dabs it adds). The mask is fully rebuilt only on undo or erase. A 2048 px mask is 4 MB per zone, 64 MB at the 16-zone limit.
- Cache key: the geometry key plus the mask definitions. Moving a gradient re-runs only the mask kernel, the local pass and everything downstream.

**The local kernel.** It takes the clarity output, the mask array, the H/S guided base layer, and a uniform array of `LocalTone`. For each pixel:

```
L  = log2(luma(rgb) / 0.18)                             // EV around middle gray
Σ over masks i:  m = mask_i
  exposure   ΔL += m · ev_i
  contrast   ΔL += m · c_i · (L − 0)           (pivot at middle gray, c scaled ±0.5)
  shadows / highlights: the same gain curve as highlights_shadows.wgsl on the base layer B, weighted by m
  whites     ΔL += m · w_i · smoothstep(1.5, 3.5, L)     (upper region only)
  blacks     ΔL += m · b_i · (1 − smoothstep(−6, −3, L)) (lower region only)
rgb *= 2^ΔL                                              // hue-preserving, like §6.5
```

- The base layer is reused from stage 2b when global H/S is active. Otherwise it is computed on demand under the same key, so it is still whole-frame and scale-invariant (§6.5).
- Overlapping masks add up in EV, which is what users expect ("two −1 EV gradients = −2 EV").
- The whites/blacks regions and the contrast scale above are first guesses. They get a tuning pass against Lightroom on the golden set (§6), and the constants live in one place (`engine::local::tuning`) so tuning does not touch the kernel.
- A disabled adjustment produces no mask layer and no key contribution.

**Mask overlay.** `RenderRequest::mask_overlay: Option<u32>` (a layer index). Stage 5 tints that layer red at 50 % (`O` key while a local tool is open, like Lightroom), reusing the clip-overlay code path.

### 3.3 Tests (all headless)

- `local` round trip: typed ⇄ `Value`; unknown `mask.kind` preserved; all-zero adjustments dropped.
- Linear mask: 0 and 1 far from the line, 0.5 on the line, ramp width = feather, correct side, and still correct after quarter turns, flip and straighten (checked with `source_to_output`).
- Brush raster: one dab matches the analytic disc; max-over never exceeds 1; erase clears; incremental raster equals full raster, bit-exact.
- Local pass: an all-zero adjustment renders like no adjustment; a full-frame mask with +1 EV equals global +1 EV before the tone map (within f16 tolerance); a proxy render matches a downscaled full-resolution render (scale invariance, §13).
- Golden images: three new param files in `tests/golden/` (red eye, gradient, brush), blessed with `just bless`.
- CLI: `viberoom-cli render --params gradient.json --stats` runs against `_7808140.NEF` and the stats/PNG change as expected.

---

## 4. UI

### 4.1 Canvas tool framework (built first)

```rust
pub trait CanvasTool {
    fn id(&self) -> ToolId;                             // Crop, RedEye, Gradient, Brush
    fn render_hints(&self, req: &mut RenderRequest);    // ignore_crop, mask_overlay, draft params
    fn canvas(&mut self, ui, img_rect, resp, geom, params) -> Option<Outcome>;
    fn options(&mut self, ui, params, geom) -> (Option<Outcome>, ToolAction); // right-panel section
    fn on_key(&mut self, key) -> bool;
}
```

- `DevelopModule.crop_tool` becomes `tool: Option<Box<dyn CanvasTool>>`, and the tools exclude each other. `crop_tool.rs` is ported without behavior changes. Its existing smoke test (`VIBEROOM_START_TOOL=crop`) must look identical before and after.
- The tool strip in the right panel (§8.2) gets real buttons: **Crop (`R`)**, **Red Eye (`Shift+R`)**, **Gradient (`M`)**, **Brush (`K`)**, as icons with tooltips. The keys follow Lightroom where Lightroom has one; none of them are taken in Develop today.
- **Draft and Apply.** The red-eye, gradient and brush tools edit a *draft* copy of the params, which renders live. **Apply** (or Enter) commits the draft as **one** history step. **Cancel** (or Esc) throws it away. Crop keeps its current behavior, where each gesture is a history step and Done closes the tool.
- `VIBEROOM_START_TOOL=redeye|gradient|brush` hooks are added for the screenshot smoke tests (see the UI smoke-testing recipe).

### 4.2 Red-eye tool

- A click on the image places a circle (default radius about 1 % of the diagonal). A drag from the click point sizes it.
- Right panel: a **Red Eye / Pet Eye** switch (applies to the selected circle and to new ones), a **Size** slider (the radius of the selected circle), a **Darken** slider, an **Add Catchlight** checkbox shown in Pet Eye mode, and **Apply**, **Cancel** and **Delete** (removes the selected circle).
- Circles are drawn in a different color per mode (white for red eye, amber for pet eye), so a mixed photo stays readable.
- Existing circles show as outlines. Clicking one selects it, dragging moves it, and dragging its edge resizes it. More clicks add more eyes before Apply.
- Hovering shows a circle cursor at the current default size.

### 4.3 Gradient tool

- A drag on the image draws the line: press point, drag direction, and length (which sets the feather). The UI draws three lines: the center and the two feather edges (dashed), plus a pin at the center.
- **Center pin**: drag to move. **Rotation handle**: a knob at one end of the center line; drag it to rotate about the pin, with Shift to snap to 15°. **Edge lines**: drag to change the feather. A small arrow shows the affected side, and **Flip** (or `'`) swaps it.
- Right panel: the six sliders (**Exposure, Contrast, Highlights, Shadows, Whites, Blacks**), plus **Apply**, **Cancel**, **Delete**, a **Show mask (`O`)** checkbox, and a list of existing local adjustments with enable toggles. Clicking one in the list, or its pin on the canvas, reopens it for editing.
- Slider drags re-render live through the existing latest-wins loop. Only the mask and local pass and what follows it re-run.

### 4.4 Brush zone tool

- The cursor becomes a brush: two circles (inner = hard core, outer = size). Painting adds to the zone; **Alt** erases.
- Right panel: **Size**, **Feather** and **Flow** sliders. `[`/`]` change the size, and `Shift+[`/`Shift+]` change the feather, as in Lightroom. Then **New zone**, the six tone sliders, **Apply**, **Cancel**, **Delete**, and **Show mask (`O`)**. The mask overlay turns on automatically while the first stroke is painted.
- A stroke streams its points into the draft. Each frame, the session rasterizes only the new dabs (§3.2), so painting stays under the §12 slider budget.
- The zone gets a pin on the canvas at its first dab. Clicking the pin reopens the zone for more painting or new slider values.

### 4.5 Rotating the crop box

It stores the same thing as the Straighten slider (`straighten.angle`, ±45°), so the engine does not change. What changes is the gesture:

- **Rotate zone**: a ring 8–40 px **outside** the crop box's corners. The cursor there becomes a rotate arrow. Dragging farther away still starts a new crop, as it does today, so both gestures keep working.
- **Lightroom style: the image rotates under an upright box.** Dragging in the rotate zone changes `straighten.angle` by the angle the cursor sweeps around the box's center. The box stays upright and fixed on screen while the image turns underneath it, live. A fine grid overlay is forced on during the drag so you can line it up with the horizon, and it goes back to the chosen overlay on release.
- **Live preview.** Each drag frame sends the new angle through the latest-wins render loop, and the geometry pass re-renders the whole straightened canvas (`ignore_crop` is already set while the tool is open). At proxy size this is well inside the slider budget. The box is kept inside the rotated image on every frame: `fit_rect` shrinks it around its center as the angle grows, and gives the size back as the angle returns toward 0, measured from the box as it was when the drag started. Releasing never moves the box.
- Dragging clockwise turns the image clockwise, matching the Straighten slider's sign. The slider and the level line keep working and stay in sync with the angle.
- Shift snaps to whole degrees. There is a light snap at 0°. The angle readout sits next to the cursor while dragging. Double-clicking the rotate zone resets the angle to 0.
- One history step per drag ("Rotate Crop"), with the existing constrain-to-image behavior.

---

## 5. Phases

Following the usual viberoom pattern, the headless and CLI-testable work comes before the UI. Each phase is self-verified (tests plus a real run against `_7808140.NEF`, and a screenshot smoke test once there is UI) before the next one starts.

### Phase A: engine and data model (headless)
- [x] `Geometry::source_to_output` and the angle helpers, with round-trip tests
- [x] `engine::local` typed model, `SettingsGroup::{RedEye, LocalAdjustments}`, round-trip and forward-compat tests
- [x] `RedEye` op and `red_eye.wgsl` (stage 2, after the scene matrix), with red and pet modes and the catchlight
- [x] Mask kernel (linear, analytic) and CPU brush raster, with a session-level mask cache
- [x] `local.wgsl` pass (stage 2d), cache keys, and the `mask_overlay` tint in stage 5
- [x] Tests from §3.3, golden files, and a CLI run with hand-written param files

**Exit:** `viberoom-cli render` produces a correct red-eye fix, a gradient and a brush zone from JSON params; `just test` and `just lint` pass; golden images are blessed.

**Done (2026-09-30).** Where the implementation differs from §3:

- **No mask kernel or per-mask texture array.** The CPU maps each circle and gradient into *output* pixels (`Geometry::source_to_output`, `source_vector_to_output`; all transforms are rigid, so a line stays a line and a circle a circle), so the red-eye kernel and the linear mask need no geometry math on the GPU. Linear masks are evaluated analytically inside `local.wgsl`; brush zones are the only textures: one `R8Unorm` array (layer per zone, source-aligned, ≤ 2048 px) sampled through the shared `to_src`. Nothing is allocated at proxy size per mask, and moving a gradient re-runs only the local pass. Code: `pipeline_local.rs`, `shaders/local_mask.wgsl` (shared), `local.wgsl`, `mask_overlay.wgsl`.
- **Mirrored orientations flip a gradient's side** if the normal is rebuilt from an angle; the plan uses the transformed normal vector instead (caught by a test over all eight EXIF orientations).
- **The local pass computes its own guided base layer** from its input (post-clarity) rather than reusing stage 2b's; it only runs when some mask uses Highlights or Shadows.
- **`RenderRequest::mask_overlay` is the adjustment's `id`** (`Option<String>`), not a layer index. An identity adjustment still gets its overlay (a freshly placed mask). `viberoom-cli render --mask-overlay <id>` exposes it.
- **Red-eye redness threshold** is `smoothstep(2.5, 4.0, R / ((G+B)/2))`, not 1.5–2.2: linear-light skin reads up to ~2.1, which the plan's range would have desaturated. The corrected pixel is `min(G, B)` on all three channels (not just R), so a saturated red ends up neutral in Rec.2020.
- `Geometry::source_to_output` returns `[f64; 2]` (outside 0..1 when cropped away) rather than an `Option`.
- Code lives in `engine::local` (model, `tuning`, `BrushRaster`), `engine::ops::RedEye`, `EditParams::{local_adjustments, set_local_adjustments}`, `SettingsGroup::{RedEye, LocalAdjustments, is_photo_specific}`.
- Phase F reminder: the two new groups now appear in the copy/paste dialog via the registry / preset dialog lists; they must be hidden from presets and unchecked by default (`SettingsGroup::is_photo_specific`).

### Phase B: tool framework and crop rotation
- [x] `CanvasTool` trait; port `CropTool` onto it without changing behavior; tool-strip buttons (the three new ones disabled until their phase lands)
- [x] Draft/Apply/Cancel plumbing in `DevelopModule`
- [x] Crop rotate zone, live Lightroom-style image rotation under the upright box, snapping, per-frame fit (§4.5)

**Exit:** crop works as it did in v0.1.0, plus drag-to-rotate. A screenshot smoke test runs with `VIBEROOM_START_TOOL=crop`.

### Phase C: red-eye tool
- [x] Place, size, move and delete circles; Red Eye / Pet Eye switch; Size and Darken sliders; Add Catchlight; Apply and Cancel (§4.2)

### Phase D: gradient tool
- [x] Draw, move, rotate, feather and flip (§4.3); six sliders; the local-adjustment list; mask overlay

### Phase E: brush zone tool
- [x] Brush cursor, painting, Alt-erase, Size/Feather/Flow sliders with bracket keys, incremental raster, zone pins (§4.4)

### Phase F: integration and release
- [x] Copy/paste and Sync dialogs show the two new groups (unchecked by default); presets exclude them
- [x] L1/L2 re-render after local edits (already handled by `params_hash`; verify only)
- [x] Persistence round trip (catalog save/load/undo; the develop settings live in the catalog, not XMP) of `local` and `red_eye`
- [ ] Tuning pass on the local whites/blacks/contrast constants against Lightroom on a few golden raws
- [x] Performance check (RX 7900, release CLI, 16 gradients with shadows/highlights/whites, 1536 px, `_7808140.NEF`): first render 21 ms, exposure-slider re-render 1.5 ms. **Not measured:** painting on the UHD 630 (needs the UI).
- [x] README feature list and shortcut cheat sheet (`PLAN.md` edits still to do) · and shortcut cheat sheet (`M`, `K`, `Shift+R`, `O`, `[`/`]`, Alt); `PLAN.md` §8.2 tool strip moved from Later to done, and §11.2 #1 marked partially done
- [ ] Bump `workspace.package.version` and the internal crate versions to `0.2.0`, update `packaging/PKGBUILD` and `Cargo.lock`, then tag `v0.2.0` (the release workflow does the rest)

**Exit (v0.2.0 done):** on a real raw, a user can fix red eye, darken a sky with a gradient, brighten a face with a brush zone and rotate the crop by dragging, all without touching a global slider. The edits survive a restart, undo/redo, sync to another photo and an export.

---

## 6. Risks

| Risk | Mitigation |
|---|---|
| Local whites/blacks/contrast don't feel like the global ones, because the global versions live inside the tone mapper | Constants isolated in `engine::local::tuning`; a dedicated tuning step in Phase F; worst case, local whites/blacks move to a per-pixel variant of the tone-map uniform (a heavier change, deferred) |
| Brush painting lags on large sources | Incremental CPU raster capped at 2048 px, one mask upload per frame, and only the mask/local stages re-run |
| Mask misalignment after crop, straighten or orientation changes | Source-normalized storage and one shared `to_src` path for image and masks, covered by tests across all orientations |
| The rotate zone conflicts with "drag outside to start a new crop" | Rotate only in a corner ring (8–40 px); the cursor shows which gesture applies |
| Scope creep toward radial masks, range masks and spot healing | Out of scope for v0.2.0; the `MaskDef` enum and the forward-compat rule make them additive in v0.3 |

---

## 7. Decisions

Settled on 2026-09-30:

1. **Crop rotation:** Lightroom style. The image rotates live under an upright, fixed crop box (§4.5).
2. **Pet Eye mode:** in v0.2.0, set per circle, with an optional catchlight (§2.2, §3.1, §4.2).
3. **Local sliders:** only the six tone sliders (Exposure, Contrast, Highlights, Shadows, Whites, Blacks). No local Temperature, Tint, Saturation or Clarity in v0.2.0.
