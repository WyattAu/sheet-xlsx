//! Property tests — write→read round-trip exactness on arbitrary workbooks
//! (500 cases), plus a structural JSON snapshot via `serde_json` that pins
//! the parsed model's shape independent of Rust type equality.

// Test harness: assertions legitimately panic; lib paths stay lint-clean.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

use proptest::prelude::*;
use sheet_xlsx::{read_xlsx, write_xlsx, SheetData, XlsxCell, XlsxValue, XlsxWorkbook};
use std::collections::BTreeMap;
use std::vec::Vec;

/// A proptest failure carrying a readable message.
fn fail(e: impl std::fmt::Debug) -> TestCaseError {
    TestCaseError::fail(format!("{e:?}"))
}

/// Printable text without control chars (XML-unsafe characters excluded at
/// the strategy level; escapes are covered by the round-trip unit tests).
fn text_strategy() -> impl Strategy<Value = String> {
    proptest::collection::vec("[ -~\u{a0}-\u{2ff}]*", 0..12).prop_map(|chars| chars.concat())
}

fn error_literal() -> impl Strategy<Value = String> {
    prop_oneof![
        Just(String::from("#DIV/0!")),
        Just(String::from("#N/A")),
        Just(String::from("#NAME?")),
        Just(String::from("#NULL!")),
        Just(String::from("#NUM!")),
        Just(String::from("#REF!")),
        Just(String::from("#VALUE!")),
        Just(String::from("#SPILL!")),
        Just(String::from("#CALC!")),
    ]
}

/// A finite, XML-round-trippable number.
fn number_strategy() -> impl Strategy<Value = f64> {
    prop_oneof![
        (-1e12f64..1e12).prop_map(|n| (n * 4.0).trunc() / 4.0),
        Just(0.0),
        Just(f64::MAX),
        Just(f64::MIN_POSITIVE),
        (-1e6..1e6),
    ]
    .prop_filter("must be finite", |n| n.is_finite())
}

fn value_strategy(shared_len: usize) -> impl Strategy<Value = XlsxValue> {
    // Max valid index is len-1; the strategy bound keeps the cast lossless.
    #[allow(clippy::cast_possible_truncation)]
    let shared = shared_len as u32;
    prop_oneof![
        number_strategy().prop_map(XlsxValue::Number),
        (0..shared).prop_map(XlsxValue::SharedString),
        text_strategy().prop_map(XlsxValue::InlineString),
        proptest::bool::ANY.prop_map(XlsxValue::Boolean),
        error_literal().prop_map(XlsxValue::Error),
        text_strategy().prop_map(XlsxValue::FormulaString),
    ]
}

fn cell_strategy(shared_len: usize) -> impl Strategy<Value = XlsxCell> {
    (
        value_strategy(shared_len),
        proptest::option::of(0u32..8),
        proptest::option::of("[A-Za-z0-9+:&*<>()]{1,24}".prop_map(|f| format!("SUM({f})"))),
    )
        .prop_map(|(value, style_index, formula)| XlsxCell {
            value,
            style_index,
            formula,
        })
}

/// Arbitrary workbook: 1–3 sheets, up to 40 cells each on a 30×8 grid,
/// 1–6 shared strings (the table is seeded so `SharedString` indices are
/// valid; the invalid case is covered by the typed-error unit tests).
fn workbook_strategy() -> impl Strategy<Value = XlsxWorkbook> {
    (1usize..3usize, 1usize..7usize)
        .prop_flat_map(|(sheets, shared_len)| {
            (
                Just(shared_len),
                proptest::collection::vec(
                    (
                        "[A-Za-z0-9 _-]{1,12}",
                        proptest::collection::hash_map(
                            (0u32..30, 0u32..8),
                            cell_strategy(shared_len),
                            0..40,
                        ),
                    ),
                    sheets,
                ),
            )
        })
        .prop_map(|(shared_len, sheets)| {
            let mut wb = XlsxWorkbook::default();
            for (name, cells) in sheets {
                wb.sheets.push(SheetData {
                    name,
                    cells: BTreeMap::from_iter(cells),
                });
            }
            for i in 0..shared_len {
                wb.shared_strings.insert(&format!("shared {i}")); // distinct: insert dedups
            }
            wb
        })
}

proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(500))]

    /// Property: write→read reproduces the workbook exactly.
    #[test]
    fn write_then_read_is_exact(wb in workbook_strategy()) {
        let bytes = write_xlsx(&wb).map_err(fail)?;
        let back = read_xlsx(&bytes).map_err(fail)?;
        prop_assert_eq!(back, wb);
    }

    /// Property: the written bytes are a valid zip every time.
    #[test]
    fn output_is_always_a_zip(wb in workbook_strategy()) {
        let bytes = write_xlsx(&wb)?;
        let cursor = std::io::Cursor::new(bytes);
        let archive = zip::ZipArchive::new(cursor).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert!(archive.len() >= 5, "expected the core parts, got {}", archive.len());
    }

    /// Property: the parsed model's JSON shape is stable — every present
    /// cell serializes with exactly the documented fields.
    #[test]
    fn json_snapshot_shape_is_stable(wb in workbook_strategy()) {
        let bytes = write_xlsx(&wb)?;
        let back = read_xlsx(&bytes)?;
        let json = serde_json::to_value(Structural::of(&back))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let obj = json.as_object().expect("top-level object");
        prop_assert!(obj.contains_key("sheets"));
        prop_assert_eq!(obj["sheet_count"].as_u64().unwrap_or(0), u64::try_from(back.sheets.len()).unwrap_or(0));
        for sheet in obj["sheets"].as_array().expect("sheets array") {
            let sheet = sheet.as_object().expect("sheet object");
            prop_assert!(sheet.contains_key("name"));
            for (_k, cell) in sheet["cells"].as_object().expect("cells object") {
                let cell = cell.as_object().expect("cell object");
                prop_assert_eq!(cell.len(), 3, "cell keys: value, style_index, formula");
                prop_assert!(cell.contains_key("value"));
                prop_assert!(cell.contains_key("style_index"));
                prop_assert!(cell.contains_key("formula"));
            }
        }
    }

    /// Property: totality — arbitrary bytes never panic; they parse or
    /// yield a typed error.
    #[test]
    fn reader_is_total_on_arbitrary_bytes(prefix in proptest::num::u8::ANY, len in 0..64usize) {
        let mut bytes = vec![prefix; len];
        if let Ok(base) = write_xlsx(&XlsxWorkbook::single_sheet()) {
            bytes.extend_from_slice(&base[..base.len().min(len.max(1) * 8 % (base.len() + 1))]);
        }
        // Totality: the reader returns — `Ok` or a typed [`XlsxError`] —
        // for arbitrary bytes; every variant is exercised somewhere, and
        // the contract under test here is simply "no panic, no hang".
        let _ = read_xlsx(&bytes);
    }
}

/// A serde-friendly structural projection of a workbook (avoids deriving
/// Serialize on the public model — the API stays codec-only).
struct Structural;

impl Structural {
    fn of(wb: &XlsxWorkbook) -> serde_json::Value {
        serde_json::json!({
            "sheet_count": wb.sheets.len(),
            "sheets": wb.sheets.iter().map(|sheet| {
                serde_json::json!({
                    "name": sheet.name,
                    "cells": sheet.cells.iter().map(|((r, c), cell)| {
                        let key = format!("{r},{c}");
                        let style = cell.style_index.map(|s| s.to_string());
                        (key, serde_json::json!({
                            "value": format!("{:?}", cell.value),
                            "style_index": style,
                            "formula": cell.formula,
                        }))
                    }).collect::<BTreeMap<_, _>>(),
                })
            }).collect::<Vec<_>>(),
        })
    }
}
