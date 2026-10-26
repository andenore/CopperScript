SOURCE ?= $(COPPER_ROOT)/examples/nrf52_coin_cell/board.copper
BOARD_NAME ?= nrf52-coin-cell
LAYERS ?= 6
FAB_PROFILE ?= jlcpcb-six-layer
PHYSICAL_ARGS += --width-mm 50 --height-mm 40
ROUTE_ARGS ?= --pitch-mm 0.5 --fanout --plane-contact-radius-mm 5 --early-plane-stitch --progress
