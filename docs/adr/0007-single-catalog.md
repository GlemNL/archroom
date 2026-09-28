# ADR 0007: One catalog open at a time

- Status: Accepted (2026-09-28)
- Plan reference: §4.7, §15 D7

## Context

Lightroom Classic supports multiple catalogs but only one open at a time,
switched via a restart-or-relaunch flow. Supporting several catalogs open
concurrently (tabs, multiple windows) is a much larger state-management
surface (separate job schedulers? separate GPU contexts? cross-catalog
drag-and-drop?) with little MVP payoff.

## Decision

One catalog open at a time, matching Lightroom. `Settings::last_catalog`
records which one to reopen on launch; an "Open Catalog…" flow to switch
without restarting is MVP+, not MVP.

## Consequences

- `AppCx` holds a single `Option<Catalog>`, not a collection — simpler
  state everywhere downstream (selection, jobs, previews all implicitly
  scope to "the" open catalog).
- Switching catalogs (MVP+) can be implemented as "tear down AppCx's
  catalog-scoped state, open the new one" rather than true multi-catalog
  concurrency.

## Revisit if

Nothing currently anticipated; not on the decision table.
