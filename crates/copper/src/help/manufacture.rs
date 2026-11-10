//! Help for the commands that export, check and manufacture a board.

use super::{Action, Command, Group, Opt};

pub const COMMANDS: &[Command] = &[
    Command {
        name: "export-kicad",
        group: Group::Manufacture,
        summary: "generate a KiCad 8 schematic",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "write the schematic to OUTPUT (default: the generated schematic's file name)",
            },
            Opt { flag: "--no-check", help: "write the schematic even when ERC reports errors" },
        ],
        actions: &[],
    },
    Command {
        name: "export-kicad-pcb",
        group: Group::Manufacture,
        summary: "generate a KiCad 8 PCB draft using resolved footprints",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "write the PCB to OUTPUT, with its .kicad_pro, fp-lib-table and CopperScript.pretty/ beside it \
                       (default: the generated PCB's file name)",
            },
            Opt { flag: "--no-check", help: "write the PCB even when ERC reports errors" },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "also search this directory for footprints; repeatable",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated proxy pads instead of resolving footprints (for backend development)",
            },
            Opt { flag: "--layers {2,4,6}", help: "copper layer count (default: 2)" },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication rule profile (default: generic)",
            },
            Opt {
                flag: "--width-mm WIDTH_MM",
                help: "width of the rectangular outline used when the source has no mechanical block (default: 100.0)",
            },
            Opt {
                flag: "--height-mm HEIGHT_MM",
                help: "height of the rectangular outline used when the source has no mechanical block (default: 80.0)",
            },
            Opt {
                flag: "--hard-macro HARD_MACRO",
                help: "apply a hard-macro scene JSON (legacy bindings); repeatable",
            },
            Opt {
                flag: "--placement-templates PLACEMENT_TEMPLATES",
                help: "apply the rigid placement templates of this JSON scene",
            },
        ],
        actions: &[],
    },
    Command {
        name: "fill",
        group: Group::Manufacture,
        summary: "fill the copper zones of a .kicad_pcb natively",
        options: &[
            Opt {
                flag: "--project PROJECT",
                help: "the .kicad_pro with the design rules (default: the board's same-stem .kicad_pro)",
            },
            Opt {
                flag: "--rules RULES",
                help: "the .kicad_dru custom rules (default: the board's same-stem .kicad_dru, if present)",
            },
            Opt { flag: "-o, --output OUTPUT", help: "write the filled board to OUTPUT (required; must not exist)" },
        ],
        actions: &[],
    },
    Command {
        name: "plot",
        group: Group::Manufacture,
        summary: "plot native Gerber X2, Excellon, IPC-D-356 and position files from a filled board",
        options: &[Opt {
            flag: "-o, --output OUTPUT",
            help: "write the files into this new directory (required; must not exist)",
        }],
        actions: &[],
    },
    Command {
        name: "drc",
        group: Group::Manufacture,
        summary: "check a filled .kicad_pcb natively against its design rules",
        options: &[
            Opt {
                flag: "--project PROJECT",
                help: "the .kicad_pro with the design rules (default: the board's same-stem .kicad_pro)",
            },
            Opt {
                flag: "--rules RULES",
                help: "the .kicad_dru custom rules (default: the board's same-stem .kicad_dru, if present)",
            },
            Opt { flag: "--report REPORT", help: "write the findings as JSON to REPORT" },
            Opt {
                flag: "--cam CAM_DIR",
                help: "also read back the Gerbers and drills under CAM_DIR and check them for shorts and opens \
                       against the board netlist",
            },
            Opt {
                flag: "--expected-open-ends REPORT",
                help: "excuse the dangling track ends a copper panelize report lists, at that exact point on that \
                       net and layer only",
            },
        ],
        actions: &[],
    },
    Command {
        name: "render",
        group: Group::Manufacture,
        summary: "draw the Gerbers of a plot directory (copper plot, or KiCad's) as one SVG",
        options: &[
            Opt { flag: "-o, --output OUTPUT", help: "write the SVG to OUTPUT (required)" },
            Opt {
                flag: "--layer LAYER[&WITHIN][:COLOR[:OPACITY]]",
                help: "draw a plate, in order, bottom first; &WITHIN draws LAYER only where WITHIN is dark; COLOR is \
                       #rgb or #rrggbb (default: KiCad's colour), OPACITY 0..1 (default: 1); repeatable, one required",
            },
            Opt { flag: "--background COLOR", help: "paint this #rgb or #rrggbb colour under the board profile" },
            Opt { flag: "--margin MM", help: "margin around the board profile, in mm (default: 1.0)" },
            Opt {
                flag: "--title TITLE",
                help: "the picture's title (default: the plot directory's name and the layers)",
            },
        ],
        actions: &[],
    },
    Command {
        name: "compare-cam",
        group: Group::Manufacture,
        summary: "compare two manufacturing file sets",
        options: &[
            Opt { flag: "--pitch-um PITCH_UM", help: "raster pitch of the Gerber comparison, in um (default: 10)" },
            Opt {
                flag: "--erosion PIXELS",
                help: "erosion radius in pixels applied to the raster difference (default: 1)",
            },
            Opt { flag: "--hole-tolerance-um UM", help: "drill position and diameter tolerance, in um (default: 2)" },
            Opt { flag: "--report REPORT", help: "write the comparison report as JSON to REPORT" },
        ],
        actions: &[],
    },
    Command {
        name: "export-manufacturing",
        group: Group::Manufacture,
        summary: "export native-DRC-clean KiCad manufacturing files without independent CAM qualification",
        options: &[
            Opt { flag: "--kicad-cli KICAD_CLI", help: "the kicad-cli program to run (default: kicad-cli)" },
            Opt {
                flag: "--skip-independent-cam",
                help: "required acknowledgement that the files are not independently CAM-qualified",
            },
            Opt {
                flag: "--drc {kicad,native}",
                help: "refill and check with kicad-cli, or with copper fill and copper drc plus a read-back of the \
                       plotted files (default: kicad)",
            },
            Opt {
                flag: "--plotter {kicad,native}",
                help: "plot with kicad-cli or with CopperScript's own plotter (default: kicad)",
            },
            Opt {
                flag: "--bom BOM",
                help: "include this reviewed BOM (from copper assembly bom) and a matching JLCPCB CPL",
            },
            Opt {
                flag: "--replace",
                help: "replace a recognized generated output directory once all new exports pass; the previous \
                       output is kept in a .manufacturing-previous-* sibling",
            },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "the output directory (required; must not exist unless --replace)",
            },
        ],
        actions: &[],
    },
    Command {
        name: "assembly",
        group: Group::Manufacture,
        summary: "pin and check explicit assembly selections offline",
        options: &[],
        actions: &[
            Action {
                name: "snapshot",
                summary: "create an unreviewed assembly lock from the compiled board",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "require every dependency to be pinned in copper.lock and verify it without rewriting \
                               the lock",
                    },
                    Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
                    Opt { flag: "-o, --output OUTPUT", help: "the new assembly lock (required; must not exist)" },
                ],
            },
            Action {
                name: "update",
                summary: "rebind the lock to the current board; changed components lose their review",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "require every dependency to be pinned in copper.lock and verify it without rewriting \
                               the lock",
                    },
                    Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
                    Opt { flag: "--lock LOCK", help: "the assembly lock to update in place (required)" },
                ],
            },
            Action {
                name: "check",
                summary: "check the lock's selections against the board",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "require every dependency to be pinned in copper.lock and verify it without rewriting \
                               the lock",
                    },
                    Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
                    Opt { flag: "--lock LOCK", help: "the assembly lock (required)" },
                    Opt { flag: "--report REPORT", help: "also write the check report as JSON to REPORT" },
                ],
            },
            Action {
                name: "bom",
                summary: "write the supplier selection BOM from a passing lock",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "require every dependency to be pinned in copper.lock and verify it without rewriting \
                               the lock",
                    },
                    Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
                    Opt { flag: "--lock LOCK", help: "the assembly lock (required)" },
                    Opt { flag: "-o, --output OUTPUT", help: "the supplier BOM CSV (required)" },
                    Opt {
                        flag: "--hand-assembly HAND_ASSEMBLY",
                        help: "also write the components the supplier will not place: consigned selections and \
                               those to leave empty",
                    },
                    Opt { flag: "--ordering ORDERING", help: "also write one ordering sheet listing every selection" },
                    Opt {
                        flag: "--assembly-sheet ASSEMBLY_SHEET",
                        help: "also write a copperscript.assembly-sheet/v1 file for CopperReview's Assembly mode",
                    },
                ],
            },
            Action {
                name: "candidate-bom",
                summary: "write an explicitly unapproved assembly review bundle",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "require every dependency to be pinned in copper.lock and verify it without rewriting \
                               the lock",
                    },
                    Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
                    Opt { flag: "--lock LOCK", help: "the assembly lock (required)" },
                    Opt { flag: "-o, --output OUTPUT", help: "the review bundle directory (required)" },
                    Opt {
                        flag: "--positions POSITIONS",
                        help: "also write an unapproved JLCPCB-subset CPL from this native positions CSV",
                    },
                ],
            },
            Action {
                name: "offers",
                summary: "look up live distributor offers for the lock's fitted parts (networked)",
                options: &[
                    Opt {
                        flag: "--locked",
                        help: "accepted; offers reads only the lock and does not compile the board",
                    },
                    Opt { flag: "--offline", help: "refused: offers needs the network" },
                    Opt { flag: "--lock LOCK", help: "the assembly lock (default: assembly.lock beside the board)" },
                    Opt {
                        flag: "-o, --output OUTPUT",
                        help: "the report (default: build/assembly-offers/offers-TIMESTAMP.json beside the board)",
                    },
                    Opt {
                        flag: "--distributors NAMES",
                        help: "comma-separated distributors to look up (default: the suppliers named in the lock)",
                    },
                    Opt {
                        flag: "--provider NAMES",
                        help: "comma-separated providers to query: digikey, mouser, farnell, nexar \
                               (default: every configured one)",
                    },
                    Opt { flag: "--quantity BOARDS", help: "number of boards the offers must cover (default: 1)" },
                    Opt {
                        flag: "--min-stock N",
                        help: "an offer needs at least N in stock; any shortfall then exits 1",
                    },
                    Opt {
                        flag: "--within-weeks W",
                        help: "an offer may instead have a factory lead time within W weeks; any shortfall then exits 1",
                    },
                    Opt { flag: "--country CC", help: "country for DigiKey and Nexar prices (default: US)" },
                    Opt { flag: "--currency CUR", help: "currency for DigiKey and Nexar prices (default: USD)" },
                    Opt {
                        flag: "--write-alternates",
                        help: "add offers for the same manufacturer part from requested distributors not yet listed \
                               on a selection; nothing else in the lock changes",
                    },
                ],
            },
        ],
    },
    Command {
        name: "panelize",
        group: Group::Manufacture,
        summary: "compose a panel document's routed boards into one panel .kicad_pcb",
        options: &[
            Opt {
                flag: "-o, --output OUTPUT",
                help: "the panel .kicad_pcb; its .kicad_pro and .kicad_dru are written beside it \
                       (required unless --dump-config)",
            },
            Opt {
                flag: "--dump-config",
                help: "print the effective panel values with their origins and the preset's rules, without composing",
            },
            Opt {
                flag: "--report REPORT",
                help: "write the copperscript-panel/v0.1 report here (default: OUTPUT with a .panel.json extension)",
            },
            Opt { flag: "--strict", help: "exit 1 when the panel fails a rule of its fab preset" },
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
        ],
        actions: &[],
    },
    Command {
        name: "models",
        group: Group::Manufacture,
        summary: "the 3D models a board's parts name by https:// URL: list them or fetch the missing ones",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt {
                flag: "--offline",
                help: "resolve dependencies from the local cache only; fetch reports missing models without \
                       downloading them",
            },
        ],
        actions: &[],
    },
];
