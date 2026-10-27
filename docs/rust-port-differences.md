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

An uncaught Python exception is compared by its final exception line only
(`python crash: KeyError: 'X'`): the traceback's paths and line numbers differ
per checkout and platform.Line endings
are not differences: the Rust binary writes LF everywhere and the harness
reads Python's Windows CRLF as LF.

Ladder boards must still route completely and pass native DRC whatever is
recorded here.

| ID | Case | Python result | Rust result | Milestone | Reason |
|---|---|---|---|---|---|
| D-0001 | `audit-footprints/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:231ffcf4bba2dcbc3b7af867f9fc0a752387a2393a2f270b6d9aa25bbea962c7` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0002 | `audit-footprints-json/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:231ffcf4bba2dcbc3b7af867f9fc0a752387a2393a2f270b6d9aa25bbea962c7` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0003 | `audit-footprints-strict/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:231ffcf4bba2dcbc3b7af867f9fc0a752387a2393a2f270b6d9aa25bbea962c7` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0004 | `audit-footprints/examples/simulation/rc_filter.copper#m1` | `sha256:ec51aaed1a594a6e02b39e8de34bbbbdc28b990789a39bfcf492736b06e9825e` | `sha256:6025386b8dddd7212bc40d1e924cdaac44c6b9865a040b3e26fa70e19ab8a783` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0005 | `audit-footprints/examples/valid_board/board.copper#m1` | `sha256:164f5551bf7ac3abece695fa6d901e62bf3f9df14e0013a3f710faef10e7bd52` | `sha256:c462ed1450ea6a3076d62ff36b3b766668a056c7193441ee62f2e1417448f618` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0006 | `lock/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:231ffcf4bba2dcbc3b7af867f9fc0a752387a2393a2f270b6d9aa25bbea962c7` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0007 | `lock/examples/simulation/rc_filter.copper#m1` | `sha256:ec51aaed1a594a6e02b39e8de34bbbbdc28b990789a39bfcf492736b06e9825e` | `sha256:6025386b8dddd7212bc40d1e924cdaac44c6b9865a040b3e26fa70e19ab8a783` | M1 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `FOOTPRINT ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0008 | `export-kicad-pcb-proxy/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0009 | `export-kicad-pcb-proxy/examples/invalid_board/board.copper#m4` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0010 | `export-kicad-pcb-proxy/examples/invalid_board/board.copper#m5` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0011 | `export-kicad-pcb-proxy/examples/invalid_board/board.copper#m6` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0012 | `export-kicad-pcb-proxy/examples/invalid_board/board.copper#m7` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0013 | `export-kicad-pcb-proxy/examples/mechanical_outline/board.copper#m3` | `sha256:ec51aaed1a594a6e02b39e8de34bbbbdc28b990789a39bfcf492736b06e9825e` | `sha256:d21e285b1e30c522d36645f11297920535c92ccc9179084cb83fe2523b51ddbc` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0014 | `export-kicad-pcb-proxy/examples/simulation/rc_filter.copper#m1` | `sha256:ec51aaed1a594a6e02b39e8de34bbbbdc28b990789a39bfcf492736b06e9825e` | `sha256:d21e285b1e30c522d36645f11297920535c92ccc9179084cb83fe2523b51ddbc` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0015 | `export-kicad-pcb-proxy/examples/valid_board/board.copper#m1` | `sha256:164f5551bf7ac3abece695fa6d901e62bf3f9df14e0013a3f710faef10e7bd52` | `sha256:116d742ea3c1089b0775be654facb0f4a665deba4a95e161d510c18275fb16fb` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0016 | `export-kicad-pcb-proxy/examples/valid_board/board.copper#m8` | `sha256:3c49d375d72543d152ccf5c809e5dcd093d3808b29d594770484686ff12f50a5` | `sha256:e7993c22e3b869ff9ed60f8b74ccaf39303700f216484b0d6988661ca46774c7` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0017 | `export-kicad-pcb-proxy/tests/fixtures/editor-artwork/board.copper#m9` | `sha256:2c14eafbb0430cf32ca5b4942cac7d915c98fafef29aeb75bbecbfa49b2e7a91` | `sha256:2039a65ec319a9e72cb4794405fc4a66e92ae87eb1ed6cf80fc55c214985d2fc` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0018 | `export-kicad-pcb-resolved/examples/invalid_board/board.copper` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0019 | `export-kicad-pcb-resolved/examples/simulation/rc_filter.copper#m1` | `sha256:ec51aaed1a594a6e02b39e8de34bbbbdc28b990789a39bfcf492736b06e9825e` | `sha256:d21e285b1e30c522d36645f11297920535c92ccc9179084cb83fe2523b51ddbc` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0020 | `export-kicad-pcb-resolved/examples/valid_board/board.copper#m1` | `sha256:164f5551bf7ac3abece695fa6d901e62bf3f9df14e0013a3f710faef10e7bd52` | `sha256:116d742ea3c1089b0775be654facb0f4a665deba4a95e161d510c18275fb16fb` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0021 | `export-kicad-pcb-fixtures/tests/rust-port/export-kicad-pcb/six_layer.copper#m2` | `sha256:b39082f8e8108e2d75d88cd4ca87ea0477102b1a7d8cb1b7a9282df42f633ca1` | `sha256:500c0d3001d1313f24f2ec2f6241e14b2ec3cd77e00d8cf6084b7d83ffa468bc` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
| D-0022 | `export-kicad-pcb-options/examples/invalid_board/board.copper --locked --offline --allow-proxy-footprints --no-check -o {out}/board.kicad_pcb` | `sha256:8e87092d9d5c5e2b45581b0889b7ef0da7b1e505c2c2aaa2dca4f0f1647585d4` | `sha256:8b3253937757494e5c876deddc8483f188cb8afa1e499f16714da510b6b41cee` | M2 | A component uses an unknown part. Python crashes with an uncaught `KeyError` (exit 1, traceback); Rust prints `BACKEND ERROR: component 'REF' uses unknown part 'PART'` and exits 2. |
