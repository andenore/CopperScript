SOURCE ?= $(COPPER_ROOT)/examples/nrf52_coin_cell.copper
BOARD_NAME ?= nrf52-coin-cell
LAYERS ?= 6
FAB_PROFILE ?= jlcpcb-six-layer
HARD_MACRO ?= $(COPPER_ROOT)/examples/nrf_antenna_hard_macro.json
PHYSICAL_ARGS += --width-mm 50 --height-mm 40
ROUTE_ARGS ?= --pitch-mm 0.5 --passes 2 --search-budget 20000 --soft-ripup \
 --fanout --package-access-trials 0 --zone-escape-trials 0 --zone-local-ripup-trials 0 \
 --constrained-pins-first --progressive-guides --plane-contact-radius-mm 5 --early-plane-stitch --progress
