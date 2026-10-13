"""Bundled, versioned agent bootstrap; available without a repository or network."""
from __future__ import annotations

import hashlib
from importlib.metadata import PackageNotFoundError, version
import json

SCHEMA = "copperscript.agent-guide/v1"
GUIDE_VERSION = 1

# The example is electrical-only and uses builtins, so it can be compiled offline.
SYNTAX_EXAMPLE = """board Divider {
    use library "tiny";
    component R1: RESISTOR { value = 10kohm; }
    component R2: RESISTOR { value = 10kohm; }
    net VIN { R1.1; }
    net MID { R1.2; R2.1; }
    net GND { R2.2; }
    supply VIN { voltage = 3.3V; external = true; }
    supply GND { voltage = 0V; external = true; }
}"""

_TOPICS = {
    "start": {
        "title": "Start here",
        "guidance": [
            "Read the project's specification, agent instructions, README and build targets. Preserve its board configuration, dependency pins, macros and evidence.",
            "Use the active compiler's guide and command --help. Get only the topic needed; the guide is bundled with this CLI and does not inspect or modify the project.",
            "Find reusable definitions in the project's declared dependencies. CopperLib is one library provider; other repositories use the same interface.",
            "Inspect selected interfaces, requirements, source notes and dependencies. Follow omissions and inherited/imported contracts before integrating a definition.",
            "Compose board-specific intent in .copper source. Keep reusable definitions in library repositories and algorithms in CopperScript.",
            "Run electrical checks, then the project's standard build/review workflow. Read compact routing summaries; retrieve detailed evidence for blockers. Routing closure, verified intent and hardware qualification are separate results.",
        ],
        "commands": [
            ["copper", "library", "search", "QUERY", "--project", ".", "--limit", "3"],
            ["copper", "library", "inspect", "PACKAGE::SYMBOL", "--project", ".", "--section", "requirements"],
            ["copper", "check", "board.copper", "--locked", "--offline"],
            ["copper", "summarize", "build/route.json", "--issues", "5"],
        ],
    },
    "syntax": {
        "title": "Language essentials",
        "guidance": [
            "A .copper file declares one board, module, part, device, board_profile or macro. A board composes components/modules, nets, supplies, interfaces and physical constraints.",
            "Board silkscreen blocks declare text, logo and shapes. The Cu logo and compiler version are included by default; opt out with silkscreen { branding { enabled = false; } }. Review final artwork placement and native DRC.",
            "Artwork can opt into material = copper on its front/back side; exposed = true adds a matching mask opening. Branding layout = stacked puts compact name/version labels below the logo. Floating artwork reserves placement/routing space and has no electrical net. Copper glyph, filled-feature and exposed-mask qualification remain unsupported signoff checks.",
            'Import with: import sensor "PROVIDER/PACKAGE"; component U1: sensor.Part; module SUPPORT: sensor.Support;. Use the actual exported kind and names returned by library inspection.',
            "A net connects named endpoints: net VDD { U1.VDD; C1.1; }. Module ports become instance endpoints in the host. Inspect pin/port names and voltage/direction limits; do not infer them from footprints.",
            "Versions and replacements live in copper.mod; copper.lock binds consumed assets. Cross-library requirements must be declared in the host manifest. Source imports do not select versions.",
            'Import a physical macro with import layout "PROVIDER/assets/layout.json"; bind existing circuitry with physical_macro NAME: layout on MODULE { member ROLE = COMPONENT; net ROLE = port.PORT; }. Omit on for board-relative mappings. Asset hashes come from copper.lock; electrical checks leave geometry pending.',
            'Author exact physical geometry in macro NAME { ... }; import its package normally and bind physical_macro NAME: alias.Definition { ... }. Footprint imports name exact .kicad_mod assets. copper compile-macro SOURCE -o ASSET.json emits the compatible generated asset; source/footprint hashes are automatic and qualification remains separate.',
            "Source quantities attach units (100nF, 3.3V, 0.25mm). Display engineering values with spaces and exact fixed-point fractions. Physical intent uses constraint KIND(TARGETS) { PARAMETERS; }; choose source-backed limits.",
            'Pin-associated bypasses use role = decoupling; decouples = "U.PIN"; on the capacitor. Apply this to bypasses, not every capacitor.',
            "The following builtin resistor divider demonstrates electrical syntax only; it is not a layout or an orderable component selection.",
        ],
        "examples": [SYNTAX_EXAMPLE],
        "commands": [["copper", "check", "board.copper", "--locked", "--offline"],
                     ["copper", "compile", "board.copper", "--locked", "--offline", "-o", "build/board.json"]],
    },
    "libraries": {
        "title": "Selective library context",
        "guidance": [
            "Search resolves providers declared in the consuming project's copper.mod. All query terms must match; --kind and repeatable --provider narrow results. Same-name definitions retain separate provider/package identities.",
            "Select PACKAGE::SYMBOL from search, then pass its content_digest through --expect-digest when inspecting. Inspection reads the selected definition; inherited device and imported contracts need separate retrieval.",
            "Start with overview, then retrieve requirements, notes and dependencies. Other sections are interfaces, properties, composition, declarations and source. Properties carry available evidence locators; external qualification files still require direct inspection.",
            "Use repeatable --member NAME for exact declared names, component refs, constraint kinds or targets. Check unmatched_members and unselected; member selection does not remove host obligations.",
            "Follow next_offset for omitted records. Default search is five records, inspection 20, with an 8000-character compact JSON budget. --limit, --offset and --max-chars control retrieval. needs_source means a record was too large: read its bound source or increase the budget.",
            "Discovery is locked/offline by default. --update-lock explicitly permits lock changes; --fetch permits cache downloads. Source-derived metadata and index snapshots are descriptive, not reviewed or electrical pass certificates.",
        ],
        "commands": [
            ["copper", "library", "search", "QUERY", "--project", ".", "--kind", "part", "--limit", "3"],
            ["copper", "library", "inspect", "PACKAGE::SYMBOL", "--section", "interfaces", "--member", "PIN", "--expect-digest", "sha256:DIGEST"],
            ["copper", "library", "inspect", "PACKAGE::SYMBOL", "--section", "dependencies"],
            ["copper", "library", "diagnostics", "--project", "."],
        ],
    },
    "verification": {
        "title": "Build and interpret evidence",
        "guidance": [
            "Use the project's documented build/review target: it carries layer count, fabrication profile, footprints, placement intent, macros and native tool settings. Retain locked inputs. A generic command below needs those project options before use.",
            "Run check for ERC and audit-footprints for selected footprint geometry before routing. Use the shared route-board pipeline and a bounded routing profile for solver work. Save reports and candidates in a fresh build directory.",
            "route-board emits a compact summary and a source-bound requirement ledger. Default success means its routing gates passed; --require-verified-intent additionally requires hard declaration coverage. Inspect blocked, failed, assumed, external and not-run results explicitly.",
            "Native fill evidence must match the final exported board. Use the configured KiCad executable through --verify-plane-fill and retain the generated evidence. Explicit DRC and native-filled connectivity may differ for deferred zones; the summary reports reconciled observations without altering raw findings.",
            "Use summarize for retained route/layout reports and read full evidence only for unresolved issues. Measured length/via costs describe geometry; unsupported electrical verifiers remain blocked. Native DRC, ERC and Gerbers do not establish hardware functionality or production approval.",
        ],
        "commands": [
            ["copper", "audit-footprints", "board.copper", "--locked", "--offline"],
            ["copper", "route-board", "board.copper", "--locked", "--offline", "--routing-profile", "balanced",
             "--require-verified-intent", "--report", "build/NEW-RUN/route.json", "-o", "build/NEW-RUN/board.kicad_pcb"],
            ["copper", "summarize", "build/NEW-RUN/route.json", "--issues", "5"],
        ],
    },
    "recovery": {
        "title": "Recover with bounded context",
        "guidance": [
            "If this compiler lacks agent-guide or library commands, inspect its --help and the project's compiler pin. Select a current compiler explicitly for guidance/review using the project's documented workflow; retain existing build pins.",
            "On a missing lock/cache, inspect copper.mod and the resolver diagnostic. Use the documented dependency setup. Explicit --fetch or --update-lock are available for library discovery when the task calls for fetching or changing dependencies; do not treat a checksum mismatch as permission to update.",
            "On empty search, check query terms, provider and kind. Search covers declared dependencies. A missing dependency or unsupported host needs manifest/setup work; it is not proof that no suitable part exists.",
            "On stale --expect-digest, repeat selection against current resolved content. On duplicate exports, repair the library. On omissions or needs_source, retrieve the indicated page or exact source before treating an obligation as understood.",
            "On ERC, package-access, native or requirement failures, use the compact issue's source origin and detailed report. Preserve exact pins, limits, immutable macro copper and required via arrays. Reproduce solver failures in a small shared fixture; report unsupported checks as incomplete.",
        ],
        "commands": [["copper", "library", "diagnostics", "--project", "."],
                     ["copper", "route-board", "--help"],
                     ["copper", "summarize", "build/route.json", "--issues", "5"]],
    },
}
TOPICS = tuple(_TOPICS)


def get_guide(topic: str = "start") -> dict:
    if topic not in _TOPICS:
        raise ValueError(f"unknown guide topic: {topic}")
    # Round-trip produces independent JSON data for callers, preserving bundled text.
    content = json.loads(json.dumps(_TOPICS[topic]))
    digest = hashlib.sha256(json.dumps(content, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    try:
        package_version = version("copperscript")
    except PackageNotFoundError:
        package_version = "unavailable"
    return {"schema": SCHEMA, "guide_version": GUIDE_VERSION, "package_version": package_version,
            "topic": topic, "content_digest": "sha256:" + digest, "topics": list(TOPICS), **content}


def render_guide(guide: dict, *, as_json: bool = False) -> str:
    if as_json:
        return json.dumps(guide, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
    lines = [f"CopperScript agent guide v{guide['guide_version']} / {guide['topic']} (package {guide['package_version']})",
             "", guide["title"], ""]
    lines.extend(f"{i}. {item}" for i, item in enumerate(guide["guidance"], 1))
    lines.extend(["", "Command templates (replace QUERY, PACKAGE::SYMBOL and paths):"])
    lines.extend(" ".join(json.dumps(arg) if any(c.isspace() for c in arg) else arg for arg in command)
                 for command in guide["commands"])
    for example in guide.get("examples", []):
        lines.extend(["", example])
    lines.extend(["", "More: copper agent-guide TOPIC; topics: " + ", ".join(guide["topics"]),
                  "JSON: copper agent-guide TOPIC --json"])
    return "\n".join(lines)
