SOURCE ?= $(COPPER_ROOT)/examples/full_vertical/board.copper
BOARD_NAME ?= full-vertical
LAYERS ?= 6
FAB_PROFILE ?= jlcpcb-six-layer
HARD_MACROS ?= $(COPPER_ROOT)/examples/full_vertical/buck-macro.json \
 $(COPPER_ROOT)/examples/full_vertical/nrf-antenna-macro.json
ROUTE_CANDIDATES ?= 3
# Candidate 00 preserves the short USB corridor with both macros and top fill.
EXTRA_ROUTE_ARGS ?= --placement-candidate candidate-00
