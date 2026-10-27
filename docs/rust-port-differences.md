# Rust rewrite: recorded differences

The comparison harness (`scripts/compare_implementations.py`) runs every
corpus case on the Python CLI and on the Rust binary. Identical output is the
default expectation ([plan, section 4](rust-port-plan.md#4-approach-clean-rewrite-checked-against-the-python-cli)).
A case whose outputs differ blocks its milestone unless this table records the
difference.

An entry applies to exactly one pair of outputs: the harness matches the case
id and both result digests from its report. When either output changes, the
entry no longer applies and the case is reviewed again.

To record a difference:

1. Run the harness with `--keep DIR` and inspect both outputs.
2. Establish that the Python behaviour is wrong or arbitrary, not that the
   Rust code is. A Rust bug is fixed, not recorded.
3. Add a row with the next id, the case id, both result digests as printed in
   the report, the milestone and the reason. Name the input and what differs.
4. Get the row reviewed with the change that introduces it.

Ladder boards must still route completely and pass native DRC whatever is
recorded here.

| ID | Case | Python result | Rust result | Milestone | Reason |
|---|---|---|---|---|---|
