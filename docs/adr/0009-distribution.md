# ADR 0009: Distribution — AUR first

- Status: Accepted (2026-09-28)
- Plan reference: §11 M5, §15 D9

## Context

The primary dev target is Arch Linux (plan §3: hardware targets, system
package list all assume `pacman`/AUR conventions). Flatpak would give
broader distro reach but adds sandboxing considerations for raw file
access, GPU driver passthrough and XDG portal usage that aren't yet
resolved.

## Decision

Ship a PKGBUILD to the AUR first (`packaging/`, M5). Flatpak is a later
addition once the AUR package is stable and the sandboxing questions have
answers.

## Consequences

- M5's packaging work is a PKGBUILD + `.desktop` file + icon, not a
  Flatpak manifest.
- System dependencies (`libraw`, `lcms2`, `libgexiv2`, Vulkan loader/ICDs)
  are declared as PKGBUILD `depends`, not bundled — consistent with how
  the dev box already gets them via `pacman`.

## Revisit if

Nothing currently anticipated; not on the decision table.
