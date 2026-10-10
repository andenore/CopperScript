"""Read-only probe run by explicitly selected KiCad Python; stdlib + pcbnew.

No CopperScript package import in the native interpreter. No refill/save.
"""
import json
import sys
import hashlib
import importlib.util
from pathlib import Path


def extract(filename):
    import pcbnew
    from editor.native_overlay import extract as geometry
    result = geometry(filename)
    board = pcbnew.LoadBoard(filename)
    point = lambda value: [int(value.x), int(value.y)]
    drills, contacts, unsupported = [], [], []
    for item in board.GetTracks():
        if isinstance(item, pcbnew.PCB_VIA):
            if item.GetViaType() != pcbnew.VIATYPE_THROUGH:
                unsupported.append("blind/buried/microvia drill reconciliation")
            drills.append({"position": point(item.GetPosition()), "diameter_nm": item.GetDrillValue(), "plated": True})
    for footprint in board.GetFootprints():
        for pad in footprint.Pads():
            size = pad.GetDrillSize()
            if size.x or size.y:
                if size.x != size.y:
                    unsupported.append("slotted pad drill reconciliation")
                else:
                    drills.append({"position": point(pad.GetPosition()), "diameter_nm": int(size.x),
                                   "plated": pad.GetAttribute() != pcbnew.PAD_ATTRIB_NPTH})
            if pad.GetNumber() and pad.GetAttribute() != pcbnew.PAD_ATTRIB_NPTH:
                contacts.append({"component": footprint.GetReference(), "pad": pad.GetNumber(),
                                 "net": pad.GetNetname() or "N/C", "position": point(pad.GetPosition()),
                                 "layers": [name for name in result["layers"]
                                            if pad.IsOnLayer(board.GetLayerID(name))]})
    result.update(drills=drills, contacts=contacts, unsupported=sorted(set(unsupported)),
                  ipc_origin_nm=point(board.GetDesignSettings().GetAuxOrigin()),
                  board_thickness_nm=int(board.GetDesignSettings().GetBoardThickness()),
                  native_tool_files={"python_sha256": hashlib.sha256(Path(sys.executable).read_bytes()).hexdigest(),
                    "pcbnew_wrapper_sha256": hashlib.sha256(Path(pcbnew.__file__).read_bytes()).hexdigest(),
                    "pcbnew_extension_sha256": hashlib.sha256(Path(importlib.util.find_spec("_pcbnew").origin).read_bytes()).hexdigest()})
    return result


if __name__ == "__main__":
    try:
        if len(sys.argv) != 2:
            raise ValueError("one explicit native PCB path required")
        print(json.dumps(extract(sys.argv[1]), allow_nan=False))
    except Exception as exc:
        print(str(exc), file=sys.stderr)
        raise SystemExit(2)
