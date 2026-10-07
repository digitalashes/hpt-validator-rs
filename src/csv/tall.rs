//! Tall shape: payer and plan are columns of their own, so each row is one charge group.

use super::{estimated_amount, same};
use crate::Finding;

/// Tall's expected columns between the code pairs and the version additions.
pub const COLUMNS: [&str; 6] = [
    "payer_name",
    "plan_name",
    "standard_charge | negotiated_dollar",
    "standard_charge | negotiated_percentage",
    "standard_charge | negotiated_algorithm",
    "standard_charge | methodology",
];

/// Upstream's `areMyColumnsTall`: both `payer_name` and `plan_name` are present.
pub fn detect(row: &[&str]) -> bool {
    ["payer_name", "plan_name"]
        .iter()
        .all(|name| row.iter().any(|c| same(c, name)))
}

/// The 3.0 alert for a leftover `estimated_amount` column.
pub fn alerts(row: &[&str]) -> Vec<Finding> {
    let column = row.iter().position(|c| same(c, "estimated_amount"));
    column
        .map(|i| estimated_amount(i, row[i]))
        .into_iter()
        .collect()
}
