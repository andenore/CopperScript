# Placement draft only. This target does NOT generate a production release.
COPPER ?= uv run --no-sync copper
KICAD_FOOTPRINTS ?= /usr/share/kicad/footprints
OUT ?= ../../build/cm4-baseboard
SOURCE := board.copper
PHYSICAL := --locked --layers 4 --fab-profile jlcpcb-four-layer --footprint-root "$(KICAD_FOOTPRINTS)"

.PHONY: all check compile layout edit
all: check layout

check:
	$(COPPER) check $(SOURCE) --locked

compile:
	$(COPPER) compile $(SOURCE) --locked -o $(OUT)/board.json

layout:
	$(COPPER) plan-layout $(SOURCE) $(PHYSICAL) -o $(OUT)/cm4_baseboard.kicad_pcb --report $(OUT)/placement.json

edit:
	$(COPPER) edit-mechanical $(SOURCE) $(PHYSICAL)
