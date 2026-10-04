# Security Policy — sheet-xlsx

## Supported versions

| Version | Supported |
|---------|-----------|
| 0.1.x   | ✅        |

## Reporting a vulnerability

Report privately via [GitHub security advisories] for this repository, or
email **wyatt_au@protonmail.com**. Do **not** open a public issue for
security reports.

You will receive an acknowledgement within **72 hours**. Coordinated
disclosure: we ask for up to 90 days before public disclosure while a
patch ships.

## Scope notes

`sheet-xlsx` parses **untrusted file bytes** — the hostile-input boundary
is `read_xlsx(&[u8])`. Security posture for integrators:

- **Typed totality, no panics.** Malformed archives, broken relationship
  graphs, and corrupt cells yield [`XlsxError`] variants. The lib target
  compiles with deny-on-unwrap/expect/panic/indexing lints and
  `#![forbid(unsafe_code)]`.
- **Not a sandbox.** The codec parses structure; it does not evaluate
  formulas, resolve external references, or process macros. Formula text
  is carried verbatim as data — callers that evaluate formulas own that
  risk surface.
- **No code execution paths.** The reader ignores every package part it
  does not consume (no external-entity expansion, no DTD processing,
  `quick-xml` is not XXE-aware because entities are rejected by the
  escape handling).
- **Zip handling** is delegated to the maintained `zip` crate; advisories
  are gated via `cargo deny` in CI.
- **Writer invariants**: non-finite numbers and out-of-range shared-string
  indices are refused rather than written as corrupt XML.

[GitHub security advisories]:
    https://github.com/WyattAu/sheet-xlsx/security/advisories/new
