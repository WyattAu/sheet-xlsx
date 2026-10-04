//! Package reading — `read_xlsx`.
//!
//! Parts are located through the relationship graph (root rels → workbook →
//! workbook rels → worksheets / sharedStrings / styles), never by hardcoded
//! paths, so packages with relocated parts still parse. Parsing is
//! streaming (`quick-xml`) and tolerant: unknown elements are skipped,
//! unknown cell types fall back to their cached text, and the only
//! rejections are structurally broken data — surfaced as typed
//! [`XlsxError`]s, never panics.

use crate::error::XlsxError;
use crate::{SharedStrings, SheetData, Styles, XlsxCell, XlsxValue, XlsxWorkbook};
use sheet_core::refs::parse_cell_name;
use std::collections::BTreeMap;
use std::io::Read;
use std::string::{String, ToString};
use std::vec::Vec;

/// Namespace suffixes used for relationship `Type` matching.
const OFFICE_DOC_REL: &str = "/officeDocument";
const WORKSHEET_REL: &str = "/worksheet";
const SHARED_STRINGS_REL: &str = "/sharedStrings";
const STYLES_REL: &str = "/styles";

/// Reads an XLSX package from `data`.
///
/// # Errors
///
/// - [`XlsxError::Zip`] when the container is unreadable.
/// - [`XlsxError::MissingPart`] / [`XlsxError::MissingRelationship`] when
///   the package graph is broken.
/// - [`XlsxError::Xml`] when a required part does not parse.
/// - [`XlsxError::CorruptCell`] when a cell's structure is invalid beyond
///   tolerance (bad reference, unparseable number, …).
/// - [`XlsxError::SharedStringRange`] when a cell indexes past the
///   shared-string table.
pub fn read_xlsx(data: &[u8]) -> Result<XlsxWorkbook, XlsxError> {
    let cursor = std::io::Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)?;

    // Root relationships → workbook part.
    let root_rels = read_part(&mut archive, "_rels/.rels")?;
    let workbook_path = find_rel_target(&root_rels, OFFICE_DOC_REL)
        .unwrap_or_else(|| String::from("xl/workbook.xml"));
    let workbook_xml = read_part(&mut archive, &workbook_path)?;

    // workbook.xml → named sheets with relationship ids.
    let (sheets, sheet_rids) = parse_workbook(&workbook_xml)?;

    // workbook rels → part paths. Base dir: `xl/workbook.xml` → `xl/`.
    let base_dir = match workbook_path.rfind('/') {
        Some(slash) => workbook_path[..slash].to_string(),
        None => String::new(),
    };
    let rels_path = rels_path_for(&workbook_path);
    let rels_xml = read_part(&mut archive, &rels_path)?;
    let rels = parse_relationships(&rels_xml)?;

    let mut shared_strings = SharedStrings::default();
    let mut styles = Styles::default();
    let mut worksheet_paths: Vec<Option<String>> = Vec::with_capacity(sheet_rids.len());
    for rid in &sheet_rids {
        let target = rels
            .iter()
            .find(|(id, _, _)| id == rid)
            .map(|(_, ty, target)| (ty.clone(), target.clone()))
            .ok_or_else(|| XlsxError::MissingRelationship { id: rid.clone() })?;
        let (rel_type, target) = target;
        if !rel_type.ends_with(WORKSHEET_REL) {
            return Err(XlsxError::MissingRelationship { id: rid.clone() });
        }
        worksheet_paths.push(Some(resolve_target(&base_dir, &target)));
    }
    for (_, rel_type, target) in &rels {
        if rel_type.ends_with(SHARED_STRINGS_REL) {
            let path = resolve_target(&base_dir, target);
            let xml = read_part(&mut archive, &path)?;
            shared_strings = parse_shared_strings(&xml)?;
        } else if rel_type.ends_with(STYLES_REL) {
            let path = resolve_target(&base_dir, target);
            let xml = read_part(&mut archive, &path)?;
            styles = parse_styles(&xml)?;
        }
    }

    let mut parsed_sheets: Vec<SheetData> = Vec::with_capacity(sheets.len());
    for ((name, _), path) in sheets.into_iter().zip(worksheet_paths) {
        let path = path.ok_or_else(|| XlsxError::MissingRelationship {
            id: String::from("<no rid>"),
        })?;
        let xml = read_part(&mut archive, &path)?;
        parsed_sheets.push(parse_worksheet(&name, &xml)?);
    }

    Ok(XlsxWorkbook {
        sheets: parsed_sheets,
        shared_strings,
        styles,
    })
}

/// Reads one part fully into a String.
fn read_part<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    path: &str,
) -> Result<String, XlsxError> {
    let mut file = archive.by_name(path).map_err(|e| match e {
        zip::result::ZipError::FileNotFound => XlsxError::MissingPart {
            part: String::from(path),
        },
        other => XlsxError::Zip(other),
    })?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)
        .map_err(|e| XlsxError::Zip(zip::result::ZipError::Io(e)))?;
    Ok(buf)
}

/// `xl/workbook.xml` → `xl/_rels/workbook.xml.rels`.
fn rels_path_for(part: &str) -> String {
    match part.rfind('/') {
        Some(slash) => {
            let (dir, name) = part.split_at(slash);
            format!("{dir}/_rels{name}.rels")
        }
        None => format!("_rels/{part}"),
    }
}

/// Resolves a relationship target against the base directory of the
/// source part. Absolute targets (`/xl/…`) are package-root-relative.
fn resolve_target(base_dir: &str, target: &str) -> String {
    if let Some(rest) = target.strip_prefix('/') {
        return normalize_path(rest);
    }
    let combined = if base_dir.is_empty() {
        String::from(target)
    } else {
        format!("{base_dir}/{target}")
    };
    normalize_path(&combined)
}

/// Collapses `.` and `..` segments of a `/`-separated package path.
fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// A parsed `<Relationship>`: `(id, type, target)`.
type Relationship = (String, String, String);

/// Parses `<Relationships>` children.
fn parse_relationships(xml: &str) -> Result<Vec<Relationship>, XlsxError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut rels = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)) => {
                if e.name().as_ref() == "Relationship" {
                    let id = attr_of(e.attributes(), "Id");
                    let ty = attr_of(e.attributes(), "Type");
                    let target = attr_of(e.attributes(), "Target");
                    if let (Some(id), Some(ty), Some(target)) = (id, ty, target) {
                        rels.push((id, ty, target));
                    }
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(XlsxError::Xml {
                    part: String::from("relationships"),
                    source,
                })
            }
        }
        buf.clear();
    }
    Ok(rels)
}

/// Finds the target of the first relationship whose type ends with `suffix`.
fn find_rel_target(rels_xml: &str, type_suffix: &str) -> Option<String> {
    parse_relationships(rels_xml)
        .ok()?
        .into_iter()
        .find(|(_, ty, _)| ty.ends_with(type_suffix))
        .map(|(_, _, target)| resolve_target("", &target))
}

/// A `<sheet>` entry: (name, relationship id).
type SheetRef = (String, String);

/// Parses `xl/workbook.xml`: sheet names in order plus their `r:id`s.
fn parse_workbook(xml: &str) -> Result<(Vec<SheetRef>, Vec<String>), XlsxError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut names: Vec<(String, String)> = Vec::new();
    let mut rids: Vec<String> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(e) | quick_xml::events::Event::Start(e)) => {
                if e.name().as_ref() == "sheet" {
                    let name = attr_of(e.attributes(), "name").unwrap_or_default();
                    let rid = local_attr(&e, "id").unwrap_or_default();
                    names.push((name, rid.clone()));
                    rids.push(rid);
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(XlsxError::Xml {
                    part: String::from("xl/workbook.xml"),
                    source,
                })
            }
        }
        buf.clear();
    }
    if names.is_empty() {
        return Err(XlsxError::MissingPart {
            part: String::from("xl/workbook.xml <sheets>"),
        });
    }
    Ok((names, rids))
}

/// Parses `xl/sharedStrings.xml`: the concatenated text of each `<si>`.
fn parse_shared_strings(xml: &str) -> Result<SharedStrings, XlsxError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut table = SharedStrings::default();
    let mut depth_si = 0usize;
    let mut in_t = 0usize;
    let mut current = String::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => match e.name().as_ref() {
                "si" => {
                    depth_si += 1;
                    current.clear();
                }
                "t" if depth_si > 0 => in_t += 1,
                _ => {}
            },
            Ok(quick_xml::events::Event::End(e)) => match e.name().as_ref() {
                "si" => {
                    depth_si = depth_si.saturating_sub(1);
                    table.insert(&current);
                    current.clear();
                }
                "t" => in_t = in_t.saturating_sub(1),
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(t)) if in_t > 0 => {
                let unescaped =
                    quick_xml::escape::unescape(t.as_ref()).map_err(|source| XlsxError::Xml {
                        part: String::from("xl/sharedStrings.xml"),
                        source: quick_xml::errors::Error::from(source),
                    })?;
                current.push_str(&unescaped);
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) if in_t > 0 => {
                // `&amp;` and friends arrive as their own event in 0.42.
                let raw = format!("&{};", r.as_ref());
                let resolved =
                    quick_xml::escape::unescape(&raw).map_err(|source| XlsxError::Xml {
                        part: String::from("xl/sharedStrings.xml"),
                        source: quick_xml::errors::Error::from(source),
                    })?;
                current.push_str(&resolved);
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(XlsxError::Xml {
                    part: String::from("xl/sharedStrings.xml"),
                    source,
                })
            }
        }
        buf.clear();
    }
    Ok(table)
}

/// Parses `xl/styles.xml` down to the `cellXfs` count.
fn parse_styles(xml: &str) -> Result<Styles, XlsxError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut in_cell_xfs = false;
    let mut declared: Option<u32> = None;
    let mut actual: u32 = 0;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Start(e)) => {
                if e.name().as_ref() == "cellXfs" {
                    in_cell_xfs = true;
                    declared = attr_of(e.attributes(), "count").and_then(|c| c.parse().ok());
                } else if in_cell_xfs && e.name().as_ref() == "xf" {
                    actual += 1;
                }
            }
            Ok(quick_xml::events::Event::Empty(e)) => {
                if in_cell_xfs && e.name().as_ref() == "xf" {
                    actual += 1;
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                if e.name().as_ref() == "cellXfs" {
                    in_cell_xfs = false;
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(XlsxError::Xml {
                    part: String::from("xl/styles.xml"),
                    source,
                })
            }
        }
        buf.clear();
    }
    Ok(Styles {
        cell_xfs: declared.unwrap_or(actual),
    })
}

/// One `<c>` element mid-parse.
struct PendingCell {
    coord: (u32, u32),
    style: Option<u32>,
    cell_type: String,
    value_text: Option<String>,
    inline_text: String,
    formula: Option<String>,
}

/// Parses one worksheet part into a [`SheetData`].
/// Parses the coordinate of a `<c>` element, falling back to the running
/// `(row, cursor)` when the `r` attribute is absent (always advancing the
/// cursor).
fn parse_cell_coord(
    e: &quick_xml::events::BytesStart<'_>,
    row_index: u32,
    col_cursor: u32,
) -> Result<(u32, u32), XlsxError> {
    let coord = match attr_of(e.attributes(), "r") {
        Some(r) => parse_cell_name(&r).map_err(|_| XlsxError::CorruptCell {
            reference: r.clone(),
            reason: "invalid cell reference",
        })?,
        None => (row_index, col_cursor),
    };
    Ok(coord)
}

/// Routes captured text into the pending cell's value / formula / inline
/// buffer according to the current capture mode.
fn push_text(
    pending: &mut Option<PendingCell>,
    in_v: usize,
    in_f: usize,
    in_inline_t: bool,
    text: &str,
) {
    if let Some(p) = pending.as_mut() {
        if in_v > 0 {
            p.value_text.get_or_insert_with(String::new).push_str(text);
        } else if in_f > 0 {
            p.formula.get_or_insert_with(String::new).push_str(text);
        } else if in_inline_t {
            p.inline_text.push_str(text);
        }
    }
}

fn parse_worksheet(name: &str, xml: &str) -> Result<SheetData, XlsxError> {
    let mut reader = quick_xml::Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut cells: BTreeMap<(u32, u32), XlsxCell> = BTreeMap::new();

    let mut row_index: u32 = 0;
    let mut col_cursor: u32 = 0;
    let mut pending: Option<PendingCell> = None;
    let mut in_v = 0usize;
    let mut in_f = 0usize;
    let mut in_inline_t = false;
    let mut row_counter: u64 = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Empty(e)) if e.name().as_ref() == "c" => {
                // Self-closing cell (e.g. style-only <c r="A3" s="1"/>):
                // occupies its coordinate for the cursor, but holds no
                // value or formula, so nothing is inserted.
                let coord = parse_cell_coord(&e, row_index, col_cursor)?;
                col_cursor = coord.1.saturating_add(1);
            }
            Ok(quick_xml::events::Event::Start(e)) => match e.name().as_ref() {
                "row" => {
                    row_counter += 1;
                    row_index =
                        match attr_of(e.attributes(), "r").and_then(|r| r.parse::<u32>().ok()) {
                            Some(r1) if r1 >= 1 => r1 - 1,
                            _ => u32::try_from(row_counter - 1).unwrap_or(u32::MAX),
                        };
                    col_cursor = 0;
                }
                "c" => {
                    let coord = parse_cell_coord(&e, row_index, col_cursor)?;
                    col_cursor = coord.1.saturating_add(1);
                    let style = attr_of(e.attributes(), "s").and_then(|s| s.parse().ok());
                    let cell_type =
                        attr_of(e.attributes(), "t").unwrap_or_else(|| String::from("n"));
                    pending = Some(PendingCell {
                        coord,
                        style,
                        cell_type,
                        value_text: None,
                        inline_text: String::new(),
                        formula: None,
                    });
                }
                "v" => in_v += 1,
                "f" => in_f += 1,
                "t" if pending.as_ref().is_some_and(|p| p.cell_type == "inlineStr") => {
                    in_inline_t = true;
                }
                _ => {}
            },
            Ok(quick_xml::events::Event::End(e)) => match e.name().as_ref() {
                "v" => in_v = in_v.saturating_sub(1),
                "f" => in_f = in_f.saturating_sub(1),
                "t" => in_inline_t = false,
                "c" => {
                    if let Some(cell) = pending.take() {
                        if let Some(value) = finish_cell(cell)? {
                            cells.insert(value.0, value.1);
                        }
                    }
                }
                _ => {}
            },
            Ok(quick_xml::events::Event::Text(t)) => {
                let text =
                    quick_xml::escape::unescape(t.as_ref()).map_err(|source| XlsxError::Xml {
                        part: String::from("worksheet"),
                        source: quick_xml::errors::Error::from(source),
                    })?;
                push_text(&mut pending, in_v, in_f, in_inline_t, &text);
            }
            Ok(quick_xml::events::Event::GeneralRef(r)) => {
                // `&lt;`-style references arrive as their own event in
                // quick-xml 0.42; re-wrap and resolve (predefined + numeric).
                let raw = format!("&{};", r.as_ref());
                let resolved =
                    quick_xml::escape::unescape(&raw).map_err(|source| XlsxError::Xml {
                        part: String::from("worksheet"),
                        source: quick_xml::errors::Error::from(source),
                    })?;
                push_text(&mut pending, in_v, in_f, in_inline_t, &resolved);
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(_) => {}
            Err(source) => {
                return Err(XlsxError::Xml {
                    part: String::from("worksheet"),
                    source,
                })
            }
        }
        buf.clear();
    }

    Ok(SheetData {
        name: String::from(name),
        cells,
    })
}

/// Turns a finished `<c>` parse into an insertable cell: `None` when the
/// cell holds neither value nor formula (blank by absence).
/// A finished cell ready for insertion: coordinate plus cell.
type FinishedCell = ((u32, u32), XlsxCell);

fn finish_cell(mut cell: PendingCell) -> Result<Option<FinishedCell>, XlsxError> {
    let reference = sheet_core::refs::cell_name(cell.coord);
    let value = if cell.cell_type == "inlineStr" {
        XlsxValue::InlineString(std::mem::take(&mut cell.inline_text))
    } else {
        match (cell.value_text.as_deref(), cell.cell_type.as_str()) {
            (Some(text), "n") => {
                XlsxValue::Number(text.parse::<f64>().map_err(|_| XlsxError::CorruptCell {
                    reference: reference.clone(),
                    reason: "unparseable number",
                })?)
            }
            (Some(text), "s") => {
                let index: u32 = text.parse().map_err(|_| XlsxError::CorruptCell {
                    reference: reference.clone(),
                    reason: "unparseable shared-string index",
                })?;
                XlsxValue::SharedString(index)
            }
            // Empty `<v></v>` produces no text event; string-ish types
            // tolerate that as the empty string.
            (text, "str") => XlsxValue::FormulaString(String::from(text.unwrap_or_default())),
            (Some(text), "b") => match text {
                "1" | "true" => XlsxValue::Boolean(true),
                "0" | "false" => XlsxValue::Boolean(false),
                _ => {
                    return Err(XlsxError::CorruptCell {
                        reference: reference.clone(),
                        reason: "unparseable boolean",
                    })
                }
            },
            (text, "e") => XlsxValue::Error(String::from(text.unwrap_or_default())),
            // Formula with no cached value: an empty string result marker
            // (documented limitation — see crate docs).
            (None, _) if cell.formula.is_some() => XlsxValue::FormulaString(String::new()),
            // Neither value nor formula: absent cell.
            (None, _) => return Ok(None),
            (Some(_), other) => {
                return Err(XlsxError::CorruptCell {
                    reference: reference.clone(),
                    reason: match other {
                        "e" => "bad error literal",
                        _ => "unsupported cell type",
                    },
                })
            }
        }
    };
    Ok(Some((
        cell.coord,
        XlsxCell {
            value,
            style_index: cell.style,
            formula: cell.formula,
        },
    )))
}

/// Fetches an attribute by exact (unqualified) name.
fn attr_of(attrs: quick_xml::events::attributes::Attributes<'_>, key: &str) -> Option<String> {
    for attr in attrs {
        let attr = attr.ok()?;
        if attr.key.as_ref() == key {
            // Attribute values are escaped text; unescape to plain text.
            return attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
    }
    None
}

/// Fetches a namespaced attribute by local name (e.g. `r:id`).
fn local_attr(e: &quick_xml::events::BytesStart<'_>, local: &str) -> Option<String> {
    for attr in e.attributes() {
        let attr = attr.ok()?;
        if attr.key.local_name().as_ref() == local {
            return attr
                .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                .ok()
                .map(std::borrow::Cow::into_owned);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{normalize_path, rels_path_for, resolve_target};

    #[test]
    fn path_resolution() {
        assert_eq!(
            rels_path_for("xl/workbook.xml"),
            "xl/_rels/workbook.xml.rels"
        );
        assert_eq!(rels_path_for("workbook.xml"), "_rels/workbook.xml");
        assert_eq!(
            resolve_target("xl", "worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_target("xl", "/xl/worksheets/sheet1.xml"),
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            resolve_target("xl", "../sharedStrings.xml"),
            "sharedStrings.xml"
        );
        assert_eq!(resolve_target("", "workbook.xml"), "workbook.xml");
        assert_eq!(normalize_path("a/./b/../c"), "a/c");
    }
}
