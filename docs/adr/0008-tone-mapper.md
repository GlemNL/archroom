# ADR 0008: Tone mapper application — hue-preserving luminance norm

- Status: Accepted (2026-09-28)
- Plan reference: §6.5, §15 D8

## Context

The Profile base curve (a parametric filmic sigmoid, §6.5) can be applied
per-channel (simple, but shifts hue and saturation as channels compress
differently near the highlights) or to a luminance norm with a controlled
"path to white" that compresses chroma near white while preserving hue —
the approach darktable's filmic/sigmoid modules use.

## Decision

Luminance norm with a hue-preserving path to white, by default. Per-channel
application stays available behind a debug switch for the M3 image-quality
comparisons against darktable and Lightroom reference renders.

## Consequences

- Saturated colors pushed toward clipping desaturate smoothly toward white
  instead of shifting hue (the classic per-channel-curve artifact on, e.g.,
  saturated reds and blues).
- The tone-map stage (§6.2, stage 3) needs to work in a luminance-plus-
  chroma representation, not just per-channel RGB — slightly more shader
  complexity, budgeted for in M3.

## Revisit if

Per-channel application clearly wins the M3 image-quality comparisons.
