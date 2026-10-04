# Shared GNU Make rules: set SOURCE, layers and options before including.
# Geometry and part identities belong in .copper, never in these recipes.
.DEFAULT_GOAL := all
PYTHON ?= uv run --no-sync python
COPPER ?= $(PYTHON) -m copperscript
PROJECT_ROOT ?= $(CURDIR)
SOURCE ?= board.copper
BOARD_NAME ?= $(basename $(notdir $(SOURCE)))
OUT ?= $(PROJECT_ROOT)/build/$(BOARD_NAME)
LAYERS ?= 2
FAB_PROFILE ?= generic
RESOLVE_ARGS ?= --locked
PROFILE ?= cprofile
KICAD_CLI ?=
KICAD_FOOTPRINTS ?= $(KICAD10_FOOTPRINT_DIR)
PHYSICAL_ARGS ?=
PLACEMENT_TEMPLATES ?=
HARD_MACRO ?=
ROUTE_ARGS ?= --candidates 1 --placement-candidate candidate-00 --feedback-iterations 1 \
 --router-iterations 5 --pitch-mm 1 --passes 2 --search-budget 20000 --progress \
 --soft-ripup --fanout --constrained-pins-first --progressive-guides \
 --repair-budget-multiplier 10 --plane-contact-radius-mm 5
EXTRA_ROUTE_ARGS ?=
BUILD_ARGS ?=
EDITOR_ARGS ?=
KICAD_PYTHON ?= /usr/bin/python3

_paths = $(if $(strip $(KICAD_CLI)),--kicad-cli "$(KICAD_CLI)") $(if $(strip $(KICAD_FOOTPRINTS)),--kicad-footprints "$(KICAD_FOOTPRINTS)")
_physical = --layers $(LAYERS) --fab-profile $(FAB_PROFILE) \
 $(if $(strip $(KICAD_FOOTPRINTS)),--footprint-root "$(KICAD_FOOTPRINTS)") \
 $(if $(strip $(PLACEMENT_TEMPLATES)),--placement-templates "$(PLACEMENT_TEMPLATES)") \
 $(if $(strip $(HARD_MACRO)),--hard-macro "$(HARD_MACRO)") $(PHYSICAL_ARGS)

.PHONY: all check compile layout edit route editor-overlay
all: check compile
check:
	$(COPPER) check "$(SOURCE)" $(RESOLVE_ARGS)
compile:
	$(COPPER) compile "$(SOURCE)" $(RESOLVE_ARGS) -o "$(OUT)/board.json"
layout:
	$(COPPER) plan-layout "$(SOURCE)" $(RESOLVE_ARGS) $(_physical) -o "$(OUT)/placed.kicad_pcb" --report "$(OUT)/placement.json"
edit:
	$(COPPER) edit-mechanical "$(SOURCE)" $(RESOLVE_ARGS) $(_physical) $(EDITOR_ARGS)
editor-overlay:
	$(if $(strip $(RUN_DIR)),,$(error Set RUN_DIR to an existing completed routing run))
	$(COPPER) editor-overlay "$(RUN_DIR)/run.json" --kicad-python "$(KICAD_PYTHON)" -o "$(RUN_DIR)/editor-overlay.json"
# Atomic route + saved fill + native DRC. Every default run has a fresh directory.
# RUN_DIR selects a new/empty path; intentionally no recursive clean target.
route:
	$(PYTHON) -u -m pcbir.build "$(SOURCE)" --project-root "$(PROJECT_ROOT)" --output-root "$(OUT)/runs" $(if $(strip $(RUN_DIR)),--output-dir "$(RUN_DIR)") --profile $(PROFILE) $(RESOLVE_ARGS) $(_paths) $(BUILD_ARGS) -- $(_physical) $(ROUTE_ARGS) $(EXTRA_ROUTE_ARGS)
