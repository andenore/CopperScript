# Power area planning implementation plan

Implement a bounded power and ground area planner inside the standard
`route-board` flow. Develop and measure it on selected CopperProbe domains before
attempting a full-board run. Keep the FPGA-free architecture, analog front end,
source constraints, fabrication limits and dependency versions unchanged.

Status: the opt-in planner now participates in placement trials and the standard
`route-board` flow. `power_geometry.py` generates bounded layer/tree alternatives,
ordinary via contacts and corridor polygons, certifies physical conflicts and
allocates competing domains jointly. `power_extract.py` screens explicit and
fresh native-filled copper with bounded DC meshes. This is experimental software;
expanded consumer acceptance and full-board promotion remain gated below.

Latest validation (2026-10-19): the corrected capacitor placement and reserved
85-contact package pattern now support native-accepted preparation of all eleven
FX10/ADC power domains, 63 bypass associations and 112 required return lands on
six layers. The complete normal preparation takes 905.772 seconds, generates
50 candidates, uses 2320 allocator states and sixteen repair slots, and accepts
its second fresh native trial. It retains 183 whole-board opens, zero other
native findings and zero islands. The final electrical-model guard is checked
with a fresh same-source candidate-pool replay: 117.431 seconds, 2515 cumulative
allocator states, sixteen repair slots and exactly identical accepted geometry.
All fixed poses, analog intent, source constraints and required vias survive.
Ordinary signal handoff, full routing and electrical qualification remain
separate gates. Earlier results below are historical and bound to their own
source, options and reservation scenes.

The complete ordinary handoff remains rejected. It has three fanout failures
(PWR/U.13, U_ADC.6 and U_ADC.7) and ten FX10 boundary-exit failures (D2, E12,
F11, G5, H11, H4, K10, K5, K7 and K9). Each of those FX10 pads now has a physical
launch; K5/K9 have no generated boundary path, and the remaining eight cannot
be allocated compatibly within the configured bounds. Native refill retains
every selected power/return join and prior connected pad group, but reports
175 opens, 78 dangling vias and sixteen dangling tracks, with zero islands or
other finding types. The proposed ordinary copper is not adopted. The one-pass
global check still fails ADC_D7, ADC_OVR and DFS. The final 172-test affected
suite passes with zero failures, errors or skips (302.163 seconds).

The first contact-feasibility implementation (2026-10-17) now joins missing
primary-layer contacts through actual same-net components on other permitted
layers. It retains terminal/layer/bridge/search observations and shares the
existing per-growth branch allowance. Native-backed return preparation can
also reject a filled assignment and reselect from its existing certified pool.
`maximum_native_trials` is an explicit positive bound, defaulting to one;
reallocations share the original allocation-state cap and generate no new
geometry. Rejections exclude complete joint assignments rather than globally
banning individual candidates. Every trial starts from the same held baseline.

The expanded source run now connects all eleven selected FX10/ADC domains,
all 63 bypass associations and all 112 selected return pads in one scene.
ADC ground pad 49, all seven A18 associations, the three FX10 bank ground
contacts and all fifteen critical nets pass fresh native checks. The result
retains all prior native pad groups, zero other violations and zero islands;
183 whole-board unconnected items remain. All 35 non-copper source fields
match the critical prefix, including analog keepouts, fixed poses, stackup
and fabrication rules. The scoped prepared package handoff has no pending
pads, zero hard findings and 103 allocated boundary ports; fresh native
revalidation preserves every domain, selected return and old pad group.
Global handoff and full-board promotion remain separate gates. The preparation
took 771.837 seconds, generated 42 candidates,
selected eleven, spent 400 allocation states and eight conflict retries, and
accepted its first native assignment within the original configured limits.

The subsequent complete-source 1.25 mm / 20-iteration global comparison still
fails at ADC_D7 / U_USB.B2, ADC_OVR / U_USB.E1 and DFS / R_DFS_TOP.2, with seven
capacity overflows. Full ordinary-package preflight has twelve pending exits:
eleven FX10 data/control contacts and the buck converter's VBUS pad. Eleven
have no candidate in the bounded immutable-input search; the remaining pad has
seven initial candidates blocked by the selected escapes. This is not an
exhaustive geometric infeasibility result. All 165 proposed boundary ports for
the available exits allocate, with zero hard geometry findings.

That unadopted ordinary proposal retains every selected power/return join and
old pad group, but native refill reports 62 dangling vias and 24 dangling
tracks in addition to 177 unconnected items. There are no shorts, clearance
findings or islands. The native preservation gate correctly rejects it. The
accepted eleven-domain prefix remains the retained result; full-board detailed
routing has not started. Next work must reserve ordinary exits jointly with
power/ground contacts, hand verified physical access into global planning and
review intermediate ordinary-stub ownership without silently waiving native
findings. The focused return/preparation/area integration suite passes 102 tests
with zero failures, errors or skips, including real KiCad checks.

A subsequent read-only ownership audit maps all 62 dangling vias and 24 dangling
tracks to newly generated ordinary fanout, outside the unchanged 1315-track /
176-via accepted prefix. Every flagged item is supported by an explicitly
verified pad-launch path; all 189 recorded fanout launches verify. This does not
establish a legal continuation or identify the precise unfinished endpoint.
The proposal remains rejected. The implementation research and next gates are
recorded under [Joint ordinary access and global handoff](#joint-ordinary-access-and-global-handoff).

Return completion now shares the exact mixed-layer component join. A new native
gate requires every selected return land, including each target package's
exposed pad, rather than accepting one connected target ground per bypass.
Prepared package access rechecks and retains that native ledger, so a repeated
local site search cannot falsely reject an already accepted distant contact.
Other requested stitch pads, fixed poses, process rules and all prefix copper
remain protected. Source and native input digests distinguish each result.

The earlier complete-source USB18 subset accepted the second native assignment:
all fourteen selected bypass associations, all fifteen critical nets and FX10
ground balls N4/N6/N8 connect, with every previous native pad group preserved.
The first assignment creates two extra islands and is rejected; the accepted
one keeps the baseline's two islands. Its 357 remaining unconnected items are
partial-board work, not a full-board DRC pass. The expansion above supersedes
that subset's geometry acceptance; electrical/PDN qualification remains open.

An earlier 2026-10-17 reference-bank candidate preserved all eight critical
groups, all ten power domains and 63 selected power/ADC associations in one
fresh native-checked scene, with three identical power regenerations. The
reference comparison identified a generated surface ground return fencing
off an FX10 bypass supply contact. The accepted fresh candidate uses local
ground-plane contacts for FX_USB18 while retaining the other nine domains'
explicit local feeds. Actual critical copper supplies its layer-specific
reservation; a blanket two-face corridor must not exclude bottom bypasses
beneath an accepted front signal route.

The return-policy sequence now has an opt-in standard-pipeline implementation
in `power_preparation.py`, selected with `PowerAreaOptions.plane_return_nets`.
It routes critical copper first from the complete source, retains the other
selected domains' explicit feeds, stitches declared local returns, plans power
areas and requires fresh native source-to-load and bypass joins before adoption.
Native-backed return continuity is a separate ledger; it does not relabel an
explicit local-loop witness or qualify decoupling impedance. Generic native
handoff/rollback tests pass. The earlier standard CopperProbe replay preserved all
eight critical groups and all ten domains with fresh native evidence, but stops
at five pad obligations: FX10 ground balls N4/N6/N8, ADC ground pad 49 and ADC A18
pad 18. Native refill confirms isolated one-pad groups; these are not merely
conservative stitch reports. All 99 probed boundary ports can be allocated, yet
the package gate correctly blocks ordinary routing. The post-handoff scene has
62 connected, five open and two unsupported bypass assessments, 195 unconnected
items overall, no other native violations and no lost prior pad groups. This
does not reproduce the historical separately composed 63-association ADC trial.

The 1.25 mm complete-source global comparison also remains unreachable after
20 iterations: ADC_D7 to U_USB.B2, ADC_OVR to U_USB.E1 and DFS to R_DFS_TOP.2;
overflow falls from 32 to four. A sampled physical diagnostic finds 21 legal
exits with no pending pads or hard findings. Next work must jointly reserve
required ground contacts around critical/power ownership, prepare the ADC area
before ordinary fanout and hand verified physical/native connectivity back to
global guide planning. Full-board promotion remains blocked.

Do not retrospectively remove
accepted return paths or treat a plane outline as their replacement.

The accepted complete prefix is occurrence-owned through ordinary routing,
cleanup and native feedback. Source/pose changes require a new preparation;
geometry changes require fresh native fragment/association and old-pad-group
checks. Rejected boundary or ordinary proposals roll back completely. Package
pattern and detailed-placement repair do not mutate this accepted prefix; a new
placement must restart preparation. Missing native tools, absent target return
terminals and failed allocation remain explicit blockers. The default explicit
return path is unchanged when no plane-return domains are selected.

The API's `routing_nets` diagnostic work scope retains all physical source
objects and every critical route. It cannot produce a full-board pipeline pass.
The common candidate remains a partial board, with electrical and hardware
qualification unresolved.

The stage runs after package-boundary reservations and before ordinary detailed
routing. It rechecks held exits against the changed board. Placement trials use
private materialized snapshots and discard rejected plans with their trial.
Generated copper has separate occurrence ownership through late escape/native
repair; it is not ordinary disposable routing. Existing source geometry, local
bypass associations, critical paths and required arrays are retained.

## Contact feasibility follow-up

Mixed-layer contact joining and bounded native assignment fallback are now
implemented and have scoped consumer evidence as described above. The remaining
site-generation, joint contact-selection and placement extensions below are
planned work. Native continuity on one supply must not compensate for losing
a ground contact or an existing pad group. A lower total open count is
insufficient for promotion.

The contact proposer in `power_geometry._prepare_contacts` delegates to
`plane.stitch_zone_pads`, then `_join_layer_contacts` can supply missing accesses
through real same-net components before `_tree` requires primary-layer anchors.
This closes a gap where valid mixed-layer contact paths prevented all area
proposals. Contact observations distinguish the actual terminal and layer;
expanded state counts and sampled blockers cover the new bridge search, not
the earlier plane-stitch search. Full site-level stitch diagnostics remain
unfinished. Do not treat temporary zones as conductive or increase the
allocator budget to compensate for absent contact candidates.

Implement and validate these generic extensions in order:

1. **Retain contact failure evidence.** Report the exact terminal/component,
   attempted layer and span, generated and rejected sites, actual emitted
   neckdowns, sampled pad/copper/region/drill blockers, search expansions and
   stop reason. Separate an exhausted allowance, unavailable sampled sites,
   missing layer access, ambiguous terminals and native rejection. Keep this
   evidence even when the complete preparation rolls back. Do not label a
   bounded candidate failure physically impossible.
2. **Share geometry-derived site generation.** Reuse the existing fanout
   interstitial-cell proposer in plane/contact preparation. Supplement cell
   centres with a bounded set of offset sites derived from the boundaries of
   expanded obstacles and free-space clearance maxima. A cell centre alone
   misses legal off-centre sites. Check the full through-via span, backside
   lands, drill separation, board material and immutable regions. Same-net SMD
   overlap still requires the existing explicit assembly/process permission;
   it is not an ordinary via reuse shortcut. Share site/path budgets and exact
   predicates rather than adding an unbounded fine grid.
3. **Connect components across their actual accessible layers (implemented).** Represent
   physical explicit components, their real ports/layers and proposed legal
   trace/via edges. Permit a missing terminal to join an already accessible
   same-net component on its own layer before reaching the backbone. Check the
   resulting explicit graph before area synthesis. Reuse proved, dispersed
   source-connected anchors; a nearby capacitor or pour outline is not an
   assumed source. Preserve the original source/feed obligation and every
   pin-associated bypass requirement. Keep structural continuity separate
   from local supply/return loop and electrical quality assessment.
4. **Select supply and ground access together.** Generate bounded alternatives
   for constrained package lands, local capacitor supply/return contacts and
   signal transitions. Build exact conflict/coverage certificates and select
   a compatible set before committing a disposable ground arrangement. Reuse
   `escape_assignment` and `power_areas` mechanisms where their contracts fit;
   do not introduce designator-specific routing or assume a dedicated new via
   is mandatory for every pad. Account for required array multiplicity and
   declared local sharing limits. One supply-first or ground-first order is
   only a candidate, never the complete policy. Joint contact feasibility must
   include every required ground ball, not just capacitor return assessments.
5. **Expose access to placement and macro authoring.** Score whole legal
   placement units using both supply and return access, local loop geometry,
   lost package exit choices and reference continuity. Use allowed rotations
   and capacitor-pad orientations; minimum pad distance alone can block the
   only through-via site. Fixed poses and existing immutable macro regions
   remain hard constraints. Fail with an identified owner when they prevent
   access. Revised macro/footprint assets need explicit source revisions and
   fresh evidence, not an automatic carve-out or relaxed reservation.

Integrate this with the existing separate power/ground area stage. During a
placement trial, materialize fixed intent and macros, assess/reserve local
power/ground access jointly with critical escape demand, realize critical
routes, then refine local contacts and distribution areas. Actual critical
copper remains authoritative. A rejected early reservation may be reselected
only within the bounded disposable preparation; accepted critical/array/macro
copper and the incumbent prefix are never ripped up to make a later trial fit.
Complete native fill, prior-pad-group preservation, per-contact and bypass
checks before handing the accepted result to ordinary routing. A placement
change restarts preparation and invalidates prior evidence.

Keep allowed power-distribution layers distinct from signal-reference layers.
Filtered and upstream domains retain separate net identities, even at the same
nominal voltage. Existing authorized power-zone partitioning can distribute a
rail after contacts are legal; it cannot create missing plated transitions.
Use electrical inputs for drop/current/thermal screening when supplied and
retain missing inputs as unresolved. Do not infer high-frequency PDN quality
from a connected mesh or adopt another board's stackup/impedance dimensions.

Focused acceptance sequence:

- A terminal with no backbone-layer contact but a legal same-layer bridge to
  an existing contact; a negative control with a genuinely blocked bridge.
- A legal off-centre site whose cell centre is blocked, including rotated and
  mirrored package geometry and an opposite-face same-net SMD land exclusion.
- Two competing supplies and required ground contacts where either greedy
  order loses an obligation; deterministic joint selection within unchanged
  explicit trial/state/site limits.
- A source-bounded dense-package consumer subset with every actual bypass,
  filter feed, adjacent critical route and ground contact present. Require
  fresh native supply joins, ground-ball joins, all critical joins and no
  split old pad group or new non-connectivity finding. Report partial-board
  opens and islands separately; never use their aggregate improvement to
  hide a lost obligation.
- Expand to competing package domains, then ADC/analog preservation, and only
  then one normal bounded full-board run. Keep the full-board gate closed
  until all previous requirements pass in one scene.

## Joint ordinary access and global handoff

Research date: 2026-10-17. The diagnostic/site-generation implementation below
is complete as a first increment. Joint contact allocation, continuation-ledger
admission and physical-port global handoff remain planned changes. The retained
proposal still has twelve missing exits; no full-board route or adoption is claimed.

### First implementation and coupled subset validation

`access_diagnostics.py` now retains finite site/path observations, emitted
geometry, layer/span failures, exact blocker occurrences and source/proposal
digests. `routing_clearance.py` distinguishes even geometrically identical
input tracks/vias and incremental additions without changing lock authority.
Fanout reports sampled site exhaustion separately from finite assignment
failure, and the CLI includes detailed observations for pending pads. Each pad
retains at most 24 samples with at most eight blockers each; at most 24 blocker
collection callbacks run, each possibly inspecting several emitted path pieces
and a via. Assignment observations have reserved sample slots.
Maze/surface internal site and state telemetry remains explicitly unobserved,
not zero, and every sample/truncation limit stays visible.

`access_sites.py` shares the circular-cell proposer between fanout and plane
contacts. A separate obstacle-boundary proposer checks every spanned layer when
deriving frontiers, including opposite-face same-net lands and drills. Its
sites remain proposals subject to all existing exact path/annulus/drill/process
checks. Fanout's `maximum_geometry_candidates` defaults to zero, so this extra
family has an explicit experimental work allowance rather than silently
expanding every run. The measured trial requests 32 proposals per physical land.
The standard CLI exposes this as `--fanout-geometry-candidates 32` with fanout
enabled; pending-pad diagnostics and both new limits are included in its report.
The shared cell proposer contributes at most four sites to plane contacts;
existing ordered radial termination and maze allowances are preserved.

The small native fixture finds a legal off-centre ordinary through-via where
the cell centre overlaps an opposite-face same-net SMD pad. Mirrored and
quarter-turn cases pass exact DRC; a fully blocked control remains blocked.
No via-in-pad permission is added. The 152-test escape/clearance/power/native
integration run passes without failures, errors or skips. After the sample
reservation refinement, the overlapping 95-test diagnostic/native-boundary
suite also passes without failures, errors or skips. Do not sum these suites.
The final 23-test diagnostic/site/CLI suite also passes, including native geometry.

The complete-context CopperProbe fixture scopes work to fifteen nets while
retaining all 166 components, all source obstacles and the accepted power
prefix. Established generation reproduces all twelve pending exits in 25.111
seconds; 32 additional geometry proposals leave the same twelve in 30.416
seconds. Both preserve the exact 1315-track/176-via prefix and all 35 other
physical fields. E12's four-pad cluster (D12/D13/E11/E12) has no compatible
assignment in its finite pool after twelve search states, rather than budget
exhaustion. Sampled conflicts identify the generated ADC_SCLK launch by exact
track/via occurrences. The zero-domain samples repeatedly identify held GND
and supply copper as well as package lands; these samples do not prove a
minimum blocker set or physical impossibility.

Fresh KiCad 10.0.6 validation of the geometry-enabled subset preserves all
eleven domains, 63 selected bypass associations, 112 return lands and every
old pad group. It reports 183 opens, twenty dangling vias, two dangling tracks,
zero other finding types and zero islands. All 22 dangling findings map to new
owned ordinary copper, outside the accepted prefix. The native power gate still rejects
this incomplete ordinary proposal. No candidate replaces the incumbent.
Evidence is in `CopperProbe/verification/fx10-ordinary-access-diagnostics-20261017/`;
the compiler-side work directory is `build/access-subset-20261017-b/`.

### What the implementation and retained evidence establish

| Issue | Current mechanism | Required change |
| --- | --- | --- |
| Eleven pending lands have zero sampled candidates | `fanout._package_candidates` proposes at most four circular-cell centres, then bounded rays; two-leg and maze refinements are also bounded | Retain site/path rejection causes and generate additional geometry-derived alternatives before changing selection order |
| FX10 E12 has seven candidates conflicting with selected exits | `escape_assignment` already has minimum-remaining-values search, forward checking and lazy exact conflicts | Diagnose the failed cluster and shared work limits; extend selection to negotiable power/return contacts instead of proposing another greedy signal ordering |
| Accepted supply/return copper prevents later negotiation | `preflight_prepared_power_access` adds ordinary fanout after native power preparation | Select competing signal/supply/return accesses within a disposable preparation before freezing distribution areas |
| Global planning does not consume the ordinary boundary contract | `route_global` derives access from raw pads and macro ports; `PackageAccessResult.ready` requires initial global success before `flow` commits boundary paths | Separate physical-access readiness from global readiness, commit verified paths, then refresh guides from their real ports |
| Unfinished fanout is rejected as a native power regression | Both `assess_power_plan_fill` and `compare_power_fill_connectivity` require zero other native violations | Add explicit intermediate continuation obligations while preserving raw native findings and strict final acceptance |

The retained inputs are `build/fx10-adc-expanded-20261017-b/prepared-state.json`,
`build/fx10-full-access-20261017-d/access-state.json` and that trial's
`native-handoff/evidence.json`. The reproducible classification is
`build/ordinary-power-handoff-research-20261017/audit.py`; `audit.json` records
input hashes, KiCad 10.0.6, the matching physical-board digest, unchanged prefix
occurrences and each flagged object's exact native index/UUID and launch pads.
It performs no native rerun and issues no waiver. The associated consumer
evidence remains in `verification/fx10-inward-escape-20261017/expanded-power-return/`.

### Research basis and applicability

- [Bayless, Hoos and Hu, ICCAD 2016](https://ada.liacs.nl/papers/BayEtAl16.pdf),
  sections 2–3: movable multilayer vias occlude neighboring trace resources,
  so pure network flow is insufficient. Their constraint formulation chooses
  traces and vias together and can hold partial solutions fixed. Section 5
  pre-routes power/ground access; it does not solve our coupled supply/return
  problem. Reuse this conflict-model principle, not its benchmark assumptions.
- [TI AM570x BGA Escape Routing](https://www.ti.com/lit/ug/spruip9/spruip9.pdf),
  sections 4.3 and 7: protect critical interfaces, allocate PDN space early and
  coordinate top-side via patterns with bottom-side bypass placement. Figure 15
  deliberately shares certain supply/ground accesses to leave capacitor space.
  This is a different device and process; it gives no permission to reduce an
  FX10 array, copy dimensions or weaken the existing local-loop requirements.
- [SPROUT, TCAD 2022](https://www.hajim.rochester.edu/ece/sites/friedman/papers/TCAD_SPROUT_22.pdf),
  section II: build an obstacle-aware multilayer graph, establish terminal
  connectivity, then grow/refine conductor shapes against electrical criteria.
  For CopperScript, joint local access should precede area growth. Reserving
  ordinary corridors inside that process is our proposed extension, not a
  demonstrated result from SPROUT.
- [TritonRoute, ICCAD 2018](https://vlsicad.ucsd.edu/Publications/Conferences/363/c363.pdf),
  section 4.3: an access point includes connecting trace/via geometry, rather
  than only a coordinate. This supports a typed physical-port handoff. Its IC
  routing technology differs from PCB fabrication; use our own full-span
  clearance, drill and assembly predicates.
- [OpenROAD global routing documentation](https://openroad.readthedocs.io/en/latest/main/src/grt/README.html)
  and [pin-access/detailed routing documentation](https://openroad.readthedocs.io/en/latest/main/src/drt/README.html)
  separate resource accounting, pin access and detailed repair. They support
  explicit access diagnostics and refreshed routing resources, not invented
  capacity or certification from coarse guides.
- [KiCad connectivity DRC implementation](https://docs.kicad.org/doxygen/drc__test__provider__connectivity_8cpp_source.html)
  tests dangling tracks/vias independently of spacing and short checks.
  Interpreting a declared unfinished end as a temporary routing obligation is
  our proposed stage policy. Verify it with the pinned KiCad version; current
  online source is not a substitute for native fixtures.

These sources support the architecture, not a turnkey solver for this board.
Start with the existing bounded conflict solvers. A new SAT/ILP dependency is
an optional later experiment if measured local clusters justify it; neither
greater solver complexity nor increased allowances repairs an absent candidate.

### Concrete next implementation milestones

The following milestones track the remaining joint-access work as separate
reviewable changes. The first implementation increment is recorded below;
unfinished parts of each milestone remain requirements. The target is first a measured reduction from twelve
pending exits while preserving every held obligation, then zero pending exits
before any complete-board detailed run.

Implementation increment (2026-10-17):

- **M1 capture is implemented in standard power preparation.**
  `copper_ownership.py` records the exact ordered track/via occurrences, producer,
  source obligations and dependency closure. Existing critical/source copper is
  fixed. Local bypass, plane-contact and return-bridge producers contribute
  evidence; ambiguous or unattributed occurrences stay held. The bridge fallback
  conservatively retains the entire explicit copper component as potential
  support, rather than guessing its shortest path. Accepted preparations freeze
  the ledger, including distribution areas. Private release rejects stale
  snapshots, partial dependency closures and protected source/array occurrences.
  The accepted eleven-domain incumbent is never made negotiable.
- **The M2 selector and its fixture are implemented.** `joint_access.py` accepts
  complete, digest-bound alternatives with all affected source obligations,
  multiple vias and actual layer-specific ports. It shares the bounded
  minimum-remaining-values/forward-checking kernel with ordinary fanout, checks
  candidate conflicts in both directions and permits a shared physical hole only
  when both alternatives declare it. The native synthetic fixture moves a return
  contact that blocks a signal and closes all three signal, supply and return
  nets with zero opens, other findings or islands. Zone holes, zone keepouts and
  wrong-layer targets cannot satisfy prospective contact requirements. Results
  are geometry proposals and carry no native acceptance authority. Automatic
  contact/decoupling alternative production and integration into disposable
  preparation are still required before M2/M3 are complete.
- **The M3 checkpoint has not been reached.** The complete-context fifteen-net
  replay still has twelve pending exits. Extending geometry proposals to
  nonempty conflicting domains, as well as empty domains, does not add a legal
  E12 candidate: all 32 additional E12 via sites are blocked. Its seven-candidate
  four-pad pool still fails after twelve states. Fresh KiCad validation retains
  all eleven domain joins, 63 bypasses, 112 return pads and all old pad groups,
  but reports 183 opens and 22 ordinary dangling findings (20 vias, two tracks).
  The strict gate rejects this proposal. No new whole-board detailed run or
  source/analog change was made.
- **M4–M6 remain unimplemented.** This increment neither admits ordinary dangling
  ends nor changes the physical/global handoff or final clean-board gate.

Regression testing also exposed inconsistent occurrence labels in the existing
contact-owner regeneration index. Generated reservations now retain their exact
positions in the complete snapshot, so freshly reconstructed blocker records
match without weakening source or duplicate-geometry protection.

The full-context `FX_XTAL` ownership pilot uses the unchanged placed consumer
source, regenerates all critical groups normally and selects one power domain.
It is native-connected for that domain and its required returns, with 740 exact
copper occurrences (629 held, 16 local-bypass, 87 return-contact and eight
distribution-area occurrences), 33 recorded uses and a frozen matching ledger.
All 166 components and 35 physical-intent fields are preserved. This scoped
preparation does not replace the eleven-domain incumbent or prove other rails.
Its whole-board native report retains 387 opens and the two baseline islands,
with zero other findings; only the selected domain and required returns pass.
The broader increment suite passes 186 tests with no failures or skips. The
final contact suite passes 17 tests, including four new zone/layer controls;
these suites overlap and are not summed. Native evidence for the synthetic
joint fixture is saved separately from consumer evidence.

Second implementation increment (2026-10-17): automatic plane-contact and
complete bypass alternatives now feed the shared selector through
`joint_preparation.py`. Contact witnesses use bounded breadth-first search over
the same positive-area, layer/drill-aware contacts as DRC; they no longer claim
every unrelated branch of a connected return component. All recorded affected
obligations must survive a replacement, while outside producers and source
occurrences remain held. Candidate generation, pair queries and assignment
states have cumulative limits across the private trials. Protected blockers,
unsupported source associations and oversized dependency groups remain explicit
failures.

`prepare_power_distribution` now calls this stage before area growth when
`joint_contact_trials` is positive. A proposed contact replacement needs an
additional fresh native comparison with the original contact scene. Area growth
must also preserve the verified ordinary launch reservations. Their conservative
track and full plated-span envelopes become temporary planning corridors, with
no authored keepout or signal copper added to the accepted power board. The
native extractor and final clean-board requirements remain strict.

`ContactPreparationSnapshot` and `prepare_power_from_contacts` allow the normal
bounded stage to restart from a genuine private producer checkpoint. Exact
source and critical digests, ordered protected prefixes, physical intent,
selected domain/return identities, macro ownership and array multiplicity are
checked again. A frozen accepted scene or changed source cannot serve as this
checkpoint. Native baseline and candidate evidence are regenerated on every
replay. This reduces iteration work without removing the complete board context.

The fresh full-context checkpoint has only three ordinary failures before area
growth: B2, C2 and D2. The unreserved fifteen-net replay after area growth again
has twelve, including E12. The bounded contact negotiation records thirty
protected-blocker cases and four dependency-closure bound failures; it generates
no eligible replacement on those sampled roots. These are finite-pool failures,
not a physical infeasibility proof. The eleven-domain preparation itself passes
native continuity and retains all selected bypass and return obligations.

The completed comparison reaches M3's first recovery checkpoint: the scoped
fifteen-net replay falls from twelve pending exits to eleven, with E12 recovered.
Fresh native evidence retains all eleven domains, 63 selected bypasses, 112
returns and every connected pad group of the old eleven-domain incumbent. The
accepted power scene has zero other native violations and zero islands.
The partial ordinary scene still has 183 opens and 23 ordinary dangling findings,
so the strict package/native gate continues to reject full routing admission.
The FPGA-free source, analog placement and all 34 checked physical-intent fields
are preserved. The historical incumbent is retained separately.

An intermediate corridor-only proposal passed power continuity but failed the
combined reservation check: an A18 via and the ADC-side ADC_SEN via had 0.500 mm
centres where 0.550 mm copper spacing was required, plus drill/copper violations.
The corrected planner inserts the actual reserved tracks and vias into clearance
indexes and disposable contact scenes, then transfers only new power copper out
of those scenes. Shape corridors continue to protect area growth; actual plated
objects enforce drill, annulus and source-specific spacing. The rejected native
assignments remain saved as failures.

`refresh_power_area_reservations` revalidates the same-source geometric candidate
pool and regenerates only blocked power families. It retains the original work
limits and cumulative allocator usage; electrical-model pool reuse is rejected.
The corrected replay regenerates A18 only, retaining 41 candidates and using
935 of 4096 allocator states and all twelve conflict/regeneration slots. One
fresh final native assignment passes. This cached follow-up takes 85.903 seconds;
the earlier pool-generation work is retained and is not counted as free work.

A same-compiler unreserved replay of the enlarged pool also has eleven pending
exits. Consequently, the evidence supports recovery through the improved power
topologies and their fresh selection, not a claim that corridor reservation alone
causes the improvement. Both accepted cached candidates preserve the old pad
groups. The original unreserved control has twelve failures; the intermediate
reserved candidate was rejected; the corrected cached result has eleven.

The current focused suites pass 163 core, 55 final reservation and 48 final
native-stage tests, with no failures or skips; these overlap and are not summed.
M3's remaining zero-domain cases, M4 continuation obligations, M5 physical-port
global handoff and M6 complete detailed/native closure remain unfinished.

### Required reference comparison of the remaining exits (2026-10-17)

Package-escape work now requires the same-pin, layer-by-layer manufacturer
comparison in [integration guidance](component-integration-guidelines.md#package-escape-faults-reference-comparison-is-required).
The consumer's `verification/package-reference-current-20261017` records all
eleven current failures against the actual FX10 kit and TI converter layouts.
Reference coordinates are aligned by physical lands, with the 4014/4013 and
Port0 configuration differences explicit.

Six blocked ADC data balls (B2/C2/D2/G2/J2/K2) begin with narrow TOP surface
channels in the kit. The four control balls K5/K6/K7/K9 instead use compact
diagonal dogbones. Their actual reference via offsets are 0.3328 mm per axis;
the reference's 0.4064/0.2032 mm vias cannot silently replace the consumer's
0.45/0.20 mm rules. The converter's pad 13 is EN, tied to VBUS in CopperProbe;
it needs a source-backed control branch, not an inferred reduction of every
VBUS power conductor. TI's EVM routes EN separately to its enable jumper.

The first generic change projects obstacle frontiers onto actual octilinear
launch rays, allowing an offset via to retain a single straight dogbone. This
extends the existing opt-in geometric site family; its candidate/obstacle caps
and all exact clearance, drill and pad checks remain authoritative. Rotated and
mirrored regressions and native geometry checks pass. The same-context bounded
probe still finds zero candidates for each of the eleven failures: no recovery
or new board/native admission is claimed.

Next work must preserve the reference's surface channels before local bypass
and return producers occupy them, negotiate compact supply/return dogbones and
support placement together, and model authorized control-branch approaches.
Actual reserved tracks/vias and independent native checks remain necessary;
an unfilled corridor or a visually similar route is not acceptance. M4's
pending-continuation contract must precede adopting incomplete ordinary copper
into an otherwise accepted power scene. Use bounded package subsets with
before/after reference comparisons before another full-board routing run.

### Early surface reservation implementation (2026-10-17)

The generic native preparation path now accepts the opt-in
`reserve_ordinary_before_contacts` policy. Verified ordinary launches become
actual temporary track/via obstacles before local bypass and return producers
run. Regular circular arrays prefer surface channels on their outer two rows;
deep control balls retain via-first dogbones. Unknown arrays retain via-first
ordering. Source poses, net rules, macros and via processes remain binding.
The normal bounded pipeline consumes the same policy at the later fanout stage.

Surface maze search now checks the actual source-authorized breakout pieces
and enforces collar distance plus the configured detour. The bypass local-site
family can mix obstacle-frontier and analytic points within its original site
cap. Missing source-declared local bypasses can use the existing joint trial
allowance after return preparation. Any remaining allowance can negotiate exact
mutable contact dependencies with the early ordinary paths held fixed; duplicate
dependency frames are skipped. Native preservation is still required before a
replacement can enter area planning. These are implemented bounded mechanisms,
not new acceptance exemptions or larger search budgets.

The complete-context CopperProbe comparison found that six reference-style
surface data exits have candidates before power/return generation and none
after the old preparation. Early planning reserves 28 ordinary ports, including
those six surface exits and the four deeper via exits. An initial unrestricted
surface policy lost two bypass associations and three native ground contacts;
it was rejected. The depth-aware policy restores C11 and all 112 native return
contacts, but still loses the E3 D18 bypass. The private native comparison is
260 to 261 opens, with zero other violations and zero islands on both sides;
one prior D18 pad group is separated. This is not the accepted full-board
incumbent's 183-open snapshot, and none of these trials replaces it.

The full eleven-domain area replay has 33 candidates across ten domains and
zero for D18. Allocation does not start: `C_FX_E3.1` has only its back-side
contact and cannot reach the permitted distribution layers within three bounded
4,000-state bridge searches. This is an empty candidate family, not exhaustion
of global allocation states. Direct local completion and exact-contact joint
negotiation also fail to produce a complete E3 supply/return proposal within
their recorded bounds. The reference comparison includes E3, C11 and the three
temporarily lost ground balls L8/N6/N8. Reference E3 capacitors C59/C62 are
on the bottom beyond the outer BGA ball row; the consumer's fixed E3/C7 bank occupies
underside escape space. Reference values and differing voltage/configuration
assignments are not inferred from physical artwork.

The next placement work must generate source-authorized support arrangements
outside contested package corridors and jointly verify supply, return and held
signal paths. It must preserve fixed/rigid intent or report that a source pose
change is required, rather than silently moving this consumer's fixed bank.
The EN/VBUS control branch still needs its separate source-backed rule. M3's
remaining recoveries, M4's continuation ledger, M5's physical-port handoff and
M6's complete routing/native closure remain open. Planning power and ground
copper areas remains a separate stage before ordinary detailed routing.

### Pattern-based support placement research (2026-10-17)

The authorized four-capacitor CopperProbe experiment is retained separately as
`hybrid-support-pattern-bank`, with evidence in
`CopperProbe/verification/support-bank-trial-20261017`. It exposes two generator
limitations: same-rail balls on different edges need separate pin-local banks,
and opposite-face banks need a land-field rim in addition to package-courtyard
edges. Both generic corrections are implemented. The final bounded subset
checks 480 whole-bank poses in the full source placement context and finds no
legal alternative with the neighboring fixed banks retained. This is a sampled
pool result, not an infeasibility proof. No poses or copper are adopted.

Fresh unchanged-geometry native controls preserve all 11 power domains, 63
selected bypasses, 112 return pads, all 15 critical nets and every old connected
pad group (183 whole-board opens, zero other findings/islands). Critical copper
was independently regenerated and matches the incumbent exactly. This control
does not validate a moved bank or close the early-reservation D18 distribution
gate. This is the frozen control for the later offset implementation below.

The follow-up implements bounded configuration-space bank offsets and preserved
source frames. In the same four-authorized-component experiment it uses 353/512
pose probes, eight complete combination checks and six routing passes. The
selected source frame shifts the two C7 capacitors by +0.2058 mm in x and retains
both E3 poses; all other component poses remain exact. Four scoped explicit
supply/return paths and 78 temporary ordinary ports survive clean regeneration.
The final compiler and consumer contract suites pass 79 and 22 tests respectively.

Fresh complete-source critical/contact regeneration preserves all 15 critical
nets, 28 scoped ordinary ports and 112 returns. Before distribution fill, the
previous E3 open is repaired but C_FX10_C7 remains open; this intermediate scene
is not accepted. The normal bounded D18 area subset then selects In3.Cu and
fresh native fill connects all seven D18 bypasses and every D18 terminal,
preserving all prior pad groups from its contact baseline and all 112 returns.
It reports 251 whole-board opens, zero other findings and zero islands. This
closes the scoped D18 area-access gate, not complete routing or PDN qualification.
The combined eleven-domain replay and source-bound before/after records belong
in `CopperProbe/verification/support-bank-offset-20261017`.

The combined replay is complete and remains unresolved: 38 candidates cover all
11 domains, but no compatible selection survives 12 regrowth slots and 730/4096
allocator states. Nine slots regenerate FX_USB18. The last sampled blockers
resolve to trial-mutable return-contact, D18-bypass and FX_V33-bypass owners in
the exact contact ledger; some sampled identities remain unmapped. No combined
native candidate or full routing is admitted, and the old incumbent is unchanged.
Next, replay blocked power joins into complete ownership witnesses, then feed
their mutable dependencies back into bounded joint contact/area alternatives.
Do not treat diagnostic samples as complete cuts, release source/macro/array
copper, or reuse native acceptance after changed contacts. Reprove all 112
returns, supply associations and held signal ports before accepting a replacement.

#### Implemented area/contact feedback (2026-10-17)

The standard private power-preparation stage now accepts an opt-in
`area_contact_trials` bound (default zero). After an unresolved area plan, each
slot permits one exact contact dependency trial and one fresh area pool. Power
allocator states and area regrowth slots are deducted cumulatively across
changed contact scenes; native candidate allocation uses the remaining limit.
Contact replacement needs additional fresh native pad-group preservation,
all required returns and ordinary-port proof before replanning. Rejection never
adopts its private copper. Native area acceptance remains a separate gate.

`power_contact_feedback.py` replays failed endpoints into a complete candidate
path, then queries every segment against a fresh clearance index. Diagnostic
sample identities never authorize release. Regrowth failures retain their
actual held candidate IDs so replay also holds the neighboring area prefix.
Frozen/source/macro/array, ambiguous and unattributed copper stays protected.
During discovery, complete shared dependency components exceeding 64 mutable
occurrences or 12 obligations stay held too. Up to 24 distinct roots share
16000 path states, distributed so an early blocked root cannot consume the
whole allowance. Exact ownership closure and joint full-obligation checks still
decide every proposed release. The path is only a temporary obstacle during
regeneration, not adopted power copper or connectivity evidence.

The experiment exposed and corrected a separate handoff defect: plane-contact
alternatives now retain their actual proposer targets, including reused vias
and empty new-copper suffixes. The joint allocator uses that actual plated
target instead of relabelling a surface lead-in as an inner-layer tail. Exact
occurrence differences also preserve temporary reservation multiplicity after
contact copper is reordered. Fresh physical and native checks remain required.

The final saved CopperProbe trial regenerates the generated L9 ground contact
and frees a complete proposed In2.Cu USB18 path in 27.34 seconds of contact work,
one dependency trial and 115 path states. Fresh native checks preserve all
previous pad groups, 112 required returns, 28 ordinary ports and all 15 critical
nets (261 whole-board opens, zero other findings/islands). This is an intermediate
contact scene, not a comparison with the complete eleven-domain incumbent.

The aligned actual kit L9 escape runs northeast from (85,16) mm to
(85.3328,15.6672) mm. That location overlaps a fixed opposite-face capacitor
ground pad and a protected USB macro in CopperProbe. The consumer instead
reuses the existing ground via at (84.625,17) mm, replacing its previous via at
(84.625,15.625) mm. The lead-in grows from 0.75 mm to about 1.155 mm versus the
kit's roughly 0.471 mm. This preserves legal geometry and native ground
continuity but is not an electrical-quality or PDN qualification improvement.
No component pose, fabrication rule, immutable macro or source definition is
changed by this contact trial.

The tighter replay holds seven actual neighboring area candidates (CORE11,
FX_P0, FX_V33, D18, FX_XTAL, FX_GPIO and A18). They remain geometrically legal
after the L9 change, but four FX_USB18 layer alternatives still produce no
complete tree. The subsequent dependency-aware retry checks 24 roots and
15999/16000 path states with no local replacement. Diagnostic-only complete
closures of the earlier blocked paths span 362–391 occurrences and 76–88
obligations, including 30–31 bypasses. Those larger groups were never released.

The next branch-repair work must retain shared trunks, record exact boundary
connections and repair only a bounded branch while freshly proving every
affected original association. This is distinct from M4's ordinary unfinished-end
continuation ledger. Merely increasing the release limit would turn a local
repair into regeneration across several domains. Combined native acceptance,
ordinary handoff and full routing remain open. Consumer-owned
before/after/reference/native evidence is in
`CopperProbe/verification/power-contact-feedback-20261017`.

#### Router and manual repair research (2026-10-18)

Research supports a smaller geometric edit with a broad validation scope. It
does not establish that any particular CopperProbe branch can be repaired.
No new routing result or electrical qualification is claimed by this section.

| Primary source | Observed behavior | Proposed CopperScript adaptation |
| --- | --- | --- |
| [KiCad 10 interactive routing](https://docs.kicad.org/10.0/en/pcbnew/pcbnew.html#routing-tracks-and-vias) | Walkaround leaves obstacles in place; shove moves eligible obstacles. Optimization can be limited to the immediate dragged section. | Try existing legal alternatives first; bound changes to explicit local connections and keep fixed geometry protected. |
| KiCad source at `620c8b4fdf0205301ef7f6b0118d55ecd89e7daf`: [joint traversal](https://github.com/KiCad/kicad-source-mirror/blob/620c8b4fdf0205301ef7f6b0118d55ecd89e7daf/pcbnew/router/pns_joint.h#L238), [line assembly](https://github.com/KiCad/kicad-source-mirror/blob/620c8b4fdf0205301ef7f6b0118d55ecd89e7daf/pcbnew/router/pns_node.cpp#L1080) | Line traversal stops at forks, physical vias/pads and disallowed locked segments; width changes can be boundaries depending on assembly options. | Define branch boundaries using actual copper contacts, layer and width, rather than the transitive set of owners. This inspected master commit is separate from the KiCad 10 manual. |
| Java Freerouting at `5aa1b24dc8e5af7f950c551c1e53de9077a84cd5`: [connection traversal](https://github.com/freerouting/freerouting/blob/5aa1b24dc8e5af7f950c551c1e53de9077a84cd5/src/main/java/app/freerouting/board/model/items/Item.java#L778), [rip-up](https://github.com/freerouting/freerouting/blob/5aa1b24dc8e5af7f950c551c1e53de9077a84cd5/src/main/java/app/freerouting/autoroute/maze/AutorouteEngine.java#L260), [optimizer](https://github.com/freerouting/freerouting/blob/5aa1b24dc8e5af7f950c551c1e53de9077a84cd5/src/main/java/app/freerouting/autoroute/pipeline/BatchOptimizer.java#L584) | Connection traversal ends at forks/terminals and can stop at vias. Rip-up expands crossed items into connections. Optimizer candidates cannot worsen completeness or clearance counts; rejected private candidate boards are discarded. | Use a separate branch transaction with explicit boundaries, private state and complete rejection rollback. Keep the current full-owner transaction contract intact. |
| [Altium interactive routing](https://www.altium.com/documentation/altium-designer/pcb/routing/interactive), sections on loop removal and partial gloss/retrace | A selected section can be modified between endpoints. Drawing a replacement path can remove the old loop; power and ground may intentionally retain multiple paths. Wider old routes receive protection from narrower replacements. | Prefer replacement paths between retained contacts where feasible. Do not infer that a DC-redundant power/ground path or via is removable. |
| [LayoutEditor's C++ freeRouting algorithm documentation](https://freerouting.org/freerouting/autorouter-algorithm) | Describes shove on a snapshot, restore before connection rip-up on failure, bounded alternative rip sets and deferred rerouting to limit oscillation. | Add bounded escalation and rejected-state history if branch replacement alone fails. Its centre-in-plane connectivity approximation is unsuitable for CopperScript acceptance. |

The last source describes LayoutEditor's C++ router, **not** the Java Freerouting
repository. Commercial documentation establishes exposed behavior, not its
proprietary implementation. These are useful precedents rather than proof of
one universal or optimal routing algorithm.

Manual BGA practice also changes the proposed search order:

- [TI SPRAD13A](https://www.ti.com/lit/pdf/sprad13), sections 8, 11 and 12,
  reserves power/decoupling resources before lower-priority routing. Figure 11-1
  shares a nearby capacitor ground pad while keeping power/ground vias close
  together to reduce loop inductance. Its AM62 interface priority table is
  device-specific; CopperProbe's analog path remains a critical interface.
- [TI SPRAD61A](https://www.ti.com/lit/pdf/sprad61), section 11.4.1 and figures
  11-21 through 11-23, shows inner-layer power distribution connected through
  BGA fanout vias to local bottom-side decoupling. This supports coordinated
  plane access and local contacts, not long serial traces linking every load.
  Its layer count, widths and pin assignments are not FX10 design rules.
- [Infineon AN238422](https://www.infineon.com/assets/row/public/documents/24/42/infineon-hardware-design-guidelines-and-schematic-checklist-for-ez-usb-fx5-fx5n-fx10-fx20-applicationnotes-en.pdf),
  section 3.2, calls for nearby decouplers and short power/ground connections to
  solid planes. Figure 2 depicts the FX20 kit; exact FX10 escape comparisons
  must still use the aligned KIT_FX10_FMC_001 native layout recorded above.

The rendered manufacturer pages and downloaded source hashes are recorded in
`build/router-research-20261018/{manifest.json,manifest-supplement.json,views/}`.
These are local research artifacts, not a release evidence bundle. No downloaded
router code was executed or copied into the implementation.

**Revised implementation order and acceptance:**

1. **Extract physical branches without changing copper.** Build a digest-bound
   graph with exact track/via occurrence identities and contacts, including
   same-layer interior T-junctions. A crossing on different layers is not a
   connection. Retain forks, pads, plated layer transitions, protected ports,
   width changes and proven filled-area contacts as boundaries. Ambiguous or
   unsupported contacts remain held. Report both the proposed edit and every
   original obligation affected by it; do not equate shared ownership with
   authority to remove the whole shared tree.
2. **Implement bounded replacement between retained contacts.** Begin with one
   generated branch and held junctions/vias; reserve the blocked power corridor
   while finding an alternative. Where possible, find replacement copper before
   removing the old branch. If old geometry must be removed for search, do so
   only on the private trial board. Commit replacement, selective removal and
   updated ownership atomically after validation. An existing same-net via or
   rail is a target only through a real physical connection, not its net name.
   Stop when exact affected associations cannot be re-established. Do not relax
   `CopperTrial.matches()` to accept a partial full-owner release; give partial
   replacement its own explicit transaction contract.
3. **Protect electrical intent and revalidate the complete affected scene.**
   Preserve source/fixed/macro copper, verified signal exits, required via-array
   multiplicity, pin-associated decoupling and all original connections. Keep
   power/ground parallel paths unless a declared constraint permits their
   replacement. Reject source-constraint regressions; record local lead length,
   via count and supply/return geometry as engineering review inputs, not
   substitutes for impedance or PDN analysis. Refill native copper after changed
   inputs and prove actual connectivity, clearances and area contacts afresh.
4. **Escalate only from an evidenced failure.** If branch replacement has no
   candidate, jointly reconsider the conflicting provisional area alternatives
   and contact choices before freezing them. Use bounded states, displacement
   costs and rejected-state history. A reusable bounded shove operation is a
   later escalation, not a prerequisite for the first implementation. Fixed
   components and immutable USB copper remain held throughout.
5. **Validate on the small saved scene before full-board work.** Test a shared
   trunk with a movable branch, an interior T, plated-layer contacts, a ground
   mesh, protected geometry, array multiplicity, stale digests, changed fill,
   exhausted bounds and exact rollback. Then replay FX_USB18 with the seven
   saved neighboring area candidates. Preserve all 112 returns, 28 ordinary
   ports, 15 critical nets and prior pad groups. Success requires a complete
   freshly checked FX_USB18 distribution while preserving those neighbors;
   freeing one sampled path is insufficient. Then re-evaluate all eleven
   domains and 63 selected bypasses before M4, M5 and full routing.

The concrete obstruction remains the saved shared-ground case: a candidate
path directly encounters 18 occurrences, whereas full-owner closure expands
to 362 occurrences and 76 obligations. Eighteen is a sampled obstruction set,
not a proven minimum cut. The new graph must establish an actually replaceable
branch; it must not assume that the observed set is safe to release. Likewise,
the accepted local L9 continuity experiment does not establish that its longer
ground lead matches the reference electrically.

Power/ground area planning remains a separate coordinated placement/routing
stage: reserve critical interface and return corridors, choose local support
patterns and plane access together, select power areas jointly, validate filled
connectivity, then route lower-priority signals. Critical routing and PDN review
must see the resulting power/ground geometry. Ordinary unfinished-end admission
(M4), refreshed physical-port guides (M5), and complete routing (M6) remain
separate unfinished milestones. No routing bounds have been increased here.

#### Implemented bounded branch replacement (2026-10-18)

`copper_branches.py` implements the first two structural steps above and
`power_contact_feedback.py` integrates their bounded replacement search into
the existing opt-in area/contact feedback stage. The complete-owner transaction
is unchanged. The small transaction retains shared trunks, source/fixed copper,
all vias, actual forks, pad/annulus contacts and width changes, while adjacent
selected degree-two corners can move. Exact contact predicates cover interior
T-junctions and off-centre capacitor entries. Search and capture recognize the
finite copper around a retained junction without allowing a walk along a shared
trunk. Unsupported overlaps, closed loops and unknown contact contracts decline.

Removal authority is still limited to 64 generated track occurrences. Validation
can cover up to 128 affected original contact/bypass obligations independently
of the 12-obligation complete-owner regenerator. Every original use identity,
terminal set and dependency survives ownership rebasing. A rejected search,
capture, physical check or association proof returns the exact original state.
The controller retains its 24-root, 16000-state and joint-trial bounds. A finer
computational pitch for narrow leads spends that same allowance. Successful
geometry still needs fresh native preservation and complete area replanning.

The final CopperProbe subset replay holds the same seven neighboring power
areas and every via. Its local cuts contain five tracks and three affected
obligations; these are different candidate paths from the earlier 18-object
diagnostic, not a smaller proved cut for that same path. The finite search
still produces no accepted replacement. The failing FX_V33 branch enters
`C_FX_L10.1` at (82.500,15.400) mm and reaches a retained junction at
(83.700,15.100) mm on B.Cu. Both anchor caps clear; the 0.05 mm local search cannot
complete the detour under the held geometry. This is a bounded unresolved
result, not a geometric impossibility proof. No consumer replacement passed
the geometry gate, so no new consumer native acceptance or board adoption is
claimed.

The new aligned comparison reads the actual kit's native filled polygons,
not just zone outlines. Reference C68.2 at (82.3517,14.939) mm lies in a BOTTOM
V3P3_FX filled area; the L10 escape uses the outward via at
(85.3328,14.9172) mm. CopperProbe instead has an explicit narrow feed across
its bottom support bank, and the proposed USB18 join crosses that feed's
route. Merely widening that feed into a pour would occupy the proposed USB18
corridor too. Reproducing the reference structure therefore calls for joint
contact/area alternatives, potentially including a separately authorized local
via-star replacement, rather than increasing full-owner release limits. Kit
CYUSB4014 and consumer CYUSB4013 configurations differ; capacitor-value and
PDN equivalence remain unresolved. The aligned drawing, raw native reference
fill, complete replay, engine hashes and tests are retained in
`CopperProbe/verification/branch-replacement-20261018`.

Steps 1–3 now have a bounded track-only implementation and scoped validation;
step 4's broader area/contact negotiation and via-star work, the CopperProbe
success gate in step 5, and M4–M6 remain open. The separate power/ground
area-planning stage remains required and implemented; expanded consumer
acceptance is still unfinished.

#### USB18 contact and area implementation sequence (2026-10-18)

This follows the measured comparison in the consumer's
`verification/usb18-route-comparison-20261018`. The native FX10 kit supplies
seven short outward TOP escapes to vias, local BOTTOM capacitor groups, and
BOTTOM/PWR1 distribution. The saved consumer contact scene has no accepted
USB18 tracks, vias or areas. Its rejected BOTTOM join intrudes into the V33
support bank. A provisional join is not an existing connection. The kit has
eight capacitor supply pads on this rail and the consumer has fourteen;
CYUSB4014/CYUSB4013 configuration equivalence is unresolved. Preserve every
consumer source association and value.

The immediate algorithmic limitation is that `plan_power_areas` caches one
greedy contact scene per layer. Its tree alternatives and joint area selector
therefore cannot choose among different contact arrangements on that layer.
The first change will expose complete, bounded contact alternatives to that
existing selector. It will not release any accepted copper.

Implementation and test order:

1. **Record and replay the baseline.** Use the saved contact checkpoint, actual
   ordinary reservations, source layer rules, seven provisional neighboring
   areas and native manufacturer geometry. Record digests, options and work
   counters. Keep the frozen-prefix case as a control; the eventual joint test
   must start before those provisional areas were appended. Measure each
   USB18 ball's contact, capacitor association, layer transitions and blocked
   terminal separately. Do not substitute the older accepted incumbent for
   this checkpoint.
2. **Add finite contact arrangements to the area proposer.** Reuse the ordinary
   plane-contact geometry proposer and its actual annulus/drill/path checks.
   Enumerate contacts for missing explicit copper components, select compatible
   choices with the existing most-constrained-first/forward-checking solver,
   and retain several complete arrangements. Derive ordering from package and
   land geometry; preserve a bounded set of inward/outward alternatives instead
   of hard-coding a package name or coordinate. Count site queries, assignment
   states and compatibility queries cumulatively per layer. Generate area
   trees from distinct arrangements within the existing candidate allowance.
   Carry the arrangement identity through reports and conflict-regrowth seeds.
   The default retains existing behavior until the opt-in path is validated.
3. **Test the new transaction contract before consumer replay.** Synthetic
   cases must demonstrate (a) independently legal contacts that conflict when
   combined, (b) a second arrangement permitting an area route that the first
   blocks, (c) exact reuse of existing plated contacts, (d) protected macros,
   required via arrays and forbidden layers, (e) stale inputs, bounded work and
   exact rejection rollback, and (f) deterministic results under repeat runs.
   Native refill must verify a positive synthetic case and reject an injected
   failed assignment without adopting its copper. Existing area/contact,
   branch, preparation and native-alternative regressions must remain green.
4. **Replay the small USB18/V33 scene and correct candidate coverage.** First
   report contact pools against the frozen-prefix control, then generate complete
   USB18 candidates. Align every failing ball against the kit's actual TOP,
   BOTTOM and PWR1 geometry. If short outward sites are absent, improve generic
   site generation from pitch, pads and obstacle boundaries. If present but
   blocked, report the exact obstacle and ownership. Missing candidate coverage
   must not be disguised by increasing maze bounds or moving fixed components.
5. **Negotiate provisional areas with the contact arrangements.** Extend the
   existing area conflict-regrowth stage to revisit the conflicting neighboring
   alternatives and contact arrangement, using only candidates tied to the same
   original source. Start from the pre-area checkpoint, retain critical/ordinary
   reservations, and jointly reallocate the whole affected set. A failed domain
   with no initial area candidate must be eligible for bounded contact/area
   regeneration; the current all-domains-have-candidates condition needs an
   explicit replacement. Rejected combinations and cumulative budgets survive
   retries. A generated via may move only in a separate exact transaction that
   reconstructs all affected obligations; that operation is not authorized by
   the existing track-only branch transaction.
6. **Native and integration acceptance.** Require actual filled connectivity
   from the declared source/feed to all seven USB18 balls and all required rail
   terminals, freshly verified capacitor supply/return associations, no new hard
   DRC errors, and preservation of prior pad groups, returns, critical nets,
   ordinary ports and required arrays. Save before/after/reference drawings plus
   all source/tool hashes in the consumer. Reject the entire proposal if any
   gate fails. Then test all eleven power domains through standard `route-board`
   preparation; only afterward proceed to unfinished-end admission and full
   routing. Native connectivity does not establish high-frequency PDN quality.

First deliverable: the opt-in contact-alternative implementation, focused
regressions, and measured small-scene replay. Completion of that deliverable
does not close steps 4–6 or the CopperProbe routing milestone. Local component
movement remains limited to source-permitted poses; a fixed support blocking
the reference structure is an explicit source-layout finding, not permission
for the generic router to move it.

First implementation result: the opt-in `maximum_contact_alternatives` path now
uses finite ordinary contact pools, exact pairwise contact conflicts and the
shared assignment kernel. It retains several assignments per layer and feeds
them into the existing bounded area candidates. Local capacitor components
may defer a separate via only when bounded layer bridging is enabled. Native
rollback testing caught and corrected that policy condition. The final affected
suite passes 71 tests without skips; the broader earlier suite passes 144
without skips (overlapping, not summed). Synthetic cases prove second-contact
area recovery, conflicting-drill avoidance, capacitor contact sharing, required
array preservation, exact rollback, ordinary plane antipads and native refill.

The final fast consumer contact-pool probes take 9.064 seconds before the seven
provisional areas and 18.408 seconds with them held, on the recorded host. The
former has no compatible assignment: L5 and M4 each have one generated contact,
whose drill centres are only 0.125 mm apart. The latter has two provisional
assignments but no direct contact candidate for L5 or M2. These are contact-pool
diagnostics, not area-tree or native acceptance. An earlier full bounded area
trial with local bridging also remained unresolved; its inputs are retained
separately. No CopperProbe geometry has been adopted.

Testing all seven aligned kit via positions against the actual consumer
0.45 mm / 0.20 mm through-via and held reservation scene finds every position
blocked. L3/L5 overlap backside supply lands and protected regions; L7 overlaps
a return land and existing copper; M2/M4/M6 encounter existing tracks/vias;
M8 overlaps a backside supply land. Thus choosing another provisional inner
area alone is insufficient. The next implementation needs source-permitted
support/contact pattern co-planning and exact regeneration of the conflicting
generated contacts, followed by step 5's joint area reconsideration. Existing
protected regions and fixed poses remain constraints. Missing reference-like
sites cannot be repaired by treating a copper fill outline as empty drilling
space or by copying the kit's smaller via. These measurements do not prove
geometric infeasibility for other legal contact/placement alternatives.

Steps 1–3 have this first bounded implementation and scoped evidence. Steps
4–6 remain open; the retained comparison and replay inputs are in the consumer's
`verification/contact-arrangements-20261018`. Full-board routing and electrical
qualification remain unfinished.

#### Coordinated implementation follow-through (2026-10-18)

The support-pattern stage now evaluates complete placement assignments through
the normal power preparation, including critical routes, ordinary reservations,
contacts, returns, area alternatives and native refill. The standard routing
pipeline forwards its global/fanout/pair-budget settings. Source poses are
selected only after a complete native-backed trial and a fresh final rebuild;
rejection returns the original poses. Whole-bank preferences vary together
within the existing check budget. This mode starts before generated contact
ownership freezes; externally supplied routed reservations are explicitly
unsupported, not silently released. See the detailed
[combined-stage contract](placement-internals.md#joint-support-contact-and-power-area-trials).

The allocator's previous all-domains-have-candidates guard has also been
removed. A source-valid missing domain can use a bounded provisional-neighbor
context to regenerate contacts/areas and then participate in complete
same-source allocation. Unknown boundaries remain ineligible. Existing trial
and state allowances are shared across retries; no provisional neighbor is
accepted as a partial solution.

Missing-domain retries now skip zero-addition neighbors, deduplicate equivalent
obstacle geometry and share their existing trial allowance across missing
domains. Generated contact arrangements are ranked by their actual layer
access after local bridging, so a failed deferred connection does not consume
the only tree slot while a complete direct-via alternative is available.
Original contact-pool identities remain available for seeded regeneration.
These changes neither expand work limits nor weaken native acceptance.

On the same complete contact scene, that ordering recovers two D18, three USB18
and three FX_V33 candidates without changing poses. Fresh scoped native checks
pass D18 and FX_V33; the first selected USB18 candidate exposes a newly proposed
via used only as a bottom-layer annular junction. Its native `via_dangling`
finding remains a rejection, even though the rail and returns are connected.

Prospective area contacts now receive a finite planar-junction normalization
pass. At most `maximum_new_contacts` newly proposed vias are visited per
candidate. Only track contacts on one actual layer are eligible; source vias,
other-layer contacts, ambiguous geometry and possible other-layer source-zone
access remain held. Exact integral projections produce short checked surface
connections, and every conductor joined by the old annulus must remain joined.
Unsupported or blocked replacements retain the original proposal. The pass is
also applied when revalidating an unaccepted same-source candidate pool, followed
by fresh conflict allocation and native acceptance; it cannot clean up an
accepted ownership ledger or replenish its allocator/regrowth allowances.

This correction passes a real native annulus-junction regression. The three
consumer scopes then each pass fresh native supply/return checks with zero
other violations and zero islands. Their separate whole-board open counts are
251 (D18), 239 (USB18) and 256 (FX_V33), against the same 261-open baseline.
These are alternative scoped scenes, not a combined eleven-domain board.

The next contact alternative now uses failed bridge components as preferences:
retain the previous assignment for successful members and prefer a real direct
contact for each unresolved deferred member that has one. Exact finite
compatibility may still adjust conflicting neighbors. When there is no such
repairable member, the existing whole-bank preference variation remains.
Bridge evaluation happens once per returned arrangement, within the existing
alternative, assignment and pair-work limits; this is not an extra unreported
repair pass. The current USB18 full-context subset produces two candidates
with six proposed vias each instead of the earlier selected candidate's fifteen.
The selected candidate passes fresh native supply/return checks with 239 total
opens, zero other findings and zero islands, and all 81 affected tests pass
without failures or skips. The reference has seven supply-ball transitions in
its different component configuration; fewer vias are not a PDN qualification.

The ranking-only eleven-domain control finished unresolved after 1,788.100
seconds: 38 candidates, 106 allocation states and all twelve conflict trials.
Its one track-branch feedback attempt also failed. The targeted-contact control
also remained unresolved: 39 candidates, 39 allocation states and twelve
conflict trials in 828.549 seconds. It repeatedly regenerated CORE11 around
USB18 without trying the reverse direction. All three CORE11 candidates put a
prospective via at (83.125, 15.000) mm, conflicting with the USB18 capacitor
contact at (83.125, 14.840) mm. These are disposable area proposals, not authority
to release source copper. The aligned manufacturer board separates these local
contact groups and uses short supply-ball transitions.

Conflict trials now share their existing allowance across regeneration owners,
so an unavoidable pair gets both repair directions before one owner consumes
the pool. A two-trial negative regression requires both directions and exact
rollback. All 82 affected tests pass without failures, errors or skips. On the
unchanged complete contact scene, the CORE11/USB18 coupled subset resolves on
the second conflict trial, using twelve allocation states and six candidates.
Fresh native validation confirms both supplies and their returns: 238 total
board opens, zero other findings and zero islands, against 261/0/0 at baseline.
Other nine domain areas are not selected in this subset. This is not complete
eleven-domain preparation or ordinary routing; the next full integration keeps
the same source, reservations and work limits.

The eleven-domain bidirectional control remains unresolved after 835.527
seconds, 39 candidates, 39 allocation states and twelve conflict trials. A
wider allocator prefix (including an unrelated earlier domain) repeatedly
selected the same blocked owner and overrode pairwise direction fairness.
The remaining unavoidable candidate-pool pairs are CORE11/USB18 and
FX_XTAL/V33. The latter places proposed through-vias only 0.1008 mm apart at
the two lands of the crystal-supply ferrite; layer choice cannot remove that
physical conflict.

The complete native manufacturer layout identifies the corresponding FB6
outside the earlier package crop. It is on the bottom layer with separate
upstream and filtered-supply copper regions and spatially separate transitions.
The consumer FB_XTAL is on the top layer. The comparison is component-centred
and explicitly identifies the surface difference; imported reference outlines
are not treated as fresh native fill or electrical qualification.

Unavoidable domain-pair conflicts now take precedence over wider-prefix
regeneration: every complete assignment must first resolve those pairs.
Wider-prefix trials remain available after each domain pair has at least one
compatible alternative. The existing two-direction regression now includes an
unrelated third domain; it fails before this correction and passes afterward.
All 31 focused geometry and missing-domain tests pass, including the existing
multi-owner positive cases. The full affected suite also passes all 83 tests,
without failures, errors or skips. A fresh eleven-domain control retains the
same source and configured bounds.

The final conflict-priority integration finishes unresolved after 1,070.308
seconds: 44 candidates, 366 allocator states and all twelve conflict trials.
It reaches the reverse USB18/CORE11 repair, resolves FX_XTAL/V33, and successfully
generates several wider-context proposals. No unavoidable domain pair remains,
but no complete eleven-domain assignment exists in the generated pool. The
standard dependency-feedback attempt does not produce an accepted replacement.
No current canonical board is adopted and ordinary detailed routing does not
start.

A read-only reduction of that exact candidate pool identifies the minimal
conflicting set USB18 / FX_V33 / FX_VDDD: each two-domain subset has a compatible
assignment, while the three-domain set does not. The diagnostic uses 785 states
within a cap equal to the unspent allocator allowance; it generates no geometry,
adds no conflict trials and retains the original routing-run counters. This is
a finite candidate-pool limitation, not a proof of physical infeasibility.
In a representative compatible USB18/V33 choice, the USB18 via at
(83.125, 14.215) mm blocks a VDDD In2.Cu branch at y = 14.250 mm. Two VDDD feed
vias at (78.750, 27.725) and (80.750, 27.725) mm conflict with the selected V33
inner-layer bus. All alternatives and exact blockers are retained with the
aligned manufacturer comparison in the consumer evidence bundle.

This is the current stopping point. The next small acceptance case must produce
one jointly compatible contact/distribution choice for these three domains on
the unchanged complete contact scene, followed by fresh native supply/return
acceptance. A conflict-directed selection of the responsible domain set, rather
than repeated unrelated wider-prefix repairs, is a candidate next algorithmic
increment; it is not implemented or validated by this work. Only after that
three-domain gate should the eleven-domain run be repeated. M4 continuation
admission, M5 physical-port guide handoff and M6 full routing/native closure
remain unfinished.

The next consumer experiment starts from the complete unrouted support-bank
source and tests USB18 with all 166 parts, 91 nets and 69 associations retained.
An exact source-rule audit finds two movable USB18 parts, `C_FX10_L7` and
`C_FX_L5`, each retaining its authored bottom-side region and fixed rotation.
The other twelve USB18 bypass capacitors are fixed. The E3/C7 freedoms from the
earlier support experiment do not free those twelve parts. These constraints
remain binding during the trial. A successful isolated USB18 result must still
be followed by all eleven domains, ordinary admission and full-board routing;
it cannot qualify the complete board or its high-frequency power network.

Measured result: the baseline plus three legal joint placement assignments each
retained all eight critical groups and 78 ordinary ports but produced zero
complete USB18 area candidates. The 1,003.374-second run adopted no placement.
L3 had no direct contact candidate; the sole L5/M4 new-via choices conflicted.
The fixed L3 capacitor, its generated return connection and an immutable USB
escape region obstruct the aligned manufacturer's via position. This was not
treated as proof of geometric impossibility.

A subsequent generic contact-pool correction permits a package terminal to
defer a colliding new via and reach another plated component through the
existing bounded layer bridge. Its own contact remains preferred. Existing
vias and required arrays are never removed. The same frozen USB18 contact
checkpoint then produced three complete candidates and passed native
preparation in 185.409 seconds, without moving a component: five new USB18 vias,
an In3.Cu distribution area, all 14 capacitor supply/return associations,
43 required returns and all 78 ordinary ports. Native opens changed from 379
to 357; other violations stayed zero and the two pre-existing islands were
unchanged. These whole-board counts include unrelated unfinished nets.

The coordinated-stage regression run passes 108 tests without failures or skips.
After the sharing correction, the 70 affected contact/area/native/preparation
tests pass without failures or skips; these suites overlap and are not summed.
The same-face sharing test fixture initially exposed an unused dangling via;
native validation rejected it. The positive sharing fixture exercises a real
transition between opposite faces and passes native DRC with one shared via.
The original placement trials and later contact replay retain separate engine
snapshots. An eleven-domain integration replay is now required before ordinary
admission and full-board routing.

The first generic implementation is now available through the opt-in
`support_pattern_trials` placement stage; its implemented bounds and limits are
in [placement contracts](placement-internals.md#joint-support-bank-patterns).
The research design below also records further electrical and consumer gates;
it does not claim a new CopperProbe acceptance result. Use parameterized component/contact
patterns together with joint local selection. The unit being optimized should
include component poses, the corresponding supply and return contacts, and
reserved signal exits. Packing component bodies independently leaves the
important routing area unaccounted for.

#### Evidence and applicability

- [Infineon AN238422](https://www.infineon.com/assets/row/public/documents/24/42/infineon-hardware-design-guidelines-and-schematic-checklist-for-ez-usb-fx5-fx5n-fx10-fx20-applicationnotes-en.pdf),
  002-38422 Rev. **, 2025-03-13, section 3.2, page 6, Figure 2: close local
  decoupling and short supply/ground connections; its bottom-side illustration
  combines perimeter banks and interior placements. The figure is an FX20 kit,
  not a replacement for the already aligned native FX10 kit. Use the actual
  selected-package layout for a specific fault and source pin-associated
  requirements for electrical assignment.
- [TI SPRAD13A](https://www.ti.com/lit/an/sprad13a/sprad13a.pdf), revised December
  2024, sections 6, 8 and 11: floorplan by interface requirements, allocate power
  resources before lower-priority routing, and minimize local power/ground loop
  inductance. Figure 11-1 discusses nearby power/ground vias and capacitor-ground
  sharing; under-SoC placement can improve particular supply connections. Its
  AM62 VCA ball-map and via-sharing exceptions must not be copied into FX10.
- [Analog Devices, Frederik Dostal, 2019-11-01](https://www.analog.com/en/resources/technical-articles/bypass-capacitor-and-coupling-capacitor-stabilizing-voltage-the-right-way.html),
  Figures 2 and 3: connection geometry, loop area and via proximity matter;
  different pad/via patterns have different parasitics and manufacturing
  applicability. This is a regulator example, so its preference for same-side
  input bypassing is not a universal rule for BGA decoupling.
- [Cadence placement replication](https://community.cadence.com/cadence_blogs_8/b/pcb/posts/place-replicate-to-increase-ip-reuse-and-decrease-design-time),
  2019-04-15: industry reuse groups relative component placement together with
  routes, vias and keepouts. Connectivity and layer mapping matter when reusing
  a block. This supports the reusable-unit representation, not proof that a
  copied block remains qualified under different footprints, stackups or rules.
- [Tsou et al., DAC 2024](https://scholars.lib.ntu.edu.tw/entities/publication/60e2c5ce-d0c5-40b4-86ae-d9d76c49cf1a),
  DOI 10.1145/3649329.3663495; expanded
  [TCAD publication](https://ieeexplore.ieee.org/abstract/document/11195800),
  DOI 10.1109/TCAD.2025.3618936: pad alignment, power-flow anchors and constrained
  legalization are relevant PCB placement techniques. Only the primary
  abstracts were available for this review; no detailed algorithm or claimed
  performance gain is assumed reproduced here.
- [NS-Place, Cheng, Ho and Holtz, 2022](https://arxiv.org/abs/2210.14259):
  combines wirelength/density objectives with net separation and local
  legalization, and evaluates placements after routing. This supports judging
  usable routing space instead of relying on short airwires or a compact image.

The synthesis below is our engineering proposal based on these sources. It is
not a claim that any single cited paper implements this exact hybrid.

#### Finite pattern families

| Family | Relative arrangement | Applicability and protected resources |
| --- | --- | --- |
| End-on edge cell | Supply land faces the relevant pin/exit; body extends along an available local direction | Useful beside a package edge; reserve the ground landing and both connecting paths as well as the body |
| Paired bank | Two source-associated capacitors placed side by side with aligned approach ends | Useful for a declared two-capacitor pin association; preserve both component identities and values rather than treating any same-rail capacitor as interchangeable |
| Staggered bank | Adjacent cells alternate tangential offset or row depth | Useful where one straight row would block via sites or signal lanes; extra depth must remain within local electrical constraints |
| Underside pocket | A capacitor/contact cell fits among actual escape vias below the package | Useful for inner supply balls and short plane access; check all through-drill obstacles, permitted via spans and actual stackup distance |
| Split or corner bank | A small group occupies two legal edge segments around a reserved channel | Useful when a continuous row blocks an exit; retain short supply/return access and source pin associations across the split |

These are candidate families, not mandatory placement rules. In particular,
neither "all capacitors outside the BGA" nor "always point the short edge at
the package" is generally sufficient. The best orientation depends on the
specific supply/return lands, available transitions, layer roles and protected
signal channels. Shared return copper or contacts need exact ownership and
fresh proof; patterns cannot reduce source-required via-array multiplicity.

#### Representation and bounded selection

1. Build groups from explicit source associations. Preserve the target supply
   pad, capacitor supply/return pads, voltage-domain boundaries, permitted return
   targets, component values/footprints, allowed sides/rotations and fixed/rigid
   membership. Same voltage, net-name similarity or physical proximity alone
   cannot establish electrical equivalence or permission to swap capacitors.
2. Derive local frames from actual lands, legal exit directions, package edges
   and free via pockets. Use normal/tangent coordinates relative to these
   features, not board reference designators or fixed XY offsets. Pitch, bank
   spacing and offsets derive from transformed courtyards, fabrication rules,
   via/drill envelopes, mask/assembly constraints and available channels.
3. Generate a finite number of complete pattern instances. Each carries its
   source digest, component poses, parameter values, local tracks/vias,
   terminal/path witnesses, layer-specific resource envelopes and exact owner
   dependencies. Rotate/mirror geometry only with correct pad/net identity.
   A pattern is a disposable alternative; it does not become an immutable
   audited macro merely because it resembles a reference layout.
4. Select small overlapping groups together. A conflict graph records body,
   drill, copper and corridor incompatibilities; source associations add
   coverage/shared-contact constraints. Use the existing most-constrained-first,
   forward-checking selection strategy with bounded backtracking. Pose changes
   require an extended placement bundle: current `AccessBundle` contains only
   copper and ports and cannot authorize moving components by itself. Existing
   copper dependencies retain their exact release closure and rollback rules.
5. Preserve a small Pareto frontier. Source legality, immutable intent, all
   already verified obligations and explicit electrical bounds are hard gates.
   Among admissible alternatives, rank new complete associations/exit access,
   model-backed local loop quality where available, routing congestion, and
   occupied support envelope including contact/corridor space. Use actual
   route length, transitions and detour as proxies when no valid loop model
   exists, and label those proxies. Individual body-area sum is constant for
   unchanged parts; minimize bank envelope/fragmentation rather than that sum.
   Compactness must not compensate for a lost supply, return or signal exit.
6. Run exact local regeneration for finalists in the complete physical scene.
   Individually legal cells may conflict jointly or lose access to the power
   area. Validate selected combinations, then normal package/contact/area
   preparation and fresh native connectivity. Bound all generation, selection,
   local path and native checks cumulatively; cached geometry never supplies
   acceptance under a changed source or placement digest.

#### Integration with current placement and power planning

The existing `improve_decoupling_placement` proposes pad-relative poses,
checks actual bypass geometry, protects previously verified associations and
respects fixed/rigid parts. Its search is sequential per capacitor and checks at
most 64 locally legal poses per capacitor; the new optional `support_patterns`
stage follows it to instantiate and select multi-capacitor banks jointly.
Legacy end-on ranking remains a same-face tie-break
after distance, not a general bottom-bank/contact arrangement. Rigid clusters
preserve authored relative poses but do not synthesize a different bank pattern.

Place the new group search during mutable placement refinement, before the
power/contact prefix is accepted and frozen. Derive or refresh candidate signal
escape corridors and local power/ground access together on private snapshots;
no speculative signal path has native acceptance. Existing critical routes and
required exits remain protected. After selection, regenerate the normal bounded
preparation from source at the selected poses, plan power/ground copper areas,
then perform global/detailed routing. A bounded placement-feedback retry can
return to this stage, but cannot reposition an already accepted/frozen prefix.

Generic pattern generation/selection belongs in CopperScript. Audited
package/circuit-specific templates belong in independently versioned library
contracts; the consumer selects its source constraints and keeps reference and
validation evidence. Do not add definitions to CopperLib for this work. Analog
feedback/RF and switched-power cells retain their own source-backed contracts
or hard macros; a generic digital-decoupling pattern does not qualify them.

#### Fast implementation and validation sequence

1. Add a source-bound pattern-instance representation and edge/pair/pocket
   generators, with independent renamed/rotated/mirrored synthetic fixtures.
   Include a case where one greedy capacitor occupies the only usable site of
   another, so a joint group move is necessary.
2. Add finite joint placement selection and fresh local supply/return/escape
   validation. Test a smaller body envelope with an unroutable ground contact,
   a valid underside pocket that defeats an outside-only policy, side-specific
   assembly limits, through-drill interference, shared contact ownership,
   unequal capacitor roles and complete rollback. Verify that a source-fixed
   bank is diagnosed rather than moved.
3. Compare applicable families on the E3/C7 neighbourhood in the frozen
   CopperProbe context. Start with the already aligned manufacturer geometry.
   Use an explicitly mutable consumer candidate only if its authored placement
   intent permits the required group moves; do not silently unlock the frozen
   bank. Retain the six data escapes and deep control exits, previous verified
   bypasses and the whole declared native return ledger, not just the moved
   capacitor count. Include access onward to D18 distribution in the subset
   gate; a legal dogbone alone is not sufficient.
4. Expand only after the subset passes to all eleven area families and fresh
   native preservation, then the existing full routing gates. Record trial
   budgets, source/engine hashes, pattern parameters, exact reference delta,
   unresolved model gaps and before/after occupied envelope, route lengths,
   transitions, pending associations and exit coverage. Geometry/native success
   does not establish high-frequency PDN or analog-front-end qualification.

**M1 — Capture contact ownership and dependency closure.** Extend
`PowerPreparation` and the contact/decoupling results to record which exact
generated track/via occurrences serve which pad, bypass association, return
contact and distribution feed. Record shared uses explicitly. Source geometry,
macros, required arrays and accepted critical copper retain fixed ownership.
Do not infer owners from a net name or remove an entire net because one segment
blocks a proposed path. Test identical copper occurrences, shared contacts,
renamed nets, protected owners and complete rollback.

Deliverable: given a sampled blocker such as `track:1319`, the new preparation
can identify the generating obligation and every dependent obligation. This is
necessary before any contact becomes negotiable.

**M2 — Propose alternatives on a private preparation and select bundles.** Add
a generic `AccessBundle` contract and a bounded joint-access selector (proposed
module: `joint_access.py`). A bundle contains its covered terminals, owner,
source digest, actual tracks, all vias, layer-specific ports and required local
associations. Extract bounded alternative-producing interfaces from fanout,
`plane._stitch_land` and the existing decoupling-path proposers, retaining their
exact geometry/process and local association checks. Supply and return paths
belong to the same dependency group where the existing contract couples them.

For an affected cluster, reconstruct a private preparation from the unchanged
source and critical prefix. Retain outside owners exactly. Omit only the
generated occurrences in the negotiable dependency closure, generate signal
and contact alternatives against that held scene, and choose them together.
Reuse the existing conflict checks and minimum-remaining-values/forward-checking
mechanism, generalized for multi-via bundles and coverage obligations. Require
all affected obligations, not merely the previously missing signal. Shared vias
consume one physical resource and required arrays retain their multiplicity.
Search, pair-query, regeneration and native-trial work have explicit cumulative
limits; no retry replenishes them.

Deliverable: a synthetic coupled-contact fixture where the sequential choice
blocks a signal and joint selection preserves both contacts and the signal;
negative fixtures reject protected-owner displacement, lost returns, incomplete
arrays, incompatible multi-via geometry and exhausted work limits.

This is the physical change missing today: an empty signal candidate domain
can gain a legal candidate when the obstructing *trial-generated* contact is
given another legal arrangement. Reordering the existing seven E12 choices
or adding more sites against the unchanged held contacts cannot provide that.
The accepted eleven-domain board remains the incumbent throughout.

**M3 — Demonstrate physical recovery on the consumer subset.** First use the
E12/D12/D13/E11 neighborhood and its actual contact dependencies, then B2 as a
zero-candidate data-pad case. Keep the full board and all outside owners present.
Integrate joint selection into the disposable portion of
`prepare_power_distribution` before distribution-area growth and final freezing.
Use the existing fifteen-net replay as a before/after control. Native-check
each complete proposed combination: all eleven domains, 63 bypass associations,
112 returns, every old pad group and the critical prefix must survive. Ordinary
unfinished ends remain reported and the current strict gate still rejects a
partial ordinary board until M4 is implemented.

Deliverable: at least one currently pending FX10 exit becomes legal and survives
fresh native preservation, giving twelve to eleven or fewer pending exits in
the coupled replay. Report finite-pool failure if this does not happen. The buck
converter's VBUS pad receives its own source-backed contact/placement case with
the converter's physical constraints; do not apply a BGA-specific workaround.

**M4 — Add an explicit intermediate continuation ledger.** Record every newly
owned ordinary free endpoint or unused via-layer connection with its verified
pad path, allowed continuation layer/span and board digest. Extend native
mapping if necessary to retain the actual reported endpoint, rather than only
the containing object's two endpoints. Individually match native findings to
these obligations. Keep raw findings and counts intact and report a distinct
partial-stage result. Selected power/return opens, unknown objects, unexpected
ends, protected-owner dangling items and shorts/clearance findings still reject.
The existing strict native extractor and final clean-board gate remain strict.

Deliverable: real native fixtures distinguish a declared unfinished end from a
broken source connection; completion discharges the obligation and produces a
clean final result. This milestone changes stage admission, not available space.

**M5 — Refresh global guides from committed physical ports.** Separate physical
package readiness from combined physical/global readiness in `package_access.py`.
In `flow.py`, require complete local access, reserve its boundary paths, validate
native preservation using M4, then refresh global routing. Add a digest-bound
prepared-access input to `route_global`, reusing `RoutingAccess` verification.
Use actual port layers and checked port-to-grid lead-ins. Register each real
plated transition once in `global_terminals.py`, with shared capacity across
aliases. Reject stale ports, missing path legs and unsupported connectivity.

Deliverable: a fixture whose raw-pad coarse access fails but whose verified port
routes successfully, plus stale/wrong-layer/duplicate-capacity negative controls.
Re-evaluate ADC_D7, ADC_OVR and DFS and all seven overflow resources on the same
current board. Physical B2 recovery remains a prerequisite for its guide claim.

**M6 — Expand and perform one normal bounded routing comparison.** Recover every
required ordinary exit in the complete-context replay, preserve all local,
power, return, critical and physical-intent gates, and require fresh global
guides before ordinary detailed routing. Then run the standard `route-board`
pipeline once with its declared bounds. Every continuation must close and final
native DRC must satisfy the existing strict requirements. A remaining global,
native or physical failure is retained as a failure with its exact owner and
work evidence. If fixed source geometry blocks the bounded alternatives,
produce a concrete source-backed placement/macro revision proposal; neither
silent constraint relaxation nor repeated budget increases is the fallback.

The checkpoints are M3's first recovered exit, M6's zero pending exits and fresh
global success, then final detailed/native closure. Passing unit tests, smaller
open counts or intermediate admission alone cannot substitute for them.

### Implementation order and contracts

1. **Make failure causes actionable.** Extend the shared access proposer to
   retain attempted sites, terminal/layer/span, emitted path widths, exact
   blocking object occurrence and owner, rejection predicate, sampled count,
   expanded states and stop reason. Distinguish source/macro copper, accepted
   prefix copper and currently negotiable trial copper. Preserve diagnostics
   after rollback. An empty sampled domain, an assignment conflict and an
   exhausted work allowance are different outcomes. Use the same diagnostics
   for ordinary fanout and power/return contact generation.
2. **Generate and jointly select local access bundles.** Extract the existing
   circular-cell proposer for reuse and add finitely many off-centre proposals
   from expanded obstacle boundaries/free-space gaps. These are proposals,
   followed by exact path, full through-via span, opposite-face land, drill,
   material and process checks. A bundle names its source terminals, obligation
   coverage, track occurrences, one or more actual vias, layers, boundary ports
   and negotiable owner. Permit sharing only when declared and verified;
   retain required array multiplicity and pin-associated bypass identities.
   Select one alternative per family with all required obligations covered,
   lazy exact incompatibility checks and explicit resource limits. Reuse the
   existing minimum-remaining-values/forward-checking search and allocator
   budgets where their contracts fit; do not encode signals as power domains.
   Recompute affected domains only when a recorded negotiable blocker changes,
   charging regeneration to the same finite site/path/state budget. Native
   rejection excludes the complete tested assignment unless a smaller conflict
   is independently established.
3. **Integrate before committing areas.** In a new preparation from source,
   materialize immutable macros, preserve/realize critical routes, select joint
   ordinary/supply/return access, then grow permitted power areas around the
   selected physical paths. Native-check supply joins, all required returns,
   bypasses and old pad groups together. Maintain the separate power/ground
   planning stage; inner power areas cannot create a missing plated access.
   Keep signal-reference planes continuous and analog exclusions authoritative.
   Competing trial contacts may be reselected; an already accepted prefix is
   never ripped up. A different placement or accepted power arrangement needs
   a new preparation with fresh evidence and complete rejection rollback.
4. **Represent unfinished ends before changing native admission.** Introduce
   a typed, digest-bound ledger containing net/pad, exact new object occurrences,
   verified pad-to-port path, allowed continuation layer/span and expected free
   endpoint or unused via-layer connection. Native findings remain unchanged
   and visible. Only individually mapped new ordinary ends satisfying that
   ledger can remain pending at an intermediate stage; source/protected copper,
   unknown mappings, orphan copper, unexpected endpoints, lost old pad groups,
   selected power/return opens, shorts and clearance findings reject the trial.
   Ownership alone is insufficient. Add a separate partial-stage assessment;
   do not weaken the shared extractor's strict result, set counts to zero, or
   report a native-clean board. Missing exits still block physical readiness.
5. **Hand physical access to global planning.** Add a prepared-access input to
   `route_global`, reusing `RoutingAccess` and its exact verifier. Bind the
   certificate to the current physical board, source terminals, rules, owned
   paths, layers and plated spans. After physical package access and native
   preservation succeed, atomically reserve boundary paths and build fresh
   guides from those ports. Connect each port to a legal coarse graph node by
   a checked local segment, not coordinate snapping. Reuse existing physical
   vias rather than checking a coincident synthetic foreign via. All aliases
   of one plated hole share one physical resource; several references to it
   do not add capacity. Contract only actually verified same-net components.
   Preserve separate physical, global and final readiness results. A fresh
   global failure remains a blocker; unresolved B2 access cannot be cured by
   merely substituting a terminal coordinate.
6. **Use bounded feedback and final closure.** A global rejection may return
   exact constrained ports/resources to the still-disposable preparation,
   within an explicitly bounded number of trials. Share/report cumulative
   generation, assignment, guide, native and detailed work; do not replenish
   allowances through retries. Detailed routing must discharge every admitted
   ordinary continuation, preserve every verified exit/array/prefix occurrence
   and pass fresh native checks. Final acceptance retains zero unintended
   opens, dangling items, shorts and clearance errors. Electrical/PDN, RF,
   assembly and CAM qualification remain separate.

### Small tests before another complete-board attempt

1. A conflict-only fixture derived from E12's seven alternatives, with all
   surrounding source geometry retained. Record whether the current solver
   exhausts a bound or proves the finite pool incompatible. Add a synthetic
   greedy-failure/joint-success control; do not presume the real cluster solves.
2. Zero-candidate fixtures for an FX10 data land and a control land, including
   their real backside lands and power/return contacts. Exercise a legal
   off-centre site, rotated/mirrored geometry, an all-layer blocker and a fully
   blocked negative control. Keep the buck converter VBUS land as a separate
   source-backed switched-power case; a BGA pattern or width waiver is not a
   substitute for its circuit/layout contract.
3. Two competing supplies, required return contacts and ordinary exits where
   changing a disposable contact arrangement frees a signal path. Require
   identity-preserving coverage, array multiplicity, exact native continuity,
   deterministic replay and full rollback when a held owner blocks the trial.
4. A physically valid port that cannot be reached through raw-pad coarse
   sampling. Require fresh global guides through its actual layer/path. Reject
   stale digests, wrong layers, missing path legs, duplicate via capacity and
   unsupported component contraction; retain genuine capacity failures.
5. Native partial-routing fixtures: a pad-connected declared unfinished end
   remains visibly pending, then closes cleanly. Reject an unowned dangling
   item, an old/protected owner, an unexpected free endpoint, a lost return,
   an unmapped object and a short. An incomplete escape inventory cannot pass
   merely because all its dangling objects are owned.
6. Run the coupled scene with all eleven domains, 63 bypass associations,
   112 return lands and fifteen critical nets. Require zero missing ordinary
   exits, preserved physical intent, fresh boundary/global/native evidence and
   explicit dispositions for the three guide failures and seven overflows.
   Only then run one normal bounded full `route-board` comparison. Retain the
   complete physical board context throughout; scope work, not obstacles.

The immediate deliverable is blocker/owner diagnostics and the two small
escape fixtures. The next integration deliverable is a joint disposable access
proposal plus the native continuation ledger and physical-port global handoff.
The first diagnostic and site-generation increment is implemented and measured
above. The next integration work is joint disposable access selection; native
continuation-ledger admission and physical-port global handoff remain required.
The extra site family did not solve the consumer failures, so increasing its
allowance or repeatedly rerunning the complete board is not the next action.

## Development sequence

| Milestone | Deliverable | Gate before advancing |
| --- | --- | --- |
| 0. Reproducible small baseline | Consumer-owned scenario manifests, frozen legal poses, exact input hashes and a timed scoped baseline | The selected domain retains its real source boundary, every load, bypass association and surrounding obstacles |
| 1. Planning contracts | Explicit domains, source/feed obligations, allowed layers, protected geometry, corridor demand and per-terminal status | Unknown sources remain unresolved; filtered nets remain separate; no net is deferred merely because a local zone exists |
| 2. Electrical area proposal engine | Bounded source-to-load trees, layer assignments and provisional polygons; coarse resistor-network solve in candidate evaluation; joint allocation of competing rail space | Explicit current/voltage and material inputs; DC residual checks; exact clearance checks; deterministic replay; no source or fixed geometry changes |
| 3. Package and local-loop integration | Plan contacts and pin-associated supply/return paths together with package exits | Every required local association survives; source-to-load connectivity and local-loop evidence are reported separately |
| 4. Placement feedback | Evaluate provisional areas inside placement/global-route trials and return actionable conflicts | Legal whole-unit moves only, fresh validation, preserved exits and via multiplicity, complete rollback |
| 5. Native acceptance and electrical refinement | Validate fresh filled fragments, feeder continuity and extracted DC network; refine the early model against actual copper and reference continuity | No new native shorts/clearance errors or scoped opens; current/voltage limits met where inputs exist; unknown electrical inputs remain visible |
| 6. Expanded consumer trial | All FX10 domains and USB routes, followed by an ADC/analog preservation trial | Shared copper solution preserves all required local and critical routes within measured work budgets |
| 7. Full board | One normal bounded `route-board` comparison using the accepted implementation | All earlier gates pass; no board-specific repair path or diagnostic geometry is silently adopted |

Milestones 1–5 form the first usable planner. Do not start a new full CopperProbe
route while one of those gates is unresolved. Existing pinned runs can finish
independently; their results belong to their original inputs.

## Small CopperProbe trials

Board-specific selection and evidence live in CopperProbe, not in compiler
selectors. Keep the complete physical context while limiting the work requested
from the router. This avoids deleting obstructing BGA lands or giving the test
unrealistic space. The first baseline uses current source with fixed rules
applied and a recorded legal pose seed for the remaining components. A frozen
diagnostic pose is not a new source placement lock.

1. **One filtered crystal supply domain.** Select `FX_XTAL`: the downstream
   terminal of `FB_XTAL`, `U_USB.M12`, and both associated capacitors. Retain the
   capacitor ground terminals, complete FX10 footprint, crystal neighborhood,
   all existing copper and other supply regions. Identify the upstream V33
   boundary explicitly. Initially measure local-loop routing; the first planner
   comparison must additionally connect the downstream feed to every load.
2. **Two competing rails and a signal corridor.** Immediately after the first
   domain, exercise two supply domains sharing constrained space with a protected
   USB corridor. Compare greedy sequential allocation against joint allocation
   before trying the entire bank. Keep this first generic competition fixture
   separate from the consumer's exact two-domain geometry experiment.
3. **USB supply bank.** Select `FX_USB18`, its filter output, all seven target
   supply balls, fourteen pin-associated capacitors and bulk capacitor. Add
   adjacent USB pairs as active route obligations. This exposes competition
   between supply vias, ground vias and critical signal exits.
4. **Competing domains.** Activate all seven filtered FX10 domains, CORE11 and
   their source feeds with all USB groups. This tests shared layer allocation,
   narrow boundaries, antipads and return continuity.
5. **Analog preservation.** Exercise an ADC supply domain with its associated
   bypasses and analog/clock routes; retain the analog front-end copper
   exclusions. Before full-board promotion, verify that compiler changes leave
   fixed analog geometry and source constraints intact.

The single-domain baseline does not exercise competing-area synthesis or USB
coexistence. A successful small case is permission to expand the experiment,
not a claim that the FX10 bank or board is routed.

## Contracts and algorithm

Reuse `PhysicalPowerDomain` membership but extend the planning view with explicit
source/feed provenance. A ferrite output is a downstream boundary, not an active
voltage source. Current lowering can identify a domain without identifying that
boundary; never substitute a capacitor or the domain centroid as the source.
Use existing electrical propagation contracts where available and report an
unresolved boundary when they cannot establish one.

Inputs include available copper layers and reference roles, source and load
terminals, local bypass associations, current/voltage-drop budgets when supplied,
source zone reservations, immutable macros, required via arrays, sensitive-node
exclusions and critical-signal corridors. Separate hard constraints from soft
placement/routing costs. Missing electrical budgets do not acquire guessed limits.

Outputs include a snapshot-bound plan, owned provisional areas, explicit feeder
and contact obligations, protected corridors, unresolved terminals and bounded
search statistics. Use stable owner identities and occurrence counts. Distinguish
`proposed`, `contact_routed`, `native_connected` and `electrically_assessed` states.
No provisional area or same-net label establishes filled connectivity.

For the first engine:

- Build a coarse multilayer graph of permissible distribution space with exact
  board, keepout and source-reservation checks at geometry realization.
- Generate a bounded set of connection trees and layer assignments. Include
  alternatives for competing rails; do not permanently grant all free space to
  the first rail visited.
- Sample spatially separated points of verified source-connected copper, keeping
  declared macro ports. The growing tree can also supply branch vertices on
  their actual layer. Keep nearby and dispersed launches instead of retaining
  only the points nearest the remaining loads. Alternate parent contacts share
  the existing per-growth maze allowance. If one load cannot join, try another
  pending load and retry the blocked load only after a successful extension.
  Deferred loads share that same allowance; no new copper component, layer
  transition or conductive zone fill is inferred. All terminals still have to
  join before a candidate can enter allocation or adoption.
- Expand feasible trees into copper corridors with explicit width and clearance
  requirements. Evaluate a coarse resistor network during proposal selection,
  including parallel paths, shared trunks, declared via resistance and return
  conductors. Use explicit copper thickness, resistivity at temperature, source
  boundaries, load scenarios and voltage budgets. Missing inputs prevent an
  electrical pass. The early network is a screening model, not extracted PDN
  qualification; refine it against actual filled copper later.
- Allocate scarce space across rails jointly. The initial kernel searches bounded
  candidate combinations and rejects shared resource conflicts and failed DC
  screens. Geometry generation must certify those resources before using them
  for real board decisions. Benchmark this against sequential allocation and a
  simple tree-growth baseline; exhausted budgets do not prove infeasibility.
- Score unresolved feed/load obligations first, then local access failures,
  reference/corridor conflicts and electrical violations, then area, via use and
  route cost. Preserve all
  hard limits regardless of a better aggregate score.
- Keep native fill out of the innermost search. Refill only shortlisted geometry;
  cache results only against all relevant geometry, rule, stackup and tool inputs.

Ground planning must preserve reference continuity and accessible local returns.
Power-island partitioning does not authorize splitting a signal reference plane.
Use device/topology-specific analog copper exclusions rather than a blanket fill.

Partitioning existing distribution space requires explicit
`partitionable_zone_ids`. Only mutable, non-reserved power zones are eligible;
macro zones, declared returns and signal-reference layers are rejected. The
source zones remain unchanged. Higher-priority provisional areas must pass a
fresh native before/after comparison: every original connected pad group on
every net must remain connected, island count must not increase and no native
non-connectivity violations may remain. Missing or stale baseline evidence
cannot authorize promotion. This connectivity check is not a plane-impedance
or reference-integrity qualification.

## Pipeline integration

Add a focused module such as `pcbir/power_areas.py`; retain the existing
`power_planning.py` membership and placement heuristics as separate concerns.
Connect the initial proposal to `routeflow.py` candidate evaluation before final
global-route acceptance. Refine with `package_access.py` before ordinary area
routing in `flow.py`. Expose controls through the existing routing profile/report
mechanism; avoid a separate consumer repair workflow.

The existing net-wide `distribution_zone_nets()` deferral is a key integration
hazard. Track terminals outside local regions and feeds between filled fragments
explicitly. A downstream island must not suppress routing to its filter or
regulator. Authored zone outlines and immutable macros remain source intent;
trial-generated regions have separate ownership and cannot replace them silently.

Every placement, via, track or area change invalidates affected evidence. A rejected
trial restores poses, polygons, copper, contact ownership and prior certificates
together. Required arrays and verified package exits remain protected while a
net is partial. Reuse the existing native-fill acceptance machinery.

## Fast feedback and evidence

Targets, to be calibrated from measurements rather than treated as guarantees:

| Tier | Initial work bound | Wall-time target |
| --- | --- | --- |
| Generic geometry/contract regressions | Small synthetic boards; no external refill except selected integration tests | Under 10 seconds per focused batch |
| Single consumer domain | At most 4 area candidates, 2 layer alternatives, 2 shortlisted native refills; no global placement search | Under 60 seconds per trial |
| FX10 bank and USB | At most 8 area candidates, 4 placement probes and 2 shortlisted native refills | Under 5 minutes per trial |
| Full board | Existing normal routing profile with explicit planner budgets | Run only after subset acceptance |

Also bound search states/cells and report budget exhaustion. A wall-time timeout
is an inconclusive trial, never an infeasibility proof. Retain enough progress to
identify whether compilation, proposal generation, package access or native fill
dominates. If a small trial exceeds its target, profile that tier before expanding.

Compare baseline and planner with identical source, dependency lock, footprint
bytes, pose seed, macros and search budgets. Bind reports to compiler content
hashes, including dirty files, rather than commit ID alone. Record timings by
phase, selected/retained inventory, connected terminals, source feeds, bypass
identities, pending exits, critical-route status, native findings, copper/via
counts and unresolved electrical checks. Repeat the accepted small comparison
three times and check equal geometry/status; report median timing.

Keep the full board in native diagnostics and label unrelated opens as outside
the trial scope. Do not hide hard geometry errors, delete external terminals or
interpret the full-board native pass flag as a scoped verdict. Promotion requires
a fresh explicit source-to-fragment connectivity check and all selected local
associations, not just a lower open count.

## Required regressions

- Source outside a local island; two disconnected same-net islands; source-free
  external boundary; filtered same-voltage rails that must remain distinct.
- Narrow neck cut by antipads; through-vias perforating a foreign plane;
  critical-signal corridor or ground-reference discontinuity.
- Geometrically near capacitor with a blocked return; required multi-via arrays;
  immutable macros; an unchanged verified exit on a still-partial net.
- Rejected area/placement trials restore all ownership and geometry; changed
  inputs reject stale native evidence; absent tools produce unsupported status.
- Renamed and translated fixtures choose equivalent geometry and outcomes;
  independent source terminal order does not select a board-specific policy.
- Fixed nanometer units, exact quantities and deterministic bounded exhaustion.

## Electrical assessment scope

DC feedback belongs in the first planner's candidate loop. The initial solver
accepts explicit resistor graphs, voltage boundaries and signed current loads;
it preserves exact input fractions and uses a bounded numerical solve with a
Kirchhoff current-law residual check. Floating components, missing electrical
inputs and exhausted budgets remain unresolved. A solver result is only as
complete as its input network: extraction, necks, return paths and via models
are required before making a board voltage-drop claim.

Define regulator, capacitor ESR/ESL/effective capacitance, package and load model
ports in the planning contract now. Higher-fidelity frequency/transient analysis
can follow the working geometry/DC loop. Candidate voltage constraints can name
the local return node; extracted models must bind that reference explicitly.
Absolute node-voltage screening alone does not close that gate. Current density, thermal and AC qualification remain
separate from DC connectivity and native DRC.

## Implementation verification

The 2026-10-18 fixed-reference routing experiment now has successful native
TX1 and USB18 replacements, both separately and together. CopperProbe retains
the fixture, frozen engine, replay commands, aligned reference views and native
reports in `verification/fx10-reference-routing-20261018/`. These are scoped
reference diagnostics, not production CopperProbe route acceptance. The combined
result leaves all ten scoped rails and fourteen USB nets connected and introduces
no native findings; the original eight crop/orphan opens and assembly/import
warnings remain visible. Negative controls detect two missing TX1 connections
and seventeen missing USB18 connections.

The experiment exposed a quadratic zone-priority check; cached polygon boundaries
and a conservative bounding-box rejection now precede the unchanged exact test.
Plane contacts also use bounded obstacle-boundary sites already available to
ordinary BGA fanout. Existing width-checked contact paths are reused without
adding redundant bridges. The affected contact, geometry, power and critical
routing regression batch passes 148 tests. These changes do not enlarge the
site/search allowances or add board-specific repair selectors.

The USB18 adapter initially applied clearance twice. Correcting that test input
allows both the baseline and frontier-site variants to pass. The frontier variant
reduces the seven top-side power leads from 5.164 mm to 4.639 mm, but the reference
uses 3.295 mm of diagonal leads. Total generated track length increases from
135.538 mm to 140.766 mm, so this is not a demonstrated PDN optimization. Its
bottom-layer distribution also differs from the reference inner power area.
Native export preserves the generated zone island policy explicitly.

The subsequent complete-context consumer transfer keeps the actual six-layer
stackup, all 166 placements, all 69 bypass associations, the immutable USB macro,
450/200 micrometre vias and original routing constraints. USB18, FX_V33 and
FX_VDDD now allocate together: twelve candidates, six conflict retries and
21 allocation states produce three jointly compatible choices. Fresh KiCad
checks connect every selected supply and required return, preserve old pad
groups and report zero other violations and zero islands. The partial board
still has 230 native unconnected items. Reintroducing the reserved ordinary
copper also produces no physical spacing/short findings. This is a three-domain
result, not an eleven-domain or full-board route acceptance.

A source-backed eight-layer diagnostic uses JLCPCB's JLC08101H-3313 catalogue
construction, retaining the outer signal/ground dielectric and adding two
central distribution layers. It also passes the same three-domain native
checks with ten new vias, the same number as six layers. This subset does not
justify changing the production stackup. The layer change invalidates prior
electrical and fabrication evidence; catalogue dimensions are not an order or
SI/PDN qualification. Full-domain preparation and ordinary/global handoff must
still be tested before a complete-board routing claim.

Continue comparing generated contact geometry with the aligned reference,
preserve pin-associated supply/return obligations, and prefer shorter reference-like
dogbones where exact clearance permits. TX1's diagnostic
400 µm skew allowance and the reference kit's eight local USB18 capacitors do
not replace the consumer's tighter signal limits and fourteen-capacitor contract.
Keep the separate placement/routing stage for power and ground area planning;
signal feasibility and filled connectivity alone do not qualify SI or PDN behavior.

The earlier retained planner, pipeline and native-CLI regression batch passes
118 tests without failures or skips (`build/power-final-42.xml`). This is affected
coverage, not a claim that the entire repository test suite was run. Fixtures
cover shared-trunk DC networks, local return voltage, extracted copper and via
resistance, floating islands, bounded failure, native preservation, ownership,
joint geometric allocation, source provenance, stale evidence and missing tools.

The allocator records at most sixteen dead-end witnesses within its existing
state allowance. A witness names a blocked domain and a compatible prefix of
candidate owners; it is never an accepted partial assignment. Optional bounded
conflict retries regrow against these owners together, falling back to a single
conflicting owner. Every held owner is reported in `conflict_contexts`. Temporary
copper stays on a disposable board, all proposals are recertified jointly, and
all allocation passes share the original state cap.

The full eleven-domain transfer subsequently reproduced the previous 44-candidate
geometry exactly and exhausted its twelve regrowth trials. Read-only reduction
of that pool again isolated USB18/FX_V33/FX_VDDD; every pair was feasible. Thus
the focused three-domain pass does not establish that obstacle-boundary contacts
alone fix the full scene. Its repair allowance was concentrated on that conflict,
whereas full-scene witnesses included unrelated owners.

The planner now reduces a failed candidate pool to an inclusion-minimal
conflicting domain subset before multi-owner regrowth. Each deletion query is
charged to the original allocation-state allowance; an exhausted query cannot
establish a smaller conflict. A cached subset is rechecked after the pool changes,
and a feasible subset never substitutes for a complete assignment. The result
records these queries in `conflict_core_observations`. Candidate regeneration
prefers a permitted distribution layer different from the held owners' primary
layers, with an allowed inner layer preceding another component face, while
retaining exact contact/geometry checks and full joint allocation.
This does not prove that an exhausted geometric search is physically impossible.

The latest full six-layer run with both changes remains unresolved after
44 candidates, twelve conflict retries and 1554 allocation states. Read-only
reduction uses another 498 of the unspent states and again isolates the
USB18/FX_V33/FX_VDDD triple, with every pair feasible. In the displayed rejected
combination, the USB18 In3 branch at y = 28.725 mm intersects proposed VDDD
through-vias at x = 77.75 mm and 79.75 mm. These are temporary candidate owners,
not immutable source vias. The reference uses distributed contacts and shaped
areas instead of this crossing feed arrangement. No full-scene assignment
reached the native adoption gate.

The eight-layer control also remains unresolved (48 candidates, twelve retries,
1461 allocation states). Its displayed V33 In2 branch crosses VDDD through-vias
at (78.75, 27.725) mm and (80.75, 27.725) mm. The extra layers do not eliminate
these all-layer obstacles. This control used the pre-reducer engine; it is not
a test of the new repair logic on eight layers or proof that eight layers
cannot help after further geometry changes.

A separate read-only union of the final full-scene pool and all twelve
three-domain candidates also remains unresolved after fresh exact pairwise
certification: 56 candidates, 91 allocation states. Its reduced conflict adds
the upstream V33 domain to USB18/FX_V33/FX_VDDD (985 diagnostic states). This
combines independently generated pools and is not a successful bounded run.
It shows why importing the successful subset alone is insufficient.

The next geometry experiment should therefore include those four domains on
the unchanged complete scene. Coordinate feed contacts, through-via sites and
distribution corridors together, and compare each changed package region
against the aligned manufacturer layout. A generic repair must propagate
exact conflicts back to both mutable owners' contact arrangements, preserving
source copper, bypass obligations, arrays, critical macros and ordinary exits.
Test a competing-feed fixture first, then the four-domain consumer scope with
fresh native source/load/return preservation, then all eleven domains. Compare
six and eight layers with the same final engine and explicit layer-candidate
coverage before changing the production stackup. Do not proceed to global or
detailed routing until complete preparation and ordinary-access handoff pass.

Candidates retain their tree width, reverse order and detour so a conflict retry
does not silently replace the working seed. Multilayer branch fallback uses only
actual explicit contacts of both connected components on permitted layers.
Protected reference regions, missing contacts and forbidden layers remain
barriers. No mid-route transition is invented. The bounded analytic contact
alternatives precede coarse/fine maze search; failed searches report exact
endpoints, layers, work and sampled blockers without claiming infeasibility.

The protected-area predicate uses a conservative bounding-box broad phase;
nearby shapes retain the exact original clearance predicate. Ordinary physical
routing checks still run first. Candidate and regrowth progress reaches the
standard placement/routing progress callback without changing work or identity.

Consumer evidence belongs in CopperProbe under `verification/power-area-planner/`:
`implementation-history.md` retains the detailed experiment narrative,
`implementation-status.json` indexes outcomes, and each trial retains exact
input hashes and native evidence. Five competing domains and the ADC supply
pass scoped native preservation. The expanded ten-domain allocation and all
eight critical groups remain acceptance gates; no full-board promotion or
electrical qualification is claimed. Missing current, voltage, material,
barrel and dynamic models stay explicit.

## Whole-package fanout planning: implementation plan

Status (2026-10-18, updated): P1 contact patterns, coupled support placement and
prospective power/return integration are implemented as explicit bounded options
in the standard pipeline. `pcbir/package_patterns.py` detects circular arrays,
generates coherent directions and geometry-derived common offsets, and checks
held-scene and mutual conflicts. `pcbir/package_support.py` arranges eligible
pin-associated capacitors around those via pockets before final placement checks.
`pcbir/package_preparation.py` freshly revalidates the complete pattern after
critical routing: selected supply/return contacts receive exact protected owners,
while other contacts remain temporary reservations. P2 joint transition-layer and
boundary-exit selection, full ordinary handoff and full-board acceptance remain
open; contact geometry alone does not establish these outcomes.

The CopperProbe correction moves 22 of 47 FX10 bypass capacitors and clears all
85 active general-route contacts in the complete held scene (166 placements,
619 tracks, 10 existing vias). The complete manufacturer board is the comparison
source, including actual bottom pads. The selected outward pattern uses a
299999 nm diagonal offset with unchanged 450000 nm / 200000 nm ordinary vias.
The bounded solve uses 29361 pose checks, 22 search states, 66779 pair queries
and one complete placement check. Fixed rules were never silently relaxed: the
consumer explicitly authored the verified new capacitor poses after a disposable
placement experiment. Electrical values, net associations, critical macros,
analog placement, source regions and fabrication rules are preserved.

The saved consumer source compiles and passes ERC. Fresh native three-domain
preparation connects FX_USB18, FX_V33 and FX_VDDD and their selected returns,
with zero other violations and the same two baseline islands. Its 324 whole-board
opens remain outside this scoped acceptance. A final replay binds the current
engine, compiled source, exact contact geometry/ownership and fresh native fill.
The evidence and aligned reference/before/after view belong in CopperProbe's
`verification/fx10-package-support-20261018/`. Earlier results below remain tied
to their old source and do not describe the corrected candidate.

Use `--package-support-trials N` on `plan-layout`, `route-global` or `route-board`
to enable a finite number of complete placement trials (default zero). Only
eligible unfixed, unconnected support parts may move; fixed/rigid/macro members,
required arrays and existing partial contacts are protected. The private
prospective index defers movable SMD lands solely while selecting a pattern;
complete source inventory and exact full-scene checks are restored before any
proposal returns. Rejected trials return the original placement and copper.
The final 187-test affected suite passes with zero failures, errors or skips,
including real KiCad checks, rotation/name independence, fixed/partial-copper
protection, bounded rejection, duplicate source occurrences and stale/conflicting
reservation replay and duplicate-reservation prevention at ordinary handoff.
CopperProbe's 22 hardware/power contract tests also pass.

For already authored poses, power configuration can enable
`package_patterns_before_contacts`, with `package_pattern_offset_nm` or per-package
`package_pattern_offsets` as optional search seeds. Seeds never bypass fresh
geometry checks. Placement reports automatically supply seeds to native-backed
power preparation. Temporary contacts stay separate from producer connectivity;
snapshot digests, fresh obstacle checks and exact occurrence ownership protect
replay. No provisional signal contact becomes native continuity evidence.

### Contact ranking and reversible multi-owner area repair (2026-10-18)

Complete contact arrangements now rank by added physical track length, then
added via count, before the existing deterministic tie break. A long surface
bridge to an existing via must not automatically consume the candidate slot
ahead of an already checked short local plated contact. Both arrangements keep
their original identities for replay; route widths, via limits, source copper
and the finite proposal allowance remain unchanged.

An allocator's last blocked rail reflects search order. When every rail pair
has a compatible choice but a larger subset does not, bounded repair now proves
a compatible held assignment for the subset minus each possible moving rail.
These reverse repair directions share the original allocation-state budget.
Regrowth still changes one disposable proposal, and the entire same-source
candidate pool must allocate again before any adoption. No partial assignment
becomes accepted copper; stale snapshots, failed electrical screens and exhausted
searches retain their existing rejection rules.

When a conflict core is smaller than the selected scope, each proven context
also gets one bounded extension through surrounding domains, most constrained
first. The existing allocator screens each additional choice against the held
resources and electrical eligibility. Compatible neighbours become temporary
obstacles; an incompatible outside domain remains unresolved. Both extended
and original contexts are retained, and less-tried repair directions take
precedence before context coverage. All extension queries share the same state
budget. This avoids repeatedly repairing one local conflict by recreating a
different conflict outside the core; it does not claim optimal context coverage.

The corrected CopperProbe contact scene exposes a three-domain VDDD/crystal/V33
conflict. Contact ranking alone remains unresolved after four repair slots.
With reverse multi-owner contexts, the same full-obstacle subset selects all
three domains after two repairs and 39 allocator states (212.381 seconds).
The physical application passes; this subset result is native-pending and does
not establish eleven-domain or whole-board acceptance. Its reference comparison
uses actual aligned manufacturer bottom copper and filled regions, including
the reference's separate inner power layer. The consumer keeps its six-layer
stackup and protected ground regions.

The first full-scope reverse-context run generates 47 candidates and spends
819 allocator states and twelve repairs in 543.272 seconds, but remains
unresolved. Each local repair produces geometry; the final conflicting subset
contains USB18, VDDD, crystal and upstream V33. A separate diagnostic finds no
compatible full assignment after removing any single domain, so merely changing
one last-blocked owner is insufficient. This motivates the surrounding-context
extension above. This rejected result preserves its exact engine hashes and is
not a native-accepted power board.

The surrounding-context implementation then passes the five-domain
USB18/FX_V33/VDDD/crystal/upstream-V33 subset in the complete held scene:
22 candidates, five jointly selected, 437 allocation states and eight repairs
in 466.527 seconds. The last FX_V33 repair holds a proven compatible assignment
of all four other rails. Applying the complete proposal passes physical checks;
native acceptance remains a separate full-stage gate. All 147 affected tests
pass without failures, errors or skips, including real KiCad checks. The tests
cover deterministic reverse directions, compatible surrounding choices, smaller
fallback contexts, electrical ineligibility, source identity and shared budgets.

Fresh KiCad fill of that five-domain proposal verifies every selected domain
and its local associations, all selected returns, and no lost old pad groups.
The complete held scene still has eight dangling vias on unselected CORE11,
FX_GPIO, FX_P0 and FX_P1, so the strict preparation gate remains unresolved;
these findings are not waived. It has 237 whole-board opens and zero islands.
Only an expanded native result may accept the complete power preparation.

The first surrounding-context full run remains unresolved: 47 candidates,
1114 allocation states and twelve repairs in 578.811 seconds. The last minimal
core is USB18/FX_V33/upstream-V33, while other overlapping conflicts also remain.
The controller now remembers overlapping multi-domain cores as a group and
solves the group's held choices jointly before selecting a repair direction.
This can distinguish an owner shared by two conflicts from an owner that affects
only one. Disjoint groups remain separate; if a group has no compatible held
context, the current minimal core remains a bounded fallback. These queries use
the same allocator allowance and do not accept any partial assignment.

With conflict-group memory, the twelve-repair full run reaches compatible
eight-owner contexts but remains unresolved at 47 candidates and 1137 allocator
states (665.352 seconds); a final diagnostic isolates D18/FX_V33/VDDD. All 148
affected regressions pass without skips. A separate sixteen-repair experiment
keeps the 4096 allocation-state limit, four candidates per initial domain and all
physical constraints unchanged. It is a separately identified budget experiment,
not a continuation that replenishes a failed run's counters. Candidate identities
include the options contract, so changed-budget tie ordering must be inspected
before attributing any result solely to the extra slots.

That sixteen-repair control remains unresolved: 47 candidates, 1748 allocator
states and 733.679 seconds. Its four extra attempts hold seven or nine neighbours
and fail D18 contact access, without generating a new candidate. The recorded
unreachable contacts are C_FX_C7.1, C_FX10_C7.1 and C_FX_E3.1. An aligned comparison
with the manufacturer's actual VDDIO_CTRL bottom copper shows short explicit
connections; no VDDIO_CTRL filled region was found in the imported reference.
The kit and consumer circuits differ, so this comparison is a routing witness,
not a claim of circuit or capacitor equivalence.

The controller had replaced the current core's smaller dead-end contexts when
a larger overlapping group offered any compatible held assignment. Compatibility
of those held rails does not guarantee that a new contact can route around them.
The repair now retains the smaller contexts alongside the larger group. Fair
direction selection can then change a blocking neighbour after a full-group
attempt fails. This does not waive source ownership, physical checks, finite
budgets or the requirement to allocate the entire candidate pool again. All
148 affected regressions pass after this correction, without failures or skips.

With local fallbacks retained, the sixteen-repair run adds a VDDD alternative
and resolves the D18/FX_V33/VDDD core, but exposes D18/USB18/VDDD. It remains
unresolved after 48 candidates and 1972 allocator states (819.948 seconds).
A fresh-contact control against the same nine held neighbours also fails,
so carrying the old contact-variant index is not the sole cause.

Physical rejection can now request another compatible held assignment with
the same exact owner scope. Only the rejected combination is excluded from
that context query. No individual owner is banned, and no exclusion is passed
to complete-board allocation. The original contexts remain available for other
layer/topology trials, with fresh contexts preferred after an exact failure.
Context queries share the original allocator allowance. Regressions prove
determinism, exhaustion, unknown/moving/duplicate owner rejection, and that an
outside owner's changed choice can retain the original core choices. Projecting
the failure onto a smaller owner set would incorrectly discard that solution.
All 150 affected tests pass with no failures, errors or skips.

The first alternate-neighbour D18 control still fails. Tracing its contact
arrangements reveals a non-monotonic bridge dependency: arrangement zero joins
C_FX10_C7 but leaves C_FX_C7/C_FX_E3 pending; arrangement one gives the latter
two direct vias but loses the former bridge. Every individual direct contact
exists in the finite pool. A diagnostic union retaining the earlier bridge
forms a D18 geometry proposal with no additional physical DRC findings, but is
not adopted or implemented as an unchecked merge in the production planner.

A separately bounded three-contact-arrangement control resolves the same held
D18 context through the existing feedback mechanism (106.668 seconds). It
selects contact variant two and generates a complete D18 geometry proposal.
This changes only a consumer search allowance, not source geometry or fabrication
rules. Full native preparation remains the acceptance gate. The reference and
the new isolated bottom contacts are retained side by side; no RF or electrical
qualification is inferred from contact connectivity.

Fresh KiCad fill verifies the isolated three-arrangement D18 proposal: all D18
terminals, local associations and selected returns connect, with no lost prior
pad groups or islands. Whole-board opens fall from 206 to 190. Four unchanged
dangling vias remain on FX_P0 (two), FX_GPIO and FX_USB18 in the provisional held
scene; the strict full-preparation gate remains unresolved and those findings
are not waived. This local native result does not accept all held neighbours.

The full alternate-context run with only two contact arrangements remains
unresolved after 48 candidates, 3167 allocation states and sixteen repairs
(1025.793 seconds). It does try different exact neighbour combinations after
the D18 failure. This is retained as the same-engine negative control for the
explicit three-arrangement consumer trial, rather than replacing or hiding
the unsuccessful result.

The three-arrangement full run still fails with a forced old contact index:
49 candidates, 2111 allocation states, sixteen repairs and 904.655 seconds.
A controlled isolated replay confirms the distinction: choosing afresh selects
complete variant two, while forcing old variant one leaves C_FX10_C7 pending
even though variant two was already generated. Repair attempts now pass a
contact preference instead of a strict replay seed. Fresh completeness checks
retain the preferred bank when complete and otherwise select an already checked
complete bank. This spends no extra contact search budget, preserves strict
diagnostic replay and retains exact source/owner recertification. Two physical
regressions cover complete and incomplete preferences and identical work counters.
The focused 66-test suite passes before the expanded consumer rerun.

The final affected suite passes all 152 tests with zero failures, errors or skips
(268.072 seconds). Regenerating the full-obstacle three-rail subset with this
engine again selects all three domains after two repairs and 39 allocator states
(220.513 seconds). Its physical application passes. This current-engine result
is retained separately from the earlier core so deterministic replay uses the
matching candidate identities and contract digest.

### Native completion of held power transitions (2026-10-19)

The complete three-arrangement run with fresh contact preferences first reaches
an eleven-domain assignment after fifteen geometric repairs: 49 candidates and
2308 allocator states. Native fill finds 183 whole-board opens, zero islands and
one dangling USB18 via. Other assignments do not clear this last finding; the
normal preparation remains rejected. The reference comparison identifies U_USB
L3: the kit's 0.4064 / 0.2032 mm plated via is at aligned (85.3328, 20.8328) mm
and touches actual In4 power fill. The consumer's unchanged 0.45 / 0.20 mm via
is at (85.299999, 20.799999) mm but connects only to F.Cu. Its dogbone is held
contact copper, so planar-junction normalization correctly preserves it.

The generic native transition repair uses exact source/export-bound via UUIDs
to propose an additional connection on another permitted layer. It retains all
original tracks, via occurrences, zones, fixed intent and package reservations.
It shares the existing conflict-trial and allocator allowances and one bounded
branch-search allowance across the selected net's flagged transitions and layers.
Source widths, via spans, layer restrictions and exact clearance remain gates.
The complete candidate pool receives fresh pairwise conflict certificates before
the complete repaired assignment is allocated and physically applied. No partial
replacement or native rejection becomes accepted geometry. A changed assignment
requires another fresh native trial before ownership is frozen.

The source-bound consumer diagnostic adds one 200 µm In2 track, approximately
1.061 mm long, from the L3 via to an existing same-net transition. The consumer's
In4 ground layer remains unchanged. Fresh KiCad validation changes the result
from 183 opens / one other finding / zero islands to 183 / zero / zero; every
selected domain and local association is connected, with no lost previous pad
groups. This isolated result does not replace the normal full-stage adoption
gate. The reference crop, before/after geometry and exact native evidence are
retained together. The targeted 13 regressions cover stale and foreign evidence,
layer restrictions, exact source preservation, bounded failure, rollback and
fresh native acceptance in the standard preparation controller.

The complete normal run then accepts all eleven domains after the second native
trial, with 50 candidates, 2320 allocation states and sixteen total repair slots.
It retains 1239 tracks and 198 vias; all 63 local associations and 112 selected
return pads pass. No rejected native geometry becomes held output. A final
guard keeps modeled candidates unresolved rather than carrying an old extracted
electrical network through a geometry change. The guard also covers models that
use another rail as a return. Complete electrical reassessment for such a repair
is not implemented. CopperProbe has no electrical models; its final normal
controller replay recertifies the retained same-source pool, preserves spent
counters, and produces exactly the same accepted board and fresh native counts.
It begins at 2438 allocator states and fifteen repairs, and ends at 2515 states
and sixteen repairs in 117.431 seconds. Complete unseeded generation and the
later guarded replay are retained separately, with their engine hashes.

The fresh full ordinary handoff takes 144.191 seconds and confirms that fitting
the dogbone lattice is distinct from completing its package-boundary exits.
All ten pending FX10 pads have legal selected launches; two have exhausted
boundary domains and eight have unallocated candidate paths. The ADC DFS/OE
pins and converter EN pin have no generated fanout candidate. Reference crops
compare all ten aligned FX10 pins plus the current ADC/converter regions with
the actual kit, related ADS41xx EVM and TPS62130A layout example. The ADC raster
is a family analogy, not an exact board registration or a source of trace widths.
The consumer's EN branch still inherits the VBUS conductor rule. The next repair
must complete P2's joint transition-layer/boundary allocation and source-backed
control-terminal approaches; a larger contact count alone cannot close it.
Intermediate ordinary dangling ends remain native failures, not accepted power
copper. Current failure measurements supersede the earlier twelve-pad handoff.
All ten reserved FX10 dogbones still pass fresh track/via checks against the
accepted power board. The measured blocker is therefore the onward exit and its
joint allocation, rather than a capacitor occupying those planned via sites.

Historical first-pilot validation:

The affected 74-test suite passes with zero failures, errors or skips, including
real KiCad geometry checks and a fully connected synthetic pattern with zero
native errors or unconnected items. Transformations, repeated-name independence,
shared-rail drill conflicts, fixed macros, required via arrays, declared width
transitions, source limits and finite work bounds are exercised. The generator
returns proposals only; boundary access and native acceptance remain false.

The first consumer pilot keeps the complete critical-prefix obstacle scene and
all fixed poses. Its twelve-contact bank is unresolved in about 0.4 seconds,
using 863 unique site queries, 55 exact pair checks and 139 offset evaluations.
Five contacts have no legal generated site; backside capacitor lands are recorded
blockers. Actual manufacturer sites were compared on the same scene, including a
separately identified reference-via-size control. Some blocking capacitor poses
are fixed by source rules, so they were not moved. Exact decision/work replay and
the source digest match. This is a useful failed fast pilot, not a routed-bank
acceptance. Frozen inputs, reference comparison and test evidence belong in
CopperProbe's `verification/fx10-package-patterns-20261018/`.

The next preparation improvement is a coherent package fanout plan shared by
signal, power and ground obligations. Existing circular-array site generation,
ring-depth surface preferences, finite escape assignment and per-domain power
contacts are useful building blocks. They do not yet jointly select the entire
package's dogbone directions, offsets, transition layers and exit channels
before mutable local feeds occupy the space. The ordinary fanout path currently
runs decoupling first; power contact assignment is per obligation. Extending
candidate counts alone does not supply the missing coordination.

This follows the documented separation between
[BGA fanout and subsequent layer escape](https://www.altium.com/documentation/altium-designer/pcb/routing/interactive),
and the use of
[quadrant directions and near-pad or cell-center via patterns](https://www.altium.com/documentation/altium-designer/pcb/design-rule-types/routing).
[TI's AM570x guide](https://www.ti.com/lit/pdf/spruip9) also coordinates routing
channels, bottom-side decoupling and internal power regions. Its depopulated
VCA channels and package dimensions are not transferable constraints for a
fully populated array. The proposed bounded implementation is our engineering
inference from these sources, not a claim to reproduce a published optimizer.

Implement and accept in this order:

1. **Geometry-derived pattern proposals.** Infer array axes, pitch and rings
   from actual pads; explicitly handle incomplete/irregular arrays. Enumerate
   bounded quadrant, uniform diagonal and staggered families with common legal
   near-pad offsets as well as cell centers. Preserve declared surface/pair
   exceptions, unused-pad policy, layer restrictions and via multiplicity.
   Never select a package or ball by a consumer-specific name.
2. **Joint fanout and escape reservations.** Select compatible signal, power
   and ground vias together with allowed transition layers and ordered boundary
   access. Account for real channel width and capacity. A legal via site alone
   is not a completed package escape. Reuse exact physical checks and bounded
   conflict-directed assignment; report exhausted work separately from proven
   local conflicts. Existing accepted copper stays protected.
3. **Placement and power-area coordination.** Supply the proposed via pattern
   and channel reservations to mutable decoupler placement, with explicit
   supply/return anchors, then to power/ground region planning. Evaluate them
   together before committing mutable local feeds. Preserve fixed/rigid poses,
   pin associations, immutable macros, required arrays and previous native
   connectivity. An incompatible fixed component produces an exact conflict;
   it is never silently moved. Area planning remains a distinct stage before
   general routing, with feedback to placement and package planning.
4. **Bounded integration and rollback.** Integrate into the standard
   `route-board` preparation flow. Every trial has exact owners, fresh physical
   and native preservation checks, a shared finite work budget and complete
   rollback. Keep electrical, RF and manufacturing contracts unchanged.
5. **Small-to-full acceptance.** First exercise rotated/mirrored arrays,
   occupied back-side via sites, competing rails, coherent patterns that beat
   greedy choices, saturated channels and rejected-trial rollback. Then compare
   against a complete manufacturer fanout with explicit source provenance.
   Test a small consumer bank in the complete held obstacle scene, expand to
   all required package balls and all power/return domains, then verify package
   and global handoff before detailed routing. Recheck native connectivity,
   dangling geometry, shorts, clearances and islands without waiving findings.
   Compare six and eight layers with the same engine and via/clearance policy;
   extra layers cannot remove a through-via collision with a bottom-side pad.

### Execution order and interfaces

Use the existing preparation entry point. Its intended order is:

```text
source intent and fixed placement
  -> critical routing and immutable macro geometry
  -> whole-package pattern proposals and layer-exit witnesses [new]
  -> pin-associated local feeds, return contacts and power/ground areas
  -> fresh native preservation and package/global handoff
  -> normal detailed routing
```

Mutable support placement can propose a different pose outside this transaction
and rerun preparation. A changed pose invalidates the prior candidate and native
evidence; `ContactPreparationSnapshot.matches()` must not be weakened to accept
an in-place placement change. Fixed/rigid source components remain fixed.

Two existing behaviors need explicit integration work. First,
`prepare_power_contacts()` currently reserves ordinary exits and then generates
power/return contacts separately. Second, its temporary-reservation interface
rejects geometry on a producer's own net. Do not bypass this guard to insert a
mixed-net pattern. Materialize chosen supply/return dogbones as owned prospective
contacts on the private board; hold signal exits as separate reservations.
Consumers of either kind must reuse the exact chosen geometry or reject it,
rather than silently generate an independent replacement.

`boundary_access.py` already constructs exact-clearance package-to-boundary
paths. Reuse those paths as feedback into pattern selection, instead of adding
another escape router or relying on lane-count estimates alone. Power and
ground need access to their permitted distribution region, not an artificial
signal port on the package edge. A prospective contact is not a filled-plane
connectivity witness.

### Work packages and acceptance gates

**P0 — Freeze inputs and add a comparison harness.** Retain the complete
manufacturer reference, current held source, critical prefix, options and source
digests in the consumer. Record each active ball's role, current contact and exit
status, and the exact stage at which it becomes blocked. Report both full-source
reference metrics and consumer active-pin metrics. Keep different active
circuits distinct. The reference endpoint classifier must reproduce its recorded
result; the old cropped file is not an all-ball oracle. This is the baseline for
later A/B runs, not a new router acceptance test.

**P1 — Add coherent geometry-derived patterns.** Add a small generic
`pcbir/package_patterns.py` module using the transforms and circular-array
helpers in `access_sites.py`. Proposed immutable records are an array description,
per-pad contact intent, pattern candidate and rejection/work report. They carry
the source digest, exact terminal IDs, actual tracks/vias, required multiplicity,
permitted layers and protected exceptions. Names are implementation suggestions;
no new `.copper` syntax or library definitions are needed for the first slice.

Start with complete regular circular arrays. Explicitly report unsupported
irregular geometry and retain existing behavior for it. Enumerate outward,
inward, four uniform diagonal and two staggered families in package coordinates.
Consider at most three common offset alternatives per family: a near-pad legal
offset, cell center and an obstacle-frontier alternative. Derive offsets from
actual clearance geometry, round into engineering integer coordinates and
recheck the final shapes. Do not encode the manufacturer's offset as a constant.
Deduplicate before exact checks. Reserve existing critical geometry and permitted
surface exceptions; do not fan out unused balls without source authorization.

P1 passes when translated, renamed, rotated and mirrored synthetic arrays
produce equivalent patterns; a backside pad blocks the real plated span; and
every emitted dogbone passes the existing track/via/drill predicates. A second
fixture must show a repeated pattern succeeding where locally preferred choices
conflict. Unsupported shapes and exhausted candidate bounds remain explicit.
Tests belong in `tests/test_package_patterns.py`, with existing geometry-access
tests retained. No consumer-specific pad selector enters the engine.

**P2 — Select patterns with real layer exits.** Evaluate complete pattern
variants across signal, power and ground balls, then obtain physical signal
paths from `boundary_access.py` and contacts to permitted power/return layers.
Rank only legal variants: first required coverage and preserved prior exits,
then missing boundary access, then top-layer detours, vias and length. Record
the individual metrics so a short but incomplete result cannot win. Pattern
similarity is a diagnostic, never a reason to waive electrical constraints.

Use the existing `escape_assignment.py` / `joint_access.py` machinery for small
conflict components and explicit shared-via ownership. Do not raise the local
12-obligation limit to 169. Search whole-pattern alternatives at the outer level;
repair only bounded connected conflicts. If a conflict exceeds that bound, reject
the variant and report its exact members. Keep cross-sector conflicts visible.
Test a legal via lattice with no available exit, an alternative pattern that
restores exit capacity, return/supply competition and preservation of USB/pair
owners. No candidate is called an escape merely because a via fits.

**P3 — Integrate fixed-pose preparation end to end.** Add the opt-in pattern
selection before ordinary reservations and contact producers in
`power_preparation.py` / `joint_preparation.py`. Feed chosen contacts into
`power_contacts.py`, `plane.py` and `decoupling.py` as needed, with complete
ownership and exact reuse checks. Carry the pattern digest and exit reservations
through contact snapshots, area planning and `preflight_prepared_power_access()`.
Keep ordinary witness copper provisional until its normal handoff; do not export
it as completed routing to avoid a native dangling finding.

Reject stale digests, dropped obligations, unsupported via sharing and any lost
previous exit. Native rejection returns the identical input board, ownership and
reservation state; rejected candidate caches/evidence cannot enter a later trial.
Tests extend `test_joint_preparation.py`, `test_power_preparation.py` and
`test_package_access.py`, including a native negative case. Existing callers
with the option disabled must retain their current geometry and work behavior.
The first consumer experiment happens here, with all capacitor poses held.

**P4 — Coordinate mutable capacitor patterns and power areas.** Extend
`support_patterns.py` so permitted support-placement trials are anchored to the
chosen supply/return vias and preserve their escape corridors. Both capacitor
lands, body/courtyard, actual via barrel span and required local loop constraints
participate. Rerun P1-P3 on each trial's fresh source; share the enclosing work
budget. Feed accepted channel reservations into the existing power-area stage;
power regions must not consume those channels or reference-plane requirements.
This is a distinct placement/preparation/area-planning step before general tracks.

Test a movable capacitor that resolves a blocked pattern, its fixed counterpart
that correctly fails, a two-rail conflict, required via-array preservation and
complete rollback after native rejection. Reuse support-power, ownership and
native power tests. Do not move an immutable reference-layout experiment just
to obtain a pass; any separate mutable consumer experiment gets a new digest.

**P5 — Expand consumer acceptance and compare layer counts.** Use the acceptance
ladder below, with replayable inputs and measured work at every rung. Promote the
result only after the same normal `route-board` gates pass. If six layers reach
a documented channel-capacity blocker after these changes, repeat the relevant
scope on eight layers with the same final engine, via policy and electrical
constraints. A through-via/backside-pad conflict is resolved by geometry or
placement, not by declaring additional layers a fix.

### Consumer experiment ladder and measurements

The first subset is a small bank on the USB-facing side of the package containing
supply/return contacts and adjacent ordinary signal exits. Its explicit pad scope
and exact input are recorded in CopperProbe, selected from the P0 conflict map.
Include the affected bottom-side capacitor geometry and the protected USB paths.
Limit the first mutable conflict cohort to the existing 12-obligation bound.
Keep the complete board's obstacles and all other source constraints present;
reduce the obligations being solved, never crop away blockers to manufacture a
pass. Existing contacts outside the trial remain held.

| Rung | Required result before expansion |
| --- | --- |
| Reference and synthetic harness | Exact reference replay; generic transformed fixtures and negative cases pass. |
| One bank, fixed poses | Every selected required ball has a legal intended contact and required exit; zero lost held geometry or witnesses. If fixed geometry blocks it, retain exact blockers and proceed only to the separately permitted placement trial. |
| Same bank, permitted support movement | Complete selected supply/return associations; fresh native preservation; fixed owners and critical paths unchanged. |
| Existing three-domain scope | USB18/FX_V33/FX_VDDD meet their native gates with ordinary package exits reserved in the same scene. |
| Whole package and all eleven domains | All active required FX10 contacts/exits and selected power/return associations pass; ADC/analog intent and previous native groups survive. |
| Normal handoff and routing | No pending required package exits or unresolved global capacity failures before detailed routing; final native findings remain authoritative. |

Every run records source/options/pattern digests, required versus resolved
contacts, missing exits, reference-versus-generated topology, top escape length
distribution, via counts, layer/edge utilization, exact rejected owners, runtime,
search work and native findings. Each blocked package escape gets a side-by-side
reference comparison; a previously explained blocker can reuse its hash-bound
comparison until its inputs change. Full-board timing is not used for the fast
inner loop. Run the existing affected regression suites after each integration
change, then broader required checks once the vertical slice passes.

### Work limits, progress and completion

The initial retained outer pool has at most eight direction families times three
offset choices (24 deduplicated pattern variants). The P1 implementation also
scores at most sixteen geometry-derived frontier offsets per family before
retaining one: at most 144 offset evaluations, with cached per-pad checks sharing
the same 24,576-site, 200,000-pair-check and 2,000,000-pair-query limits. Frontier
collection is independently capped at 2,048 queries and 24 sites per pad/query.
All work, including discarded offset evaluations, is reported. These bounds are
not a measured sufficiency claim. For P2, reuse current local assignment limits (12 obligations,
20,000 search states, 200,000 exact pair checks and 2,000,000 pair queries), but
share the state/check counters across pattern retries instead of replenishing
them. Add an explicit aggregate boundary-path search allowance. Existing native
and placement trial limits remain explicit and shared by the enclosing run.
Changing any budget creates a separately identified experiment, never a silent
retry. A candidate cache is valid only for the exact geometry/options digest.

Emit progress after pattern generation, exact checks, boundary allocation,
local feeds, area planning and native review. Distinguish unsupported input,
empty finite candidate pool, owner conflict, exit-capacity failure, work-budget
exhaustion, missing native tooling and native rejection. These are different
reasons to stop; none alone proves global geometric infeasibility.

The implementation is complete when the generic opt-in stage, consumer ladder,
rollback/constraint regressions and normal handoff all pass. Full routing closure
remains a reported board result, and electrical/RF/assembly/CAM qualification
retains its separate gates. If a fixed owner prevents progress, return a concrete
conflict and its reference comparison rather than continue budget-only retries.

The complete-reference classification and the correction to the earlier
power/USB-only comparison belong in CopperProbe's
`verification/fx10-full-fanout-research-20261018/`. The new artifacts establish
the topology target; they do not establish a new route, native DRC pass or
electrical qualification. Reproduce the classifier first, but do not use its
reference counts as a universal target for consumers with different active pins.

## Opt-in use and explicit limits

```console
copper route-board board.copper --locked --offline --routing-profile balanced --plan-power-areas --power-area-net FX_XTAL --power-area-config power-area.json --verify-plane-fill /path/to/kicad-cli --report trial/route.json -o trial/board.kicad_pcb
```

Repeat `--power-area-net` for joint planning. With no selection, the planner uses
declared power domains. A configuration file is optional. The report includes
the source snapshot, contract digest, terminal/feed obligations, selected owner
IDs, finite work limits, adoption status and fresh native/electrical observations.
Planner enablement is explicit and does not change normal profile work limits.

For native-backed `plane_return_nets` preparation, `maximum_native_trials` can
be set explicitly (for example, `3`) to try unverified assignments from the
already generated candidate pool after rejection. Its default is `1`. It
does not regenerate contacts, enlarge maze budgets or replenish allocator
states. If repair and branch-layer allowances remain, an exactly mapped dangling
power via may receive an additive other-layer connection before reselection.
This spends one existing repair slot, recertifies the full candidate pool and
requires a complete new assignment. Modeled candidates remain unresolved until
their complete changed electrical network can be reassessed; old network
results never authorize the additional copper. Each attempted assignment receives its own fresh fill/report; all
rejected combinations and observations are retained in `native_trials`.
Ordinary power planning without native return preparation retains its existing
single-adoption behavior.

An example configuration containing only geometry controls is:

```json
{
  "schema": "copperscript-power-area/v0.1",
  "nets": ["FX_XTAL", "FX_GPIO"],
  "maximum_domains": 2,
  "candidates_per_domain": 4,
  "maximum_layer_alternatives": 2,
  "maximum_new_contacts": 8,
  "search_states": 4000,
  "allocation_states": 4096,
  "maximum_conflict_trials": 4,
  "maximum_branch_layers": 2,
  "branch_search_states": 4000,
  "step_nm": 125000,
  "detour_nm": 2000000
}
```

`external_boundaries` can identify a real external supply/return terminal with
`net`, `terminal: {component, pad}`, `role: source|return` and nonempty
`provenance`. It never invents a connector from a net name. `protected_corridors`
contain `id`, `layers` and integer `vertices_nm`. Authored foreign distribution
regions, sensitive-node exclusions and actual critical copper are also protected.
`partitionable_zone_ids` is an explicit exception for eligible power regions,
subject to the native preservation gate above. Default behavior protects them.

`electrical_models` use the `PowerElectricalModel` contract: named physical pad
ports with exact signed currents, fixed source voltages, load minima and named
local returns; layer thickness/resistivity/temperature/provenance; and resistance
for each actual plated span. Decimal/fraction electrical values must be strings
or integers, not binary JSON floats. Regulator, package and capacitor model IDs
are declared interfaces, not automatically supplied dynamic models. Incomplete
models cannot produce a DC pass or outrank assessed candidates.

The extractor includes only complete mesh cells inside actual copper, subtracts
drilled voids and retains floating cells as unresolved obligations. It never uses
zone outlines as conductive material. A two-pitch native refinement requires an
explicit `refinement_tolerance_v` before reporting a refinement screen pass.
Both pitches share the configured per-mesh cell/iteration bounds; no convergence
loop or automatic allowance increase is used. Contact injection is resolved at
mesh pitch: narrow necks, merged terminals, absent via models and unsupported
native fragments remain unresolved. Even a converged DC screen is not AC,
thermal, current-density, assembly, CAM or prototype qualification.

## Research basis

- [TI PDN implementation and analysis](https://www.ti.com/lit/pdf/sprac76): early
  stackup planning, local loop inductance, DC and dynamic PDN assessment.
- [SPROUT](https://www.hajim.rochester.edu/ece/sites/friedman/papers/TCAD_SPROUT_22.pdf):
  graph-based board power routing with conductor growth/refinement and multilayer
  connections. Its electrical optimization is not supplied by mere polygon growth.
- [Hierarchical plane generation](https://arxiv.org/html/2210.16314v2): competing
  rail partitions, layer assignment and the limitations of sequential searches.
- [2025 graph-based PCB shape and stackup synthesis](https://ieeexplore.ieee.org/document/11169915):
  joint layer/shape refinement with DC resistance requirements.
- [Multilayer PCB rail routing](https://scholars.lib.ntu.edu.tw/entities/publication/c937d895-32fb-4456-9b55-c8d739b1f76c):
  demand-aware allocation of competing rail space and resistance-aware detours.
- [Infineon hardware guidance](https://www.infineon.com/assets/row/public/documents/24/42/infineon-hardware-design-guidelines-and-schematic-checklist-for-ez-usb-fx5-fx5n-fx10-fx20-applicationnotes-en.pdf):
  local supply/return access and pin-associated decoupling.
- [ADI LTC6268 demo](https://www.analog.com/media/en/technical-documentation/user-guides/dc2414af.pdf):
  topology-dependent copper geometry and its effects on input capacitance/stability.
