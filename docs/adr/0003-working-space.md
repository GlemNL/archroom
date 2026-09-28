# ADR 0003: Working space — linear Rec.2020 (D65)

- Status: Accepted (2026-09-28)
- Plan reference: §3, §4.1, §6.1, §15 D3

## Context

The scene-referred stages of the develop pipeline (§6.1) need a wide-gamut
linear working space wide enough to hold typical camera gamuts without
clipping, before any tone mapping happens.

Lightroom uses linear ProPhoto RGB internally. darktable moved to linear
Rec.2020 for its scene-referred workflow.

## Decision

Linear Rec.2020 (D65), following darktable rather than Lightroom's
ProPhoto choice.

## Consequences

- Camera color matrices (LibRaw's `cam_xyz`, the DNG ColorMatrix) are
  composed with an XYZ→Rec.2020 matrix instead of XYZ→ProPhoto.
- Rec.2020's gamut is wide enough for essentially all camera sensors and
  for Display P3/Adobe RGB export targets without going as far into
  imaginary-color territory as ProPhoto does.

## Revisit if

Nothing currently anticipated; not on the decision table.
