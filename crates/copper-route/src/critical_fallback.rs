//! Observes existing connected-fallback decisions, after fresh candidate gates.
//! It does not run search, recompute geometry, or choose a candidate.

use copper_json::Value;

use super::{CriticalNetResult, fallback_rank, object};

const RECORD_LIMIT: usize = 16;

#[derive(Clone, Debug, Default)]
pub struct PairFallbackSelection {
    observed: usize,
    kept_sequence: Option<usize>,
    last_kept_offer: Option<Value>,
    records: Vec<Value>,
    outcome: Option<Value>,
}

fn snapshot(result: &CriticalNetResult) -> Value {
    let rank = fallback_rank(result);
    object(vec![
        ("nets", super::texts(&result.nets)),
        ("lengths_nm", super::ints(&result.lengths_nm)),
        ("skew_nm", Value::from(result.skew_nm)),
        ("warnings", super::texts(&result.warnings)),
        (
            "rank",
            Value::Array(vec![
                Value::Bool(rank.0),
                Value::from(rank.1 as u64),
                Value::from(rank.2),
                Value::from(rank.3),
            ]),
        ),
    ])
}

impl PairFallbackSelection {
    pub(super) fn observe(
        &mut self,
        producer: &str,
        candidate: &CriticalNetResult,
        incumbent: Option<&CriticalNetResult>,
        kept: bool,
    ) {
        debug_assert!(candidate.connected);
        self.observed += 1;
        if kept {
            self.kept_sequence = Some(self.observed);
        }
        if self.records.len() == RECORD_LIMIT && !kept {
            return;
        }
        let reason = match (incumbent, kept) {
            (None, true) => "kept_first_connected_fallback",
            (Some(_), true) => "kept_strictly_better_existing_rank",
            (Some(item), false) if item.warnings.is_empty() => "rejected_incumbent_within_budget",
            _ => "rejected_existing_rank_not_strictly_better",
        };
        let row = object(vec![
            ("sequence", Value::from(self.observed as u64)),
            ("producer", Value::from(producer)),
            ("candidate", snapshot(candidate)),
            ("incumbent_before", incumbent.map_or(Value::Null, snapshot)),
            ("kept", Value::Bool(kept)),
            ("reason", Value::from(reason)),
        ]);
        if kept {
            self.last_kept_offer = Some(row.clone());
        }
        if self.records.len() < RECORD_LIMIT {
            self.records.push(row);
        }
    }

    pub(super) fn finish(mut self, outcome: &CriticalNetResult) -> Option<Self> {
        if self.observed == 0 {
            return None;
        }
        self.outcome = Some(snapshot(outcome));
        Some(self)
    }

    pub fn document(&self) -> Value {
        object(vec![
            (
                "scope",
                Value::from(
                    "route_pair_group fresh-validated connected fallback offers only; excludes invalid/unconnected candidates, within-budget early acceptance, nested exits and lower producer proposals; outcome precedes later stage tuning/repair",
                ),
            ),
            (
                "rank_order",
                super::texts(&[
                    "has_warnings".into(),
                    "warning_count".into(),
                    "skew_nm".into(),
                    "summed_lengths_nm".into(),
                ]),
            ),
            ("record_limit", Value::from(RECORD_LIMIT as u64)),
            ("observed_candidate_count", Value::from(self.observed as u64)),
            ("trace_rows_omitted_count", Value::from((self.observed - self.records.len()) as u64)),
            ("truncated", Value::Bool(self.observed > self.records.len())),
            ("kept_fallback_sequence", self.kept_sequence.map_or(Value::Null, |n| Value::from(n as u64))),
            ("last_kept_offer", self.last_kept_offer.clone().unwrap_or(Value::Null)),
            (
                "trace_rows_scope",
                Value::from(
                    "first bounded offers; last_kept_offer is a supplementary snapshot and does not reduce trace_rows_omitted_count",
                ),
            ),
            ("records", Value::Array(self.records.clone())),
            ("group_outcome", self.outcome.clone().unwrap_or(Value::Null)),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(lengths: [i64; 2], warnings: &[&str]) -> CriticalNetResult {
        let mut result = CriticalNetResult::new(
            vec!["M".into(), "P".into()],
            true,
            2,
            0,
            lengths.to_vec(),
            (lengths[0] - lengths[1]).abs(),
            Vec::new(),
        );
        result.warnings = warnings.iter().map(|item| (*item).into()).collect();
        result
    }

    #[test]
    fn fallback_selection_records_both_lengths_and_full_warning_rank() {
        // A first-failing length message alone cannot establish the other
        // member's length or skew. Record those exact values and the actual
        // existing decision; do not infer them from the rejection summary.
        let balanced = result([18_098_078, 18_098_078], &["M length", "P length"]);
        let asymmetric = result([18_057_070, 15_899_520], &["M length", "pair skew"]);
        let kept = crate::critical::within_or_shorter(&asymmetric, &balanced);
        assert!(!kept, "existing ranking must preserve a genuinely balanced equal-warning incumbent");
        let mut ledger = PairFallbackSelection::default();
        ledger.observe("validated_coarse_guide", &balanced, None, true);
        ledger.observe("validated_exact_search", &asymmetric, Some(&balanced), kept);
        let document = ledger.finish(&balanced).unwrap().document();
        let rows = document.get("records").unwrap().as_array().unwrap();
        assert_eq!(document.get("kept_fallback_sequence"), Some(&Value::from(1u64)));
        assert_eq!(rows[1].get("reason").and_then(Value::as_str), Some("rejected_existing_rank_not_strictly_better"));
        let candidate = rows[1].get("candidate").unwrap();
        assert_eq!(candidate.get("lengths_nm"), Some(&super::super::ints(&[18_057_070, 15_899_520])));
        assert_eq!(candidate.get("skew_nm"), Some(&Value::from(2_157_550i64)));
        assert_eq!(candidate.get("warnings"), Some(&super::super::texts(&["M length".into(), "pair skew".into()])));
        assert_eq!(
            candidate.get("rank").unwrap().as_array().unwrap(),
            &[Value::Bool(true), Value::from(2u64), Value::from(2_157_550i64), Value::from(33_956_590i64),]
        );
    }

    #[test]
    fn fallback_selection_counts_omitted_offers_and_kept_sequence_exactly() {
        let candidate = result([20, 20], &["M length", "P length"]);
        let mut ledger = PairFallbackSelection::default();
        ledger.observe("validated_exact_search", &candidate, None, true);
        for _ in 1..RECORD_LIMIT + 2 {
            ledger.observe("validated_exact_search", &candidate, Some(&candidate), false);
        }
        let better = result([19, 19], &["M length", "P length"]);
        assert!(crate::critical::within_or_shorter(&better, &candidate));
        ledger.observe("validated_exact_search", &better, Some(&candidate), true);
        // A later compliant early acceptance is not itself a fallback offer.
        // Preserve the omitted last fallback independently from that outcome.
        let compliant = result([10, 10], &[]);
        let document = ledger.finish(&compliant).unwrap().document();
        assert_eq!(document.get("records").unwrap().as_array().unwrap().len(), RECORD_LIMIT);
        assert_eq!(document.get("observed_candidate_count"), Some(&Value::from(19u64)));
        assert_eq!(document.get("trace_rows_omitted_count"), Some(&Value::from(3u64)));
        assert_eq!(document.get("truncated"), Some(&Value::Bool(true)));
        assert_eq!(document.get("kept_fallback_sequence"), Some(&Value::from(19u64)));
        let last = document.get("last_kept_offer").unwrap();
        assert_eq!(last.get("sequence"), Some(&Value::from(19u64)));
        assert_eq!(last.get("reason").and_then(Value::as_str), Some("kept_strictly_better_existing_rank"));
        assert_eq!(last.get("candidate").unwrap().get("lengths_nm"), Some(&super::super::ints(&[19, 19])));
        assert_eq!(last.get("candidate").unwrap().get("warnings").unwrap().as_array().unwrap().len(), 2);
        assert_eq!(
            last.get("incumbent_before").unwrap().get("rank").unwrap().as_array().unwrap()[3],
            Value::from(40i64)
        );
        assert_eq!(last.get("candidate").unwrap().get("rank").unwrap().as_array().unwrap()[3], Value::from(38i64));
        assert_eq!(document.get("group_outcome").unwrap().get("lengths_nm"), Some(&super::super::ints(&[10, 10])));
        assert_eq!(document.get("group_outcome").unwrap().get("warnings").unwrap().as_array().unwrap().len(), 0);
    }
}
