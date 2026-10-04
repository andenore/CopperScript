SOURCE ?= $(COPPER_ROOT)/examples/cm4_baseboard/board.copper
BOARD_NAME ?= cm4-baseboard
LAYERS ?= 4
FAB_PROFILE ?= jlcpcb-four-layer
EXTRA_ROUTE_ARGS += --fanout-maze
