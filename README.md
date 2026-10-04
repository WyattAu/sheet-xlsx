# sheet-xlsx

XLSX codec — workbook, worksheets, shared strings, styles, calc chain.

`sheet-xlsx` parses and serializes SpreadsheetML packages on top of
[`sheet-core`](https://github.com/WyattAu/sheet-core): `read_xlsx(bytes)` →
`XlsxWorkbook`, `write_xlsx(&XlsxWorkbook)` → bytes. Cell coordinates are the
shared 0-based `(row, col)` space; `A1` conversion is reused from
`sheet_core::refs`.

## Design

- **Relationship-graph discovery.** Parts are located through
  `_rels/.rels` → workbook → workbook rels, not by hardcoded paths, so
  relocated or renamed packages still parse. Absolute (`/xl/…`) and
  `..`-relative targets normalize.
- **Faithful round-trips.** `write_xlsx` → `read_xlsx` reproduces
  `XlsxCell`s exactly — including the shared-vs-inline string distinction,
  error literals, cached formula results, and style indices. This is the
  property the suite checks on 500 random workbooks per run.
- **Deterministic output.** Same workbook in, byte-identical package out
  (row-major cell order, stable part order, deflate).
- **Derived calc chain.** `xl/calcChain.xml` is derived fresh from the
  cell map on write and ignored on read — Excel rebuilds it, and a stale
  chain is worse than none.
- **Tolerant reader, strict model.** Unknown elements are skipped; cells
  with neither value nor formula are absent (blank-by-absence, matching
  `sheet-core`). Genuinely broken data — invalid references, unparseable
  numbers, dangling relationships — is a typed [`XlsxError`], never a
  panic.

## Known v0.1 limitations (documented)

- **Style definitions are not modeled** — `Styles` is a `cellXfs` count;
  indices round-trip, fonts/fills/borders do not.
- **Shared formulas**: the master keeps its text; slaves keep cached
  values, not translated formulas.
- Charts, pivot tables, themes, merged cells, and number formats are out
  of scope; the reader ignores parts it does not consume.

## Example

```rust
use sheet_xlsx::{read_xlsx, write_xlsx, XlsxCell, SheetData, XlsxValue, XlsxWorkbook};

let mut wb = XlsxWorkbook::single_sheet();
wb.sheets[0].cells.insert((0, 0), XlsxCell::new(XlsxValue::Number(41.0)));
wb.sheets[0].cells.insert((1, 0), XlsxCell::formula("A1+1", XlsxValue::Number(42.0)));

let bytes = write_xlsx(&wb).unwrap();
let back = read_xlsx(&bytes).unwrap();
assert_eq!(back.sheets[0].cells.get(&(1, 0)).unwrap().formula.as_deref(), Some("A1+1"));
```

## Layer

L1 substrate over `sheet-core` L1 (same-layer composition), plus `zip` and
`quick-xml`. `#![forbid(unsafe_code)]`, deny-on-unwrap/expect/panic/indexing
lints.

## Gates

`cargo build --locked --all-features` and `--no-default-features` ·
`cargo test --all-features` · `clippy -D warnings` (+ zero pedantic
warnings) · `cargo fmt --check` · `cargo deny check` · `cargo vet` ·
coverage ≥ 90% (currently ~93%) · estate layer check — enforced by the
shared [rust-kit workflow](https://github.com/WyattAu/engineering-standards).

## License

MIT OR Apache-2.0.
