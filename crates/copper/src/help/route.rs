//! Help for the place, route and benchmark commands.
use super::{Command, Group, Opt};

/// `edit-mechanical` and its alias `edit`.
const EDIT_MECHANICAL: &[Opt] = &[
    Opt {
        flag: "--project PROJECT",
        help: "consumer project whose copper.mod pins a library definition's dependencies",
    },
    Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
    Opt { flag: "--offline", help: "reject remote package cache misses" },
    Opt { flag: "--footprint-root FOOTPRINT_ROOT", help: "extra footprint root for the physical build; may repeat" },
    Opt {
        flag: "--layers {2,4,6}",
        help: "copper-layer count; default derives from source requirements; must match a declared stackup",
    },
    Opt {
        flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
        help: "fabrication-rule profile of the physical build (default: generic)",
    },
    Opt { flag: "--placement-templates PLACEMENT_TEMPLATES", help: "apply a placement-template JSON scene" },
    Opt { flag: "--hard-macro HARD_MACRO", help: "apply a legacy hard-macro scene JSON; may repeat" },
    Opt { flag: "--allow-proxy-footprints", help: "explicit inspection-only proxy geometry" },
    Opt { flag: "--no-browser", help: "print the private loopback launch URL without opening a browser" },
    Opt { flag: "--port PORT", help: "loopback port; zero chooses an available port (default: 0)" },
    Opt { flag: "--scene-output SCENE_OUTPUT", help: "write a derived scene and exit without serving" },
    Opt { flag: "--overlay OVERLAY", help: "optional content-bound routed reference JSON" },
];

pub const COMMANDS: &[Command] = &[
    Command {
        name: "route-board",
        group: Group::Layout,
        summary: "attempt complete physical routing and run native DRC",
        options: &[
            // Inputs and project.
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt { flag: "--no-check", help: "route even when ERC reports errors" },
            Opt {
                flag: "--footprint-root DIR",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
            Opt {
                flag: "--placement-templates SCENE",
                help: "apply a placement-template JSON scene (rigid placement clusters)",
            },
            Opt {
                flag: "--hard-macro SCENE",
                help: "apply a legacy hard-macro scene JSON; may repeat (source-bound macros apply automatically)",
            },
            Opt {
                flag: "--routing-profile {quick,balanced,thorough}",
                help: "standard work budgets: filled-copper repair trials and search states, and decoupling \
                       placement trials (quick 2/2000/4, balanced 8/4000/16, thorough 16/12000/64); an explicit \
                       --filled-copper-trials or --decoupling-placement-trials wins (default without a profile: \
                       8/4000/64)",
            },
            // Board and stack-up.
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt {
                flag: "--width-mm MM",
                help: "board rectangle width, used only when the source has no mechanical block (default: 100.0)",
            },
            Opt {
                flag: "--height-mm MM",
                help: "board rectangle height, used only when the source has no mechanical block (default: 80.0)",
            },
            // Placement.
            Opt { flag: "--candidates N", help: "placement candidates to generate (default: 1)" },
            Opt { flag: "--placement-grid-mm MM", help: "positive placement sampling step (default: 1)" },
            Opt {
                flag: "--placement-refinement-passes N",
                help: "optional placement refinement passes; 0 keeps the legalized poses (default: 2)",
            },
            Opt {
                flag: "--escape-spacing-passes N",
                help: "optional escape-spacing improvement passes of placement (default: 2)",
            },
            Opt {
                flag: "--decoupling-placement-trials N",
                help: "copper-access pose trials per decoupling capacitor; 0 disables the relocation heuristic \
                       (default: 64)",
            },
            Opt {
                flag: "--placement-candidate ID",
                help: "use this placement candidate identity (such as candidate-01); an unavailable identity is \
                       an error",
            },
            Opt {
                flag: "--placement-feedback-trials N",
                help: "global placement trials per feedback iteration, unrelated to --congestion-feedback-trials \
                       (default: 24)",
            },
            Opt { flag: "--feedback-iterations N", help: "placement-routing feedback iterations (default: 1)" },
            Opt {
                flag: "--congestion-feedback-trials N",
                help: "bound on congestion-driven placement reruns of the pipeline tail; 0 disables them \
                       (default: 4)",
            },
            // Escape and fanout.
            Opt {
                flag: "--escape-margin-mm MM",
                help: "margin of the soft escape-corridor estimate of placement, not a DRC clearance (default: 0.5)",
            },
            Opt {
                flag: "--escape-transit-lanes N",
                help: "transit lanes reserved by the soft escape-corridor estimate of placement (default: 1)",
            },
            Opt {
                flag: "--fanout, --no-fanout",
                help: "seed the unified router's pad escapes; --no-fanout routes without them (default: on)",
            },
            Opt {
                flag: "--fanout-refinement-step-mm MM",
                help: "finer escape sampling step for empty or conflicting coarse escapes (default: 0.1)",
            },
            Opt { flag: "--fanout-maze", help: "try maze escapes for pins left without an escape choice" },
            Opt {
                flag: "--escape-plan, --no-escape-plan",
                help: "plan two-end escape order on the placed board and add it to the report as escape_plan \
                       (default: off)",
            },
            Opt {
                flag: "--bus-layers, --no-bus-layers",
                help: "give nets outside the planar subset further layers as largest non-crossing subsets; \
                       implies the two-end escape plan (default: off)",
            },
            Opt {
                flag: "--array-flow, --no-array-flow",
                help: "plan ball-array escapes and fan out only the plan's via balls (default: off)",
            },
            Opt {
                flag: "--via-lattice, --no-via-lattice",
                help: "reserve package via-site lattices for ball arrays before anything else is routed \
                       (default: off)",
            },
            Opt {
                flag: "--package-initial-pair-states N",
                help: "cap on each coupled pair's search states in the critical stage; 0 means no cap and \
                       --unified-set critical_pair_state_limit takes precedence (default: 6000)",
            },
            // Global routing.
            Opt { flag: "--tile-size-mm MM", help: "global routing tile size (default: 5)" },
            Opt { flag: "--router-iterations N", help: "global router iterations (default: 5)" },
            Opt { flag: "--pin-access-step-mm MM", help: "global router pin-access sampling step (default: 0.5)" },
            // Unified router.
            Opt {
                flag: "--unified-pitch-mm MM",
                help: "base lattice pitch of the unified router; 0 uses the default (default: 0.25)",
            },
            Opt {
                flag: "--unified-time-budget-s S",
                help: "wall-time bound in seconds on unified negotiated routing, from the unified stage on \
                       (default: no limit)",
            },
            Opt {
                flag: "--unified-save-inputs PATH",
                help: "save the unified router's replay inputs to this state file",
            },
            Opt {
                flag: "--unified-set KEY=VALUE",
                help: "override one unified router option (a JSON value, else text); may repeat; see \
                       --list-unified-options for the keys",
            },
            Opt {
                flag: "--list-unified-options",
                help: "list every --unified-set key with its type and default, then exit",
            },
            // Planes.
            Opt {
                flag: "--early-plane-stitch, --no-early-plane-stitch",
                help: "reserve declared plane contacts in package preflight before ordinary routing when the \
                       board declares copper zones (default: on)",
            },
            Opt {
                flag: "--early-plane-pad REF.PAD",
                help: "reserve early plane contacts only for this zone-net pad; may repeat; requires a declared \
                       copper zone",
            },
            Opt {
                flag: "--stitch-surface-zones, --no-stitch-surface-zones",
                help: "consider authored surface-zone contacts when stitching planes; the opt-out is diagnostic \
                       (default: on)",
            },
            Opt {
                flag: "--prefer-local-ground, --no-prefer-local-ground",
                help: "prefer a nearby via over reusing an existing contact for every GND pad (default: on)",
            },
            Opt {
                flag: "--prefer-local-ground-pad REF.PAD",
                help: "add this GND pad to the local-contact preference; may repeat",
            },
            Opt {
                flag: "--ground-via-in-pad",
                help: "permit filled-and-capped GND vias in pads; requires a declared copper zone and \
                       --fab-profile jlcpcb-six-layer",
            },
            Opt {
                flag: "--plane-stitch-step-mm MM",
                help: "sampling step of plane stitching-via sites (default: 0.5)",
            },
            Opt {
                flag: "--plane-stitch-radius-mm MM",
                help: "largest distance from a pad to its plane stitching via (default: 3)",
            },
            Opt {
                flag: "--plane-maze-step-mm MM",
                help: "mesh step of the local surface-path fallback for plane contacts (default: 0.1)",
            },
            Opt {
                flag: "--plane-maze-budget N",
                help: "search-state bound of the local surface-path fallback for plane contacts (default: 12000)",
            },
            Opt {
                flag: "--plane-contact-radius-mm MM",
                help: "radius within which a blocked land may reach the plane through an escaped same-net land; \
                       0 disables it (default: 0)",
            },
            Opt {
                flag: "--plane-stitch-detour-mm MM",
                help: "largest offset of three-leg detours tried for plane-contact surface paths; 0 tries none \
                       (default: 0)",
            },
            Opt {
                flag: "--verify-plane-fill {native,none,KICAD_CLI}",
                help: "check zone fills and connectivity of the written board: `native` refills and checks \
                       in-process (default), a kicad-cli path runs KiCad's refill and DRC, `none` skips the check \
                       (the run then cannot pass while zone nets are deferred)",
            },
            // Filled-copper feedback.
            Opt {
                flag: "--filled-copper-trials N",
                help: "native-driven filled-copper repair trials; 0 runs verification only (default: 8)",
            },
            Opt {
                flag: "--native-evidence-directory DIR",
                help: "new directory for native verification evidence (default: a fresh directory beside the \
                       report)",
            },
            // Outputs and checkpoints.
            Opt {
                flag: "-o, --output OUTPUT",
                help: "write the inspection-only KiCad PCB draft and its .editor-intent.json sidecar",
            },
            Opt {
                flag: "--report REPORT",
                help: "route report JSON, with a .summary.json beside it (default: BOARD.route-report.json)",
            },
            Opt { flag: "--require-verified-intent", help: "also fail unless every hard source requirement passes" },
            Opt { flag: "--progress", help: "print flushed phase, group and trial progress events" },
            Opt {
                flag: "--stop-after {placement}",
                help: "stop after this stage and write a partial report and draft; such a run never passes",
            },
            Opt {
                flag: "--checkpoint-dir DIR",
                help: "save placement.state.json and the critical-input and final-board diagnostic states in DIR",
            },
            Opt {
                flag: "--reuse-placement PATH",
                help: "start from a saved placement checkpoint; refused when the board or options differ",
            },
        ],
        actions: &[],
    },
    Command {
        name: "build",
        group: Group::Layout,
        summary: "route a board, refill and check it, and write run.json binding the outputs by SHA-256",
        options: &[
            Opt {
                flag: "--project-root DIR",
                help: "the project: SOURCE must be inside it and the output below its build/ (default: the current \
                       directory)",
            },
            Opt {
                flag: "--output-root DIR",
                help: "parent of the run directories (default: build/SOURCE-STEM below the project)",
            },
            Opt {
                flag: "--output-dir DIR",
                help: "the run directory itself, which must be new or empty and below the project's build/ \
                       (default: OUTPUT_ROOT/UTC-TIME-PID)",
            },
            Opt {
                flag: "--verify {native,kicad}",
                help: "refill and check the routed board with CopperScript's own filler and DRC, which needs no \
                       KiCad, or with kicad-cli (default: native)",
            },
            Opt {
                flag: "--kicad-cli PATH",
                help: "kicad-cli for --verify kicad (default: $KICAD_CLI, then PATH, then the standard Windows KiCad \
                       10 install); with --verify native it is only reported as an available cross-check",
            },
            Opt {
                flag: "--kicad-footprints DIR",
                help: "KiCad footprint libraries, passed to route-board as a footprint root. With --verify kicad the                        default is share/kicad/footprints beside kicad-cli, else /usr/share/kicad/footprints. A                        native build passes a root only when this is given, so it never depends on an installed                        KiCad; route-board's own --footprint-root also works after --",
            },
            Opt {
                flag: "--locked",
                help: "require copper.lock to match every package byte (for check and route-board)",
            },
            Opt {
                flag: "--offline",
                help: "reject remote package cache misses while resolving the board (routing itself always runs \
                       offline)",
            },
            Opt {
                flag: "--dry-run",
                help: "print the step plan, check the source offline and exit without writing anything",
            },
        ],
        actions: &[],
    },
    Command {
        name: "plan-layout",
        group: Group::Layout,
        summary: "produce a legal placement candidate and coarse routability report",
        options: &[
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "KiCad PCB of the planned placement (default: the board's generated PCB file name)",
            },
            Opt { flag: "--report REPORT", help: "write the layout readiness report JSON and its summary" },
            Opt { flag: "--no-check", help: "plan even when ERC reports errors" },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt { flag: "--candidates CANDIDATES", help: "placement candidates to generate (default: 3)" },
            Opt {
                flag: "--placement-grid-mm PLACEMENT_GRID_MM",
                help: "positive placement sampling step in mm (default: 1)",
            },
            Opt {
                flag: "--escape-margin-mm ESCAPE_MARGIN_MM",
                help: "margin in mm of the soft escape-corridor estimate, not a DRC clearance (default: 0.5)",
            },
            Opt {
                flag: "--escape-transit-lanes ESCAPE_TRANSIT_LANES",
                help: "transit lanes reserved by the soft escape-corridor estimate (default: 1)",
            },
            Opt {
                flag: "--power-domain-weight POWER_DOMAIN_WEIGHT",
                help: "soft supply-domain attraction weight; 0 disables it (default: 0.25)",
            },
            Opt {
                flag: "--support-pattern-trials SUPPORT_PATTERN_TRIALS",
                help: "joint support-pattern combination checks per placement seed; nonzero is not implemented \
                       yet (default: 0)",
            },
            Opt {
                flag: "--package-support-trials PACKAGE_SUPPORT_TRIALS",
                help: "package-support placement trials; nonzero is not implemented yet (default: 0)",
            },
            Opt {
                flag: "--decoupling-placement-trials DECOUPLING_PLACEMENT_TRIALS",
                help: "copper-access pose trials per decoupling capacitor; 0 disables the relocation heuristic \
                       (default: 64)",
            },
            Opt {
                flag: "--placement-seeds PLACEMENT_SEEDS",
                help: "positive placement seed budget (default: max(2, candidates))",
            },
            Opt {
                flag: "--placement-refinement-passes PLACEMENT_REFINEMENT_PASSES",
                help: "optional placement refinement passes; 0 keeps the legalized poses (default: 2)",
            },
            Opt {
                flag: "--escape-spacing-passes ESCAPE_SPACING_PASSES",
                help: "optional escape-spacing improvement passes (default: 2)",
            },
            Opt {
                flag: "--width-mm WIDTH_MM",
                help: "board rectangle width, used only when the source has no mechanical block (default: 100.0)",
            },
            Opt {
                flag: "--height-mm HEIGHT_MM",
                help: "board rectangle height, used only when the source has no mechanical block (default: 80.0)",
            },
            Opt { flag: "--hard-macro HARD_MACRO", help: "apply a legacy hard-macro scene JSON; may repeat" },
            Opt {
                flag: "--placement-templates PLACEMENT_TEMPLATES",
                help: "apply a placement-template JSON scene (rigid placement clusters)",
            },
        ],
        actions: &[],
    },
    Command {
        name: "route-global",
        group: Group::Layout,
        summary: "produce multilayer routing guides with negotiated congestion",
        options: &[
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt {
                flag: "-o, --output OUTPUT",
                help: "routing-guide JSON (default: BOARD.global-route.json, named after the board)",
            },
            Opt {
                flag: "--pcb-output PCB_OUTPUT",
                help: "also write a KiCad PCB of the placed board with its hard macros materialized",
            },
            Opt { flag: "--no-check", help: "route even when ERC reports errors" },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt { flag: "--candidates CANDIDATES", help: "placement candidates to generate (default: 3)" },
            Opt {
                flag: "--tile-size-mm TILE_SIZE_MM",
                help: "global routing tile size in mm, also the initial placement-feedback move (default: 5)",
            },
            Opt { flag: "--router-iterations ROUTER_ITERATIONS", help: "global router iterations (default: 20)" },
            Opt {
                flag: "--feedback-iterations FEEDBACK_ITERATIONS",
                help: "placement-routing feedback iterations (default: 4)",
            },
            Opt {
                flag: "--feedback-trials FEEDBACK_TRIALS",
                help: "placement trials per feedback iteration (default: 24)",
            },
            Opt {
                flag: "--placement-grid-mm PLACEMENT_GRID_MM",
                help: "positive placement sampling step in mm (default: 1)",
            },
            Opt {
                flag: "--escape-margin-mm ESCAPE_MARGIN_MM",
                help: "margin in mm of the soft escape-corridor estimate, not a DRC clearance (default: 0.5)",
            },
            Opt {
                flag: "--escape-transit-lanes ESCAPE_TRANSIT_LANES",
                help: "transit lanes reserved by the soft escape-corridor estimate (default: 1)",
            },
            Opt {
                flag: "--power-domain-weight POWER_DOMAIN_WEIGHT",
                help: "soft supply-domain attraction weight; 0 disables it (default: 0.25)",
            },
            Opt {
                flag: "--support-pattern-trials SUPPORT_PATTERN_TRIALS",
                help: "joint support-pattern combination checks per placement seed; nonzero is not implemented \
                       yet (default: 0)",
            },
            Opt {
                flag: "--package-support-trials PACKAGE_SUPPORT_TRIALS",
                help: "package-support placement trials; nonzero is not implemented yet (default: 0)",
            },
            Opt {
                flag: "--decoupling-placement-trials DECOUPLING_PLACEMENT_TRIALS",
                help: "copper-access pose trials per decoupling capacitor; 0 disables the relocation heuristic \
                       (default: 64)",
            },
            Opt {
                flag: "--placement-seeds PLACEMENT_SEEDS",
                help: "positive placement seed budget (default: max(2, candidates))",
            },
            Opt {
                flag: "--placement-refinement-passes PLACEMENT_REFINEMENT_PASSES",
                help: "optional placement refinement passes; 0 keeps the legalized poses (default: 2)",
            },
            Opt {
                flag: "--escape-spacing-passes ESCAPE_SPACING_PASSES",
                help: "optional escape-spacing improvement passes (default: 2)",
            },
            Opt {
                flag: "--width-mm WIDTH_MM",
                help: "board rectangle width, used only when the source has no mechanical block (default: 100.0)",
            },
            Opt {
                flag: "--height-mm HEIGHT_MM",
                help: "board rectangle height, used only when the source has no mechanical block (default: 80.0)",
            },
            Opt { flag: "--hard-macro HARD_MACRO", help: "apply a legacy hard-macro scene JSON; may repeat" },
            Opt {
                flag: "--placement-templates PLACEMENT_TEMPLATES",
                help: "apply a placement-template JSON scene (rigid placement clusters)",
            },
            Opt {
                flag: "--pin-access-step-mm PIN_ACCESS_STEP_MM",
                help: "pin-access sampling step in mm (default: 0.5)",
            },
            Opt {
                flag: "--critical-pin-access-step-mm CRITICAL_PIN_ACCESS_STEP_MM",
                help: "finer pin-access step in mm retried for critical-net pins that found no access",
            },
        ],
        actions: &[],
    },
    Command {
        name: "edit-mechanical",
        group: Group::Layout,
        summary: "open the physical, mechanical and definition editor for a board",
        options: EDIT_MECHANICAL,
        actions: &[],
    },
    Command {
        name: "edit",
        group: Group::Layout,
        summary: "alias of edit-mechanical",
        options: EDIT_MECHANICAL,
        actions: &[],
    },
    Command {
        name: "editor-overlay",
        group: Group::Layout,
        summary: "export a routed editor reference from a generic build run",
        options: &[
            Opt { flag: "--kicad-python KICAD_PYTHON", help: "explicit Python interpreter with pcbnew (required)" },
            Opt { flag: "-o, --output OUTPUT", help: "new overlay JSON; never overwritten (required)" },
        ],
        actions: &[],
    },
    Command {
        name: "synthesize-antenna",
        group: Group::Layout,
        summary: "tune a constraint antenna in a full-wave (FDTD) model of the board",
        options: &[
            Opt {
                flag: "--antenna NAME",
                help: "the constraint antenna to tune; required when the board declares more than one",
            },
            Opt { flag: "--budget N", help: "coarse full-wave evaluations the search may spend (default: 12)" },
            Opt {
                flag: "--pixel-cell MM",
                help: "pixel cell size for topology = pixel (default: the larger of the track width and 1 mm)",
            },
            Opt { flag: "--no-fine", help: "skip the fine-mesh verification (and its correction) after the search" },
            Opt { flag: "--verify", help: "evaluate the declared geometry instead of searching" },
            Opt { flag: "--threads N", help: "solver threads; 0 picks one per physical core (default: 0)" },
            Opt { flag: "--report PATH", help: "write the synthesis report" },
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
        ],
        actions: &[],
    },
    Command {
        name: "si-check",
        group: Group::Layout,
        summary: "screen impedance, reference planes, layer groups and length-match groups before routing",
        options: &[
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
            Opt { flag: "--json", help: "emit deterministic JSON (schema copperscript-si-check/v0.1)" },
        ],
        actions: &[],
    },
    Command {
        name: "emi-check",
        group: Group::Layout,
        summary: "screen switching cells for noise sources: hot-loop area and inductance, switch-node copper, proximity (screening evidence)",
        options: &[
            Opt { flag: "--locked", help: "require copper.lock to match every package byte" },
            Opt { flag: "--offline", help: "reject remote package cache misses" },
            Opt {
                flag: "--layers {2,4,6}",
                help: "copper-layer count; default derives from source requirements; must match a declared stackup",
            },
            Opt {
                flag: "--fab-profile {generic,jlcpcb-four-layer,jlcpcb-six-layer}",
                help: "fabrication-rule profile (default: generic)",
            },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "extra KiCad footprint root for footprint identifiers the source does not bind; may repeat",
            },
            Opt {
                flag: "--allow-proxy-footprints",
                help: "use generated inspection-only proxy footprints instead of resolving real ones",
            },
            Opt {
                flag: "--run RUN",
                help: "analyse the routed board of a route-board run: a final-board.state.json file, or a directory holding it (also in checkpoint/)",
            },
            Opt {
                flag: "--write",
                help: "with --run: also store the result as the emi section of the run's route-report.json (written atomically,                        report_sha256 of the summary beside it updated); `copper issues RUN --stage emi` then lists it",
            },
            Opt {
                flag: "--plot-spectrum PLOT_SPECTRUM",
                help: "write the conducted-emission spectrum as a self-contained SVG: one panel per cell with an EMI-CONDUCTED finding, harmonic stems 150 kHz to 30 MHz, quasi-peak and average limit lines when an emission standard is declared and the levels are detector readings (a panel whose isolated-line assumption fails, or a declared switching-frequency range, shows line amplitudes only, no limit); the common-mode score is a ranking and is never plotted; a screening estimate",
            },
            Opt {
                flag: "--plot-field PLOT_FIELD",
                help: "write the quasi-static magnetic near-field map of each hot loop (|H| in dB(uA/m) in a plane above the board, with the level at sensitive pads and connectors) as an SVG; the hot-loop current path only, the inductor's own field is not included; a screening estimate",
            },
            Opt {
                flag: "--field-height FIELD_HEIGHT",
                help: "with --plot-field: height of the map plane above the top copper, e.g. 1mm (default: 1mm)",
            },
            Opt {
                flag: "--fullwave",
                help: "add, per buck cell, the hot-loop inductance from a region full-wave (FDTD) model of the loop on the pinned ems solver (measures.fullwave, an EMI-FULLWAVE:<cell> coverage entry); two meshes when the budget allows, giving mesh_difference_pct and mesh_sensitivity_pct; slow (minutes per cell), never part of the default run; screening evidence",
            },
            Opt {
                flag: "--fullwave-fidelity {coarse,fine}",
                help: "with --fullwave: mesh density (default: coarse, with the loop refined to 0.2-0.25 mm; fine halves the general copper cell: about twice the work of the standard mesh on the placed example, up to eight times where the narrow features set the cell)",
            },
            Opt {
                flag: "--fullwave-budget FULLWAVE_BUDGET",
                help: "with --fullwave: refuse a model whose estimated run time exceeds this many seconds (default: 1800); the check runs before the solver starts, and a run that takes half as long again as the budget is stopped (budget_exceeded, with its partial statistics)",
            },
            Opt {
                flag: "--fullwave-single-mesh",
                help: "with --fullwave: run the standard mesh only (no second mesh, so no mesh_difference_pct; flag single_mesh)",
            },
            Opt {
                flag: "--fullwave-margin FULLWAVE_MARGIN",
                help: "with --fullwave: window margin around the loop in millimetres (default: 5, at least ten dielectric heights)",
            },
            Opt {
                flag: "--extract",
                help: "add, per buck cell, the hot-loop interconnect impedance (L and R at 1, 10 and 100 MHz) from a quasi-static partial-element (PEEC) extraction of the real filled copper: zone fills made natively, tracks, pads and vias, the stack-up's heights, the capacitor and diode as copper bars between their pads and the switch as the port (measures.extract with numerical_sensitivity_pct from two meshes, parameter_assumptions and not_modelled, an EMI-EXTRACT:<cell> coverage entry), beside the closed-form and full-wave values; seconds to minutes, never part of the default run; screening evidence",
            },
            Opt {
                flag: "--extract-budget FILAMENTS",
                help: "with --extract: the most filaments of the dense inductance matrix (default: 2400, which is some three seconds per frequency in an optimised build); a larger mesh is coarsened by 1.3 until it fits (flag coarsened, status indicative)",
            },
            Opt {
                flag: "--extract-margin EXTRACT_MARGIN",
                help: "with --extract: window margin around the loop in millimetres (default: 3); copper and planes end at the window edge",
            },
            Opt {
                flag: "--extract-cell EXTRACT_CELL",
                help: "with --extract: the fine mesh pitch in millimetres (default: 0.2; the coarse pitch is five times it)",
            },
            Opt { flag: "--extract-single-mesh", help: "with --extract: one mesh only (no numerical sensitivity)" },
            Opt {
                flag: "--extract-no-switch-bar",
                help: "with --extract: put the port between the facing sides of the switch pads instead of in a gap in a copper bar across the switch (the gap between the pads is then not in the extracted path)",
            },
            Opt { flag: "--json", help: "emit deterministic JSON (schema copperscript-emi-check/v0.1)" },
        ],
        actions: &[],
    },
    Command {
        name: "bench",
        group: Group::Benchmark,
        summary: "benchmark ladder: route a fixed set of boards and compare completion and time",
        options: &[
            Opt {
                flag: "--out OUT",
                help: "results directory: one subdirectory per board plus results.json and results.md (required)",
            },
            Opt { flag: "--only ONLY", help: "comma-separated board names to run; an unknown name is an error" },
            Opt { flag: "--tags TAGS", help: "comma-separated tags; run only the boards carrying any of them" },
            Opt { flag: "--label LABEL", help: "label recorded in results.json (default: the --out directory name)" },
            Opt {
                flag: "--baseline BASELINE",
                help: "earlier results.json to compare with; lost completion, more DRC errors, native opens, \
                       unrouted critical groups, skew or length-match violations, or a slowdown \
                       beyond max(1.25x, +30 s) exits 1",
            },
            Opt {
                flag: "--kicad-cli KICAD_CLI",
                help: "passed as --verify-plane-fill to the boards the ladder marks native; `native` selects the \
                       native checker",
            },
            Opt {
                flag: "--extra-arg EXTRA_ARGS",
                help: "extra route-board argument for every board; may repeat (write --extra-arg=VALUE for a value \
                       starting with -)",
            },
            Opt {
                flag: "--placements PLACEMENTS",
                help: "per-board placement checkpoints: reuse BOARD/placement.state.json when present, else save \
                       it there",
            },
            Opt { flag: "--jobs JOBS", help: "boards routed at once in parallel processes (default: 1)" },
        ],
        actions: &[],
    },
    Command {
        name: "benchmark-routing",
        group: Group::Benchmark,
        summary: "run bounded routing acceptance cases and measure final copper",
        options: &[
            Opt {
                flag: "-o, --output OUTPUT",
                help: "new directory for acceptance.json and one subdirectory per case (required)",
            },
            Opt {
                flag: "--kicad-cli KICAD_CLI",
                help: "verify each case's filled planes with this kicad-cli; `native` selects the native checker",
            },
        ],
        actions: &[],
    },
];
