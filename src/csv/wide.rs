//! Wide shape: every payer/plan has its own columns, named `kind | payer | plan`, so each
//! row holds one charge group per payer/plan.

use super::{Version, amounts, estimated_amount, placeholder, same, sep_eq};
use crate::{Finding, js};

const CHARGES: [&str; 4] = [
    "negotiated_dollar",
    "negotiated_percentage",
    "negotiated_algorithm",
    "methodology",
];

/// Payer and plan of a column named the wide way, and whether it is a four-part charge
/// column: `standard_charge | payer | plan | charge` or `kind | payer | plan`.
fn split(v: Version, column: &str) -> Option<(&str, &str, bool)> {
    let parts: Vec<&str> = column.split('|').map(js::trim).collect();
    match parts[..] {
        [sc, payer, plan, charge]
            if same(sc, "standard_charge") && CHARGES.iter().any(|c| same(c, charge)) =>
        {
            Some((payer, plan, true))
        }
        [kind, payer, plan]
            if ["additional_payer_notes"]
                .iter()
                .chain(amounts(v))
                .any(|k| same(k, kind)) =>
        {
            Some((payer, plan, false))
        }
        _ => None,
    }
}

/// Upstream's `getPayersPlans`: `payer | plan` keys, lower-cased (the cause of F-27), each
/// once, in the order of first appearance.
pub fn payers_plans(v: Version, row: &[&str]) -> Vec<String> {
    let mut keys = Vec::new();
    for column in row {
        if let Some((payer, plan, _)) = split(v, column) {
            let key = js::to_lower(&format!("{payer} | {plan}"));
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }
    keys
}

/// Upstream's `findPayersPlansPlaceholders` (3.0): a charge column with either placeholder,
/// any other with both (F-24).
pub fn placeholders(v: Version, row: &[&str]) -> Vec<Finding> {
    let is_placeholder = |column| {
        split(v, column).is_some_and(|(payer, plan, four)| {
            let (payer, plan) = (same(payer, "[payer_name]"), same(plan, "[plan_name]"));
            if four { payer || plan } else { payer && plan }
        })
    };
    (row.iter().enumerate())
        .filter(|(_, c)| is_placeholder(c))
        .map(|(i, c)| placeholder(i, c))
        .collect()
}

/// Whether a payer/plan key has either placeholder, which drops it from the list.
pub fn is_placeholder(key: &str) -> bool {
    let (payer, plan) = key.split_once(" | ").unwrap();
    same(payer, "[payer_name]") || same(plan, "[plan_name]")
}

/// Wide's expected columns between the code pairs and the version additions.
pub fn columns(payers_plans: &[String]) -> impl Iterator<Item = String> {
    payers_plans.iter().flat_map(|pp| {
        let charges = CHARGES.map(|c| format!("standard_charge | {pp} | {c}"));
        charges
            .into_iter()
            .chain([format!("additional_payer_notes | {pp}")])
    })
}

/// The version's amount columns for every payer/plan, after the drug columns.
pub fn amount_columns(v: Version, payers_plans: &[String]) -> impl Iterator<Item = String> {
    (payers_plans.iter()).flat_map(move |pp| amounts(v).iter().map(move |a| format!("{a} | {pp}")))
}

/// The 3.0 alerts for leftover `estimated_amount | payer | plan` columns.
pub fn alerts(row: &[&str], payers_plans: &[String]) -> Vec<Finding> {
    (payers_plans.iter())
        .filter_map(|pp| {
            let name = format!("estimated_amount | {pp}");
            row.iter().position(|c| sep_eq(c, &name))
        })
        .map(|i| estimated_amount(i, row[i]))
        .collect()
}
