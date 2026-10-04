//! The shared-string table.
//!
//! `SpreadsheetML` stores repeated text once in `xl/sharedStrings.xml` and
//! references it by index (`<c t="s"><v>0</v></c>`). [`SharedStrings`] is
//! that table: append-or-dedup on the write path ([`insert`](Self::insert)
//! returns the index), positional lookup on the read path
//! ([`get`](Self::get)).

use std::string::String;
use std::vec::Vec;

/// An ordered, de-duplicating string table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SharedStrings {
    strings: Vec<String>,
}

impl SharedStrings {
    /// Returns the text at `index`, or `None` when out of range.
    #[must_use]
    pub fn get(&self, index: u32) -> Option<&str> {
        // The index comes from untrusted file input; `.get` keeps the
        // lookup total.
        self.strings.get(index as usize).map(String::as_str)
    }

    /// Appends `s` (de-duplicated) and returns its index.
    pub fn insert(&mut self, s: &str) -> u32 {
        if let Some(pos) = self.strings.iter().position(|t| t == s) {
            return u32::try_from(pos).unwrap_or(u32::MAX);
        }
        self.strings.push(String::from(s));
        u32::try_from(self.strings.len() - 1).unwrap_or(u32::MAX)
    }

    /// Number of distinct strings in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.strings.len()
    }

    /// `true` when the table holds no strings.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }

    /// Iterates the table in index order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.strings.iter().map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::SharedStrings;

    #[test]
    fn insert_dedup_and_get() {
        let mut t = SharedStrings::default();
        assert_eq!(t.insert("a"), 0);
        assert_eq!(t.insert("b"), 1);
        assert_eq!(t.insert("a"), 0);
        assert_eq!(t.len(), 2);
        assert_eq!(t.get(0), Some("a"));
        assert_eq!(t.get(1), Some("b"));
        assert_eq!(t.get(2), None);
        assert_eq!(t.iter().collect::<Vec<_>>(), vec!["a", "b"]);
    }
}
