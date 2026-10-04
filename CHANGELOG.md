# Changelog

All notable changes to this project are documented here. Format: [Keep a
Changelog](https://keepachangelog.com/) — versions follow [semver](https://semver.org).

## [0.1.1] - 2026-10-04

### Fixed

- **Security: upgrade `quick-xml` 0.37 → 0.42** to clear
  RUSTSEC-2026-0194 (quadratic duplicate-attribute checking) and
  RUSTSEC-2026-0195 (unbounded namespace-declaration allocation) — both
  reachable through `read_xlsx` on hostile packages. The reader migrates
  to the 0.42 event API: names are `&str`, attribute values unescape via
  `normalized_value`, and text entities (`&amp;`, `&#38;`, …) arrive as
  `Event::GeneralRef` — now resolved instead of dropped.

## [0.1.0] - 2026-10-04

### Added

- **Package reading** — `read_xlsx`: relationship-graph part discovery
  (no hardcoded paths), streaming `quick-xml` parse of workbook, sheets,
  shared strings, and styles. All SpreadsheetML cell types map to
  `XlsxValue`: numbers, shared and inline strings (variant preserved),
  booleans, error literals, and formula cells with cached results.
  Shared formulas keep the master's text and slaves' cached values
  (documented limitation).
- **Package writing** — `write_xlsx`: deterministic, XML-escaped emission
  of the canonical minimal package — content types, root rels, workbook,
  per-sheet worksheets with `dimension`, the shared-string table, a
  canonical minimal `styles.xml`, and a freshly derived
  `xl/calcChain.xml` when formulas exist. Shared/inline string
  distinction survives round-trips exactly.
- **Calc chain** — `CalcChain::from_workbook` derives the chain from the
  cell map (sheet-major, coordinate-ascending); the reader never trusts
  a stored chain over the sheets themselves.
- **Typed errors** — exhaustive `XlsxError` with preserved `zip`/`quick-xml`
  sources, refusing non-finite numbers and out-of-range shared-string
  indices with dedicated variants.

[0.1.0]: https://github.com/WyattAu/sheet-xlsx/releases/tag/v0.1.0
