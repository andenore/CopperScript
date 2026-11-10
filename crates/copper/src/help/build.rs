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
