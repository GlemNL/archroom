# ADR 0006: Preview storage — JPEG files + previews.db index

- Status: Accepted (2026-09-28)
- Plan reference: §4.7, §5.4, §15 D6

## Context

L1 (thumbnail) and L2 (standard) previews (§5.4) need to load fast enough
for a 50k-photo grid to hit the §12 budget (cold start under 2s, 60fps
scroll), while keeping the catalog portable the way Lightroom's `.lrcat` +
preview folder is.

## Decision

Rendered previews are JPEG files on disk, next to the catalog in
`Viberoom Previews/`, indexed by a small `previews.db` SQLite database for
fast lookup by `(photo_id, level, params_hash)`.

## Consequences

- The catalog file itself stays small and fast to back up; previews can be
  regenerated from the catalog + originals if lost.
- Grid scrolling reads JPEG bytes from disk (OS page cache absorbs most of
  the cost after the first pass) rather than BLOBs from the main catalog
  connection, keeping the two write paths (catalog mutations vs. preview
  writes) independent.

## Revisit if

Loading the grid from files misses the §12 performance budget — the
fallback would be storing preview bytes as BLOBs in `previews.db` (or the
main catalog) instead.
