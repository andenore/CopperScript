# Routed prototype only. This does NOT generate a production release.
COPPER ?= uv run --no-sync copper
PYTHON ?= uv run --no-sync python
KICAD_FOOTPRINTS ?= /usr/share/kicad/footprints
KICAD_CLI ?= kicad-cli
OUT ?= ../../build/cm4-baseboard
ROUTE_OUT ?= $(OUT)/routed
SOURCE := board.copper
PHYSICAL := --locked --layers 4 --fab-profile jlcpcb-four-layer --footprint-root "$(KICAD_FOOTPRINTS)"

.PHONY: all check compile layout edit route
all: check layout

check:
	$(COPPER) check $(SOURCE) --locked

compile:
	$(COPPER) compile $(SOURCE) --locked -o $(OUT)/board.json

layout:
	$(COPPER) plan-layout $(SOURCE) $(PHYSICAL) -o $(OUT)/cm4_baseboard.kicad_pcb --report $(OUT)/placement.json

edit:
	$(COPPER) edit-mechanical $(SOURCE) $(PHYSICAL)

# Select a new/empty ROUTE_OUT for each run; never overwrite previous evidence.
route:
	$(PYTHON) ../../scripts/route_cm4.py --kicad-cli "$(KICAD_CLI)" --kicad-footprints "$(KICAD_FOOTPRINTS)" --output-dir "$(abspath $(ROUTE_OUT))"
