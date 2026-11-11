//! Help for the commands that review a routing or layout run.

use super::{Command, Group, Opt};

pub const COMMANDS: &[Command] = &[
    Command {
        name: "issues",
        group: Group::Review,
        summary: "list a run's failures grouped by cause, each with an id, nets, layers and window",
        options: &[
            Opt { flag: "--issue ID", help: "show only the issue with this id" },
            Opt { flag: "--net NET", help: "only issues on this net" },
            Opt {
                flag: "--stage {global,access,critical,native,physical,routing,layout,placement,review,emi,error}",
                help: "only issues from this stage",
            },
            Opt { flag: "--code CODE", help: "only issues with this code (case-insensitive)" },
            Opt {
                flag: "--limit LIMIT",
                help: "most issues listed, balanced across stages; 0 lists all (default: 15)",
            },
            Opt {
                flag: "--cluster-mm CLUSTER_MM",
                help: "distance in mm within which findings group into one issue; it changes grouping and ids, \
                       so pass the same value to view (default: 3.0)",
            },
            Opt { flag: "--json", help: "emit a copperscript.run-issues/v1 document" },
        ],
        actions: &[],
    },
    Command {
        name: "view",
        group: Group::Review,
        summary: "render a board window per copper layer: an issue, net, pad, component or explicit window",
        options: &[
            Opt { flag: "--issue ISSUE", help: "frame the issue with this id (needs a run directory or report)" },
            Opt { flag: "--net NET", help: "frame the copper of this net; repeatable" },
            Opt { flag: "--pad PAD", help: "frame one pad, given as REF.PAD" },
            Opt { flag: "--component COMPONENT", help: "frame every pad of this component" },
            Opt { flag: "--window WINDOW", help: "frame an explicit rectangle X0,Y0,X1,Y1 in mm" },
            Opt { flag: "--highlight HIGHLIGHT", help: "also highlight this net; repeatable" },
            Opt {
                flag: "--layers LAYERS",
                help: "auto (layers with highlighted copper), all, or a comma-separated list of copper layers \
                       (default: auto)",
            },
            Opt { flag: "--margin MARGIN", help: "margin around the framed geometry, in mm (default: 1.5)" },
            Opt {
                flag: "--cluster-mm CLUSTER_MM",
                help: "issue grouping distance in mm; use the value given to issues (default: 3.0)",
            },
            Opt { flag: "--filled", help: "draw the run's saved native (refilled) board; does not refill" },
            Opt {
                flag: "--emi-field",
                help: "with --issue of an EMI-HOT-LOOP finding: draw the quasi-static magnetic near-field map of the loop (|H| in dB(uA/m) in a plane above the board, level at sensitive pads) instead of the layer panels; a screening estimate, the inductor's own field is not included",
            },
            Opt {
                flag: "--field-height FIELD_HEIGHT",
                help: "with --emi-field: height of the map plane above the top copper, e.g. 1mm (default: 1mm)",
            },
            Opt {
                flag: "--reference REFERENCE",
                help: "overlay a second .kicad_pcb as magenta outlines, matched by KiCad layer name; needs --align",
            },
            Opt {
                flag: "--align ALIGN",
                help: "align the reference on the pads of OURS[:THEIRS] (component references); mirroring is tried",
            },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "write the SVG here, with its suffix replaced by .svg (default: build/views/LABEL.svg)",
            },
            Opt { flag: "--json", help: "print the image path and legend as JSON" },
        ],
        actions: &[],
    },
    Command {
        name: "summarize",
        group: Group::Review,
        summary: "read a compact engineering summary of a retained route/layout report",
        options: &[
            Opt { flag: "--json", help: "print the summary as JSON" },
            Opt { flag: "--issues {0,1,...,100}", help: "most issues listed (default: 8)" },
            Opt {
                flag: "--net NET",
                help: "show the saved per-net copper metrics of NET, including partial stubs; \
                       it does not reroute or verify",
            },
        ],
        actions: &[],
    },
    Command {
        name: "state",
        group: Group::Review,
        summary: "inspect copperscript.state files (routing checkpoints and saved solver inputs)",
        options: &[],
        actions: &[],
    },
    Command {
        name: "replay-unified",
        group: Group::Review,
        summary: "route a --unified-save-inputs state again with option overrides and summarise it",
        options: &[
            Opt {
                flag: "--set OPTION=VALUE",
                help: "override a saved unified router option (VALUE is read as JSON, else as text); repeatable",
            },
            Opt { flag: "--report REPORT", help: "also write the JSON summary to REPORT" },
            Opt {
                flag: "--save-result SAVE_RESULT",
                help: "save the routing result as a new state file; refuses an existing file",
            },
            Opt { flag: "--only NET", help: "route only this net; repeatable; it must be within the saved selection" },
            Opt { flag: "--component REF", help: "also route every net touching this component; repeatable" },
            Opt {
                flag: "--refanout",
                help: "plan the ball-grid fanout again before routing (not implemented yet: exits 3)",
            },
            Opt {
                flag: "--fanout-set OPTION=VALUE",
                help: "a fanout option (surface_rings, stub_width_nm or array_flow; VALUE as JSON); implies \
                       --refanout; repeatable",
            },
            Opt { flag: "-o, --output OUTPUT", help: "write the routed board as a KiCad project to OUTPUT" },
            Opt { flag: "--progress", help: "print a PROGRESS JSON line for each router event" },
            Opt {
                flag: "--exact-escape REF",
                help: "run the exact escape for component REF (not implemented yet: exits 3)",
            },
            Opt {
                flag: "--power-islands auto|LAYER",
                help: "plan power islands on LAYER (auto: the board's power layer) and add their zones before routing",
            },
        ],
        actions: &[],
    },
    Command {
        name: "replay-critical",
        group: Group::Review,
        summary: "bounded critical-group diagnostic from a placement checkpoint or source-only inputs",
        options: &[
            Opt { flag: "--only NET", help: "replay only the critical group of this net; repeatable" },
            Opt {
                flag: "--pair-states PAIR_STATES",
                help: "bound on the aggregate pair search states (default: 6000)",
            },
            Opt { flag: "--report REPORT", help: "write the copperscript-critical-replay/v1 report to REPORT" },
            Opt { flag: "-o, --output OUTPUT", help: "write the resulting board as a KiCad project to OUTPUT" },
        ],
        actions: &[],
    },
];
