//! Wide shape: every payer/plan has its own columns, named `kind | payer | plan`, so each
//! row holds one charge group per payer/plan.

use super::{CHARGES, amounts, parts, placeholder, same};
use crate::{Finding, Version, js};
use std::collections::HashSet;

/// Payer and plan of a column named the wide way, and whether it is a four-part charge
/// column: `standard_charge | payer | plan | charge` or `kind | payer | plan`.
fn split(v: Version, column: &str) -> Option<(&str, &str, bool)> {
    let parts: Vec<&str> = parts(column).collect();
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

/// Whether payer and plan are the literal placeholders.
fn placeholders(payer: &str, plan: &str) -> (bool, bool) {
    (same(payer, "[payer_name]"), same(plan, "[plan_name]"))
}

/// Upstream's `getPayersPlans` and, for 3.0, `findPayersPlansPlaceholders`: the
/// `payer | plan` keys, lower-cased (the cause of F-27), each once in order of first
/// appearance, and the placeholder errors.
pub fn payers_plans(v: Version, row: &[&str]) -> (Vec<String>, Vec<Finding>) {
    let (mut keys, mut errors, mut seen) = (Vec::new(), Vec::new(), HashSet::new());
    for (i, column) in row.iter().enumerate() {
        let Some((payer, plan, four)) = split(v, column) else {
            continue;
        };
        let key = js::to_lower(&format!("{payer} | {plan}"));
        if v == Version::V3_0 {
            // Reported for a charge column with either placeholder, any other with both
            // (F-24); dropped with either, judged on the lower-cased key as upstream does.
            let (payer, plan) = placeholders(payer, plan);
            let reported = if four { payer || plan } else { payer && plan };
            if reported {
                errors.push(placeholder(i, column));
            }
            let (payer, plan) = key.split_once(" | ").unwrap();
            let (payer, plan) = placeholders(payer, plan);
            if payer || plan {
                continue;
            }
        }
        if seen.insert(key.clone()) {
            keys.push(key);
        }
    }
    (keys, errors)
}
