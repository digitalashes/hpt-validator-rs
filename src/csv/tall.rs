//! Tall shape: payer and plan are columns of their own, so each row is one charge group.

use super::same;

/// Tall's expected columns ahead of its charge group.
pub const COLUMNS: [&str; 2] = ["payer_name", "plan_name"];

/// Upstream's `areMyColumnsTall`: both `payer_name` and `plan_name` are present.
pub fn detect(row: &[&str]) -> bool {
    COLUMNS.iter().all(|name| row.iter().any(|c| same(c, name)))
}
