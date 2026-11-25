# Coupling probe

The tuned 2.4 GHz monopole of [`antenna_synthesis`](../antenna_synthesis/README.md) on the same 40 x 30 mm,
two-layer board, and a 48 MHz clock (`CLK`) from `U2` to `U3` placed 4 mm below the antenna's region. `U2.OUT`
declares its source as a Thevenin source, `open_circuit_swing = 3.3V` behind `output_impedance = 33ohm`, with 1 ns
edges. The parts are synthetic; nothing here is a qualified radio design or an isolation claim.

Route it with a checkpoint, then ask how much the antenna couples into the routed clock:

```sh
copper route-board examples/coupling/board.copper -o /tmp/coupling/board.kicad_pcb \
    --report /tmp/coupling/route.json --checkpoint-dir /tmp/coupling/checkpoint --locked --offline
copper coupling examples/coupling/board.copper --antenna bt --to CLK \
    --run /tmp/coupling/checkpoint --locked --offline
```

`--describe` prints the model and each end's termination without solving: `U2.1` takes the declared 33 ohm, `U3.1`
the default 50 ohm. The other direction, the clock as the aggressor in the region model, is
`--from CLK --to ANT_FEED`. Accuracy and limits are in
[`docs/coupling-query-plan.md`](../../docs/coupling-query-plan.md).
