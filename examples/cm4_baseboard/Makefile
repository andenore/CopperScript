# Settings only; the same rules also build other boards and standalone projects.
COPPER_ROOT := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))/../..)
PROJECT_ROOT := $(COPPER_ROOT)
include $(COPPER_ROOT)/make/examples/cm4.mk
include $(COPPER_ROOT)/make/board.mk
