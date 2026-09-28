# ADR 0005: XMP sidecars — auto-write off, Adobe naming

- Status: Accepted (2026-09-28)
- Plan reference: §5.3, §15 D5

## Context

The catalog is the authoritative store (plan principle 2); XMP sidecars
exist for interop with darktable, digiKam and Lightroom, and as a second
copy for disaster recovery. Two independent choices: whether sidecars
write automatically on every edit, and what the sidecar filename looks
like when a RAW+JPEG pair would otherwise clash.

## Decision

- Auto-write **off** by default, matching Lightroom. `Ctrl+S` writes
  explicitly; automatic writing is an opt-in preference.
- Naming follows Adobe's convention: `IMG_0001.xmp`, falling back to
  `IMG_0001.CR3.xmp` when a RAW+JPEG pair sharing a basename would
  otherwise collide. Both the auto-write preference and the naming
  fallback are configurable.

## Consequences

- Writes are read-modify-write through exiv2, so fields another app wrote
  (Adobe `crs:`, darktable history) survive round trips (§5.3).
- A sidecar newer than the catalog's recorded `sidecar_mtime` gets a
  "metadata changed on disk" badge on folder sync (MVP+, §5.3) — the off-
  by-default auto-write makes external edits to sidecars a real scenario
  to design for, not an edge case.

## Revisit if

Nothing currently anticipated; not on the decision table.
