# ADR 0002: License — GPL-3.0-or-later

- Status: Accepted (2026-09-28)
- Plan reference: §3, §15 D2

## Context

Viberoom links against `exiv2` (GPL-2.0-or-later) for metadata read/write
and LibRaw (dual LGPL-2.1/CDDL) for raw decoding. The license needs to be
compatible with both, and the project wants to keep downstream forks and
improvements open.

## Decision

GPL-3.0-or-later for the whole workspace. `exiv2`'s GPL-2.0-or-later is
compatible (GPL-3.0-or-later can incorporate GPL-2.0-or-later code), and
LibRaw's LGPL-2.1/CDDL dual license is permissive enough to link from GPL
code either way.

## Consequences

- `cargo-deny`'s license allow-list (`deny.toml`) is built around this:
  permissive licenses plus GPL-2.0-or-later, LGPL-2.1, CDDL-1.0 and CC0-1.0
  (raw.pixls.us test fixtures).
- A future pure-Rust metadata backend (replacing `exiv2`) would not force a
  license change on its own, but see "Revisit if."

## Revisit if

`exiv2` is replaced by a permissively licensed metadata backend and no
other GPL dependency remains — the project could then consider a more
permissive license, though there's no plan to do so.
