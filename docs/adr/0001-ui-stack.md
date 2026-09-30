# ADR 0001: UI stack — egui + eframe on wgpu

- Status: Accepted (2026-09-28)
- Plan reference: §3, §15 D1

## Context

Viberoom's Develop canvas needs to draw a GPU-rendered image, at interactive
frame rates, inside the same window as dense tool panels (sliders, curve
editors, a filmstrip). The UI toolkit and the GPU compute pipeline should
ideally share one GPU device so the rendered image can be shown with zero
copies.

Alternatives considered: Qt 6/QML via `cxx-qt` (best native look and feel,
heavy FFI surface); Slint (declarative, GPU interop less direct); `iced`
(wgpu-based, less mature widget set for dense tool UIs).

## Decision

`egui` + `eframe`, with the `wgpu` backend. Immediate-mode UI suits the
dense, frequently-rebuilt tool panels Lightroom-style apps need. `eframe`'s
wgpu backend shares the same `wgpu::Device`/`Queue` as `viberoom-engine`, so
the Develop canvas can paint the pipeline's output texture directly via an
egui paint callback.

## Consequences

- The engine, catalog, jobs and services crates stay UI-agnostic (plan
  principle 6); only `ui-kit`, the module crates and `app` depend on `egui`.
- egui's accessibility and typography are more limited than a native
  toolkit's — acceptable for v0.1, revisited only if it blocks release.

## Revisit if

The M0 spike can't show the pipeline's output texture in egui within the
§12 latency budget. Fallback: Qt 6/QML via `cxx-qt`; the engine and services
layers are unaffected since they never depended on egui.
