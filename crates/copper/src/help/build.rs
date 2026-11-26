//! Help for the start-here commands and the commands that write and check source.

use super::{Action, Command, Group, Opt};

pub const COMMANDS: &[Command] = &[
    Command {
        name: "agent-guide",
        group: Group::Start,
        summary: "read compact, versioned agent guidance; start here",
        options: &[Opt { flag: "--json", help: "print the topic (default: start) as JSON instead of text" }],
        actions: &[],
    },
    Command {
        name: "explain",
        group: Group::Start,
        summary: "explain a diagnostic code and its usual fix",
        options: &[
            Opt { flag: "--list", help: "list every diagnostic code and the compiler code families" },
            Opt { flag: "--json", help: "print the explanation or the code list as JSON" },
        ],
        actions: &[],
    },
    Command {
        name: "init",
        group: Group::Start,
        summary: "create a new project: copper.mod, board.copper and .gitignore",
        options: &[
            Opt { flag: "--name NAME", help: "board name in the generated source (default: the directory name)" },
            Opt { flag: "--force", help: "overwrite existing files" },
        ],
        actions: &[],
    },
    Command {
        name: "completions",
        group: Group::Start,
        summary: "print a shell completion script (bash, zsh or fish)",
        options: &[],
        actions: &[],
    },
    Command {
        name: "check",
        group: Group::Source,
        summary: "run electrical rules checks",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt {
                flag: "--json",
                help: "emit a copperscript.check/v1 document with located findings, hints and machine-applicable fixes",
            },
            Opt {
                flag: "--pins {warn,error,off}",
                help: "report UNMARKED_PIN as a warning, as an error, or not at all (default: warn)",
            },
            Opt {
                flag: "--macro-inputs FILE",
                help: "prepare the board's physical macro assets (fetching pinned dependencies unless --offline) \
                       and write every macro input path to FILE, one per line",
            },
            Opt {
                flag: "--sim-results FILE",
                help: "also check the pin voltages a copper sim run recorded in FILE (sim-results.json) against the \
                       pins' absolute ratings (SIM_PIN_ABSOLUTE_HIGH / SIM_PIN_ABSOLUTE_LOW) and the simulated \
                       components against their parts' rated_power, rated_voltage and pins' current_max \
                       (SIM_STRESS_*), naming the bench, step and value; results made from other board, bench or local SPICE model \
                       sources are refused (SIM_RESULTS_STALE); check never runs a simulation",
            },
        ],
        actions: &[],
    },
    Command {
        name: "compile",
        group: Group::Source,
        summary: "compile source to JSON IR",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt { flag: "-o, --output OUTPUT", help: "write the JSON IR to OUTPUT (default: standard output)" },
            Opt { flag: "--no-check", help: "skip ERC; by default compilation stops when ERC reports errors" },
        ],
        actions: &[],
    },
    Command {
        name: "query",
        group: Group::Source,
        summary: "answer compact questions about the compiled design (pins, nets, components, open pins)",
        options: &[
            Opt { flag: "--json", help: "print the result as JSON instead of text" },
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
        ],
        actions: &[],
    },
    Command {
        name: "power-check",
        group: Group::Source,
        summary: "analyze explicit steady-state power scenarios",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt {
                flag: "--sim-results FILE",
                help: "also list each board supply's simulated current from a copper sim run's sim-results.json \
                       (per bench, step and bench supply, with its source), the rail's totals and the loads in no \
                       bench, and fail a rail whose mean or peak exceeds its budget (the supply's current_max / \
                       current_peak_max, its source pin's current_max, its source part's output_current_limit: \
                       SIM_RAIL_MEAN_HIGH / SIM_RAIL_PEAK_HIGH); results made from other board, bench or model sources \
                       are refused (SIM_RESULTS_STALE); power-check never runs a simulation",
            },
        ],
        actions: &[],
    },
    Command {
        name: "sim",
        group: Group::Source,
        summary: "run the board's simulation benches and check their measures against limits",
        options: &[
            Opt {
                flag: "--bench BENCH",
                help: "run only the bench of this name (default: every bench in benches/); a bench with several analyses runs them all, BENCH.SECTION one of them",
            },
            Opt {
                flag: "--out OUT",
                help: "directory for sim-results.json (schema copperscript-sim/v1, docs/sim-results-schema.md), BENCH.csv and the interactive BENCH.html (default: build/sim)",
            },
            Opt {
                flag: "--jobs N",
                help: "run a bench's steps, Monte Carlo runs and worst-case corners on up to N threads (default: one per core); the results do not depend on N",
            },
            Opt {
                flag: "--models {default,macro,vendor}",
                help: "for parts that declare a built-in block and a sim spice block for one unit: default runs the built-in macro model, vendor the sim spice model (its file must be cached), macro built-in models only (a part with only a sim spice model is SIM107)",
            },
            Opt {
                flag: "--layout RUN",
                help: "the routed board for the benches that say use layout; (RUN/final-board.state.json, as route-board --checkpoint-dir writes it): they run on the schematic alone and with the board's extracted capacitance, track resistance and inductance; without it they are not run (SIM118), a board of another design or not fully routed is refused (SIM119)",
            },
            Opt {
                flag: "--check-model PART",
                help: "instead of the benches, run the standard bench set on PART of the board's library (by qualified or plain name): its macro model against the values its block declares, beside its vendor model; exit 1 when a figure is out of tolerance or a model does not load or converge",
            },
            Opt {
                flag: "--supply VOLTS",
                help: "with --check-model: the supply span of an amplifier (default 10 V, as +/-5 V) or the input voltage of a regulator or reference (default |output_voltage| + dropout + 1 V)",
            },
            Opt {
                flag: "--tie PIN=ROLE",
                help: "with --check-model: tie a vendor subcircuit's pin that the macro block leaves unmodelled to a role's node (vp, vn, out, in, gnd, ...); repeatable or comma-separated; other such pins are left open",
            },
            Opt {
                flag: "--no-tran",
                help: "with --check-model: leave out the transient figures (slew rate, start-up)",
            },
            Opt {
                flag: "--tran-budget SECONDS",
                help: "with --check-model: the wall time each transient run may take (default 120); a run that takes longer is reported as not finished and its figure is not judged",
            },
            Opt {
                flag: "--timing",
                help: "print each bench's wall time, solver work and time per phase (also COPPER_SIM_TIMING=1); adds timing to the JSON",
            },
            Opt {
                flag: "--watch",
                help: "after the run, keep watching the board's directory (sources, parts, benches, local model files) and the vendor model cache, and rerun on a change: a changed bench file reruns its benches, anything else every bench; stop with Ctrl-C",
            },
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
        ],
        actions: &[],
    },
    Command {
        name: "lock",
        group: Group::Source,
        summary: "resolve package content and update copper.lock",
        options: &[
            Opt {
                flag: "--locked",
                help: "prepare and verify the selected assets against the existing copper.lock without rewriting it",
            },
            Opt { flag: "--offline", help: "require cached inputs; never fetch" },
        ],
        actions: &[],
    },
    Command {
        name: "audit-footprints",
        group: Group::Source,
        summary: "resolve and validate all selected board footprints",
        options: &[
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
            Opt {
                flag: "--footprint-root FOOTPRINT_ROOT",
                help: "also search this directory for footprints; repeatable",
            },
            Opt { flag: "--strict", help: "treat footprint import warnings as errors" },
            Opt { flag: "--json", help: "print a copperscript-footprint-audit/v0.1 document" },
        ],
        actions: &[],
    },
    Command {
        name: "check-footprint",
        group: Group::Source,
        summary: "validate and inspect a KiCad .kicad_mod footprint",
        options: &[Opt { flag: "--strict", help: "treat import warnings (such as presentation-only items) as errors" }],
        actions: &[],
    },
    Command {
        name: "compile-macro",
        group: Group::Source,
        summary: "lower a Copper macro to a verified JSON asset",
        options: &[
            Opt {
                flag: "-o, --output OUTPUT",
                help: "write the macro asset to OUTPUT (required; must differ from the source and its inputs)",
            },
            Opt {
                flag: "--locked",
                help: "require every dependency to be pinned in copper.lock and verify it without rewriting the lock",
            },
            Opt { flag: "--offline", help: "resolve dependencies from the local cache only; never fetch" },
        ],
        actions: &[],
    },
    Command {
        name: "lsp",
        group: Group::Source,
        summary: "run the language server over stdio (locked, offline)",
        options: &[],
        actions: &[],
    },
    Command {
        name: "library",
        group: Group::Source,
        summary: "compact discovery and inspection across declared part libraries",
        options: &[],
        actions: &[
            Action {
                name: "index",
                summary: "write a copperscript.library-index/v1 snapshot of a standalone library ROOT",
                options: &[Opt {
                    flag: "-o, --output OUTPUT",
                    help: "write the index to OUTPUT, outside the indexed library root (required)",
                }],
            },
            Action {
                name: "search",
                summary: "search declarations across the project's library providers",
                options: &[
                    Opt {
                        flag: "--project PROJECT",
                        help: "the project whose copper.mod declares the libraries (default: the current directory)",
                    },
                    Opt { flag: "--limit LIMIT", help: "most records per page, 1..100 (default: 5)" },
                    Opt { flag: "--offset OFFSET", help: "records to skip, to fetch the next page (default: 0)" },
                    Opt {
                        flag: "--max-chars MAX_CHARS",
                        help: "bound on the compact JSON output, 2048..65536 (default: 8000)",
                    },
                    Opt { flag: "--provider MODULE", help: "search only this declared provider; repeatable" },
                    Opt {
                        flag: "--kind {part,device,module,board_profile,macro}",
                        help: "only definitions of this kind",
                    },
                    Opt { flag: "--fetch", help: "permit fetching uncached remote content (default: offline)" },
                    Opt { flag: "--update-lock", help: "create or update copper.lock (default: locked)" },
                    Opt { flag: "--json", help: "accepted; the output is compact JSON either way" },
                ],
            },
            Action {
                name: "inspect",
                summary: "show one section of a selected definition (PACKAGE::SYMBOL)",
                options: &[
                    Opt {
                        flag: "--project PROJECT",
                        help: "the project whose copper.mod declares the libraries (default: the current directory)",
                    },
                    Opt { flag: "--limit LIMIT", help: "most records per page, 1..100 (default: 20)" },
                    Opt { flag: "--offset OFFSET", help: "records to skip, to fetch the next page (default: 0)" },
                    Opt {
                        flag: "--max-chars MAX_CHARS",
                        help: "bound on the compact JSON output, 2048..65536 (default: 8000)",
                    },
                    Opt {
                        flag: "--kind {part,device,module,board_profile,macro}",
                        help: "select the definition of this kind when names collide",
                    },
                    Opt {
                        flag: "--section {overview,interfaces,requirements,composition,properties,notes,dependencies,declarations,source}",
                        help: "the section to show (default: overview)",
                    },
                    Opt {
                        flag: "--expect-digest DIGEST",
                        help: "fail unless the module content digest is DIGEST (sha256:..., from a search result)",
                    },
                    Opt {
                        flag: "--member NAME",
                        help: "select exact declaration names, component refs, constraint kinds or targets within \
                               the section; repeatable",
                    },
                    Opt { flag: "--fetch", help: "permit fetching uncached remote content (default: offline)" },
                    Opt { flag: "--update-lock", help: "create or update copper.lock (default: locked)" },
                    Opt { flag: "--json", help: "accepted; the output is compact JSON either way" },
                ],
            },
            Action {
                name: "diagnostics",
                summary: "list the project's library discovery diagnostics",
                options: &[
                    Opt {
                        flag: "--project PROJECT",
                        help: "the project whose copper.mod declares the libraries (default: the current directory)",
                    },
                    Opt { flag: "--limit LIMIT", help: "most records per page, 1..100 (default: 20)" },
                    Opt { flag: "--offset OFFSET", help: "records to skip, to fetch the next page (default: 0)" },
                    Opt {
                        flag: "--max-chars MAX_CHARS",
                        help: "bound on the compact JSON output, 2048..65536 (default: 8000)",
                    },
                    Opt { flag: "--fetch", help: "permit fetching uncached remote content (default: offline)" },
                    Opt { flag: "--update-lock", help: "create or update copper.lock (default: locked)" },
                    Opt { flag: "--json", help: "accepted; the output is compact JSON either way" },
                ],
            },
        ],
    },
];
