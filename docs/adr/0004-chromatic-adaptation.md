# ADR 0004: Chromatic adaptation — CAT16

- Status: Accepted (2026-09-28)
- Plan reference: §6.3, §6.4, §15 D4

## Context

White balance is applied as a chromatic adaptation on linear scene-referred
data, from the chosen illuminant to D65 (§6.3: raws are demosaiced once at
a fixed reference white, and Temp/Tint changes become a CAT rather than a
re-demosaic). The two common choices are Bradford (older, widely used —
Lightroom, most ICC workflows) and CAT16 (from CAM16, part of the newer
CIE color appearance model family — darktable's `color calibration`
module).

## Decision

CAT16 by default. Bradford stays available behind a debug switch for
side-by-side comparisons on the golden image set (§13).

## Consequences

- `viberoom-color` implements both matrices; the debug switch is a cheap
  runtime toggle, not two code paths to maintain.
- M3's image-quality tuning pass (§11, M4 exit) should include a CAT16 vs.
  Bradford comparison on the golden set as part of validating this choice.

## Revisit if

Bradford clearly wins the M3 golden-set comparisons.
