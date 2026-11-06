# Rust placement and routing closure plan

## Latest-main integration and publication, 2026-11-06

Routing changes are committed as `ad6dd53`; tested merged code is
`438ae037e8d2d78d4b9465b4e30ebba50c86f9fb`, containing remote main
`2adc450841b5762bf709c68cdd1ba443a7308b49`. Main's thermal-relief parameters,
correct KiCad pad-zone numbering, rendering command and example changes are
retained. The consumer pin selects that tested code revision; this validation
entry changes documentation only.

The fresh standard CopperProbe case is `build/p27-main-publish/probe`, with
the consumer's bounded candidate-01 profile and fresh placement. It retains
**237/237 signals, zero native missing connections, zero native other
violations/islands and zero hard geometry errors**. All 43 mandatory inner-ball
sites and exact admitted copper occurrences remain. Maximum length,
differential skew and the unchanged 12.7 mm ADC match query have no findings.
There are 2,520 tracks, 346 vias and 2827.680 mm of copper, matching P26.
Runtime is 1055.1 s under concurrent workspace compilation and other ladder
jobs; P26's 636.0 s reused placement, so that timing is not a like-for-like
performance comparison. The fresh run remains below the plan's 30 min target.

The other nine cases in `build/p27-main-publish/ladder` preserve completion,
hard-DRC counts and all copper metrics. CopperLedRing (51/51) and
CopperAssetTracker (124/124) remain native-clean. Round LED and FX10 bridge
retain their existing one and four opens. Loaded round LED/nRF52 timing flags
do not reproduce in serial controls on the same current sources: round LED
11.5 s before / 10.6 s after; warmed nRF52 4.6 s before / 4.9 s after. The
initial old-binary nRF52 pass was 36.4 s before warming. No speedup is claimed.
Controls are `timing-before`, `timing-before-warm` and `timing-after` beneath
the same build parent; their labels identify frozen old/new binaries rather
than relying on the runner's current-checkout commit field.

Workspace release tests: **1407 passed, zero failed, six ignored**, across
137 test targets. Formatting and all-target Clippy with warnings denied pass.
The consumer's ERC (two existing warnings) and public ADC/clock/macro test
pass against the merged binary. Build profile is release, LTO off, eight
codegen units. Executable SHA-256:
`88FCE3B9E5F7AB77708772FB37880B9B228FDABA4C31F90AEFBF8B6C2E706822`.
Final native PCB SHA-256:
`1CEBCD8DBFAF5E4440AC000721308ECDF634DD0D55BFA336451F4D11EE85EC02`,
at `build/pf-nat/p27/native-cleanup/round-003/CopperProbe.kicad_pcb`.

Consumer board/macro hashes remain P26's
`E4D9BCF1762C7054840D822ED86F24383BB9E6A29C92D08F42DBA2633C5551E5` /
`5BCFCA742F2AAA3EEDACFB2417D988256D9A3822A5D794263A7D2979AD4D2F1E`.
The requirement summary retains 30 blocked/incomplete verifier checks and
`fabrication_ready = false`; electrical, assembly and CAM qualification remain
open. Earlier measurements below retain their original bindings and scope.

## Optional escape retirement and contact trims, 2026-11-06

Unmerged follow-up after P22, with unchanged board, macro and routing limits:

| Candidate | Scope | Full native missing / dangling | Required sites |
| --- | --- | --- | --- |
| P22 | Previous control | 0 / 17 | 43, retained |
| P23 | Fresh optional-prefix and dependency admission | 0 / 10 | 43, retained |
| P24 | Exact existing contact trims | 0 / 7 | 43, retained |
| P25 | Joint owned via/exposed-leg retirement | **0 / 0**, native clean | **43, retained** |

P23 (`build/p23-ordinary-retirement`) completes in 1,340.1 s with 237/237
signals, zero hard geometry errors/islands, 353 vias and 2,854.297 mm copper.
P24 (`build/p24-contact-trim`) completes in 1,429.8 s with the same connection,
geometry and site results, 353 vias and 2,853.105 mm copper. Timings include
concurrent compilation/benchmark work and do not establish a speed change.
Normal case manifests bind the frozen compilers and all changed Rust files.

**Final P25** (`build/p25-exposed-leg-retirement`) completes in 1,275.0 s with
237/237 signal connections, zero native missing connections, zero native
violations/islands and zero hard geometry errors. KiCad 10.0.6 passes. All 43
mandatory sites and their exact copper remain, and ADC matching passes the
unchanged 12.7 mm limit. There are 2,543 tracks, 346 vias and 2,850.455 mm copper:
11 fewer vias and 5.867 mm less track copper than P22. Cleanup uses three native
rounds: mutable tails commit, the standalone via trial is rejected, then the
combined owned via/leg candidate commits. Fifteen old track occurrences are
replaced by nine, and 11 unused ordinary vias are removed. The source board,
macro, limits and frozen Rust file bindings still match.

Final compiler SHA-256:
`6451B220F5898FC824DCF9D22D60BC3ADF81D8322F6AA2721894F5C204B5637B`.
Final native PCB SHA-256:
`6531D854CAFBA4D7C7DA99E19A7EF1E7042445866CF4E20F288A61B6BC6F1F30`,
under `build/pf-nat/p25/native-cleanup/round-003`. All 17 prior native findings
are gone; no connection regresses. Per-net copper lengths mostly shrink;
ADC_D2/D3 each grow 0.041650 mm. These and the before/after via counts are in
the normal case's `per-net-comparison.json`. Three existing maximum-length
warnings remain: SENSE 9.873107/4 mm, TX1M 8.204671/8 mm and TX1P 8.141204/8 mm.
Native-clean connectivity does not waive these or establish electrical signoff.

Optional retirement now renews ownership from the original clean source, never
from a routed board relabelled as source. Only uniquely owned ordinary-prefix
occurrences may change; replacements remain same-net/layer/width subsegments.
Source, macros, mandatory lattice sites and required-array multiplicity stay
exact. Caller guard omissions do not waive mandatory-prefix protection.

The remaining useful ADC_D1/D5/SDATA legs required trims rather than deletion.
Integral via projections need full endpoint-disk containment. Width contacts
need a retained inscribed neck at least as wide as the board minimum. ADC_D5's
existing 0.15 mm F.Cu stub ends at x=80.000 mm beside a vertical launch at
x=80.050 mm: trimming that launch to y=22.750 mm keeps the 0.10 mm minimum neck
and adds no copper. Copper View now prints highlighted track positions/widths;
the aligned manufacturer comparison remains in the normal comparison outputs.

P25 keeps rejected dangling-only via candidates provisional while assessing
their newly native-reported owned legs. The complete delta and dependencies
are always re-proved against the incumbent, not the provisional board; no
intermediate candidate is adopted. Disposable, optional and exposed-leg trials
share the existing four-round native bound. Failed trials retain bounded located
findings, artifact bindings and `candidate_not_adopted` provenance.

Both P25 saved-board tests are native-clean, with zero missing connections,
violations or islands. Their exact typed state reloads and freshly validates
46 cross-layer supply/return witnesses and 19 separate same-surface supply feeds.
The latter require non-increasing measured feed length and unchanged return
copper; they do not gain return-path qualification. Four associations remain
unqualified by this local checker (two immutable macro associations, one absent
explicit supply path and one absent explicit return path); global native
continuity does not upgrade that scope. The final native-clean candidate is in
`build/p25-exposed-leg-retirement/full-prefix-probe`.

All 1,397 default release workspace tests pass, zero fail; six opt-in tests are
excluded from that default run. The real-board retirement opt-in was additionally
run on the original 17-finding board and the seven-finding continuation, including
exact prefix/witness/feed state reload and fresh path verification. All-target
release Clippy with warnings denied and formatting/whitespace checks pass. All
nine other ladder boards have byte-identical PCB exports, identical copper
metrics and unchanged connection counts against P22. Both actual consumers stay
native-clean; the round LED example and FX10 bridge retain their existing one
and four signal opens. A loaded LedRing timing flag (118.5 to 162.2 s) does not
reproduce at that scale in serial controls: the first control takes 69.0 s,
then warm baseline runs take 10.5/10.5/10.7 s and warm candidate runs take
10.8/11.0/12.2 s. Warm medians are 10.5 versus 11.0 s, with the remaining small
difference and load/startup variation reported rather than a speed claim. See
`build/p25-led-timing-control/measurements.json`. Existing SENSE/TX1 maximum-length
warnings remain separate work; ADC matching is not eased.

## Native dead ends and retained launch handoff, 2026-11-06

The post-merge P20 census binds all 22 native dead ends: four mutable ADC
track tails, 17 retained ordinary-escape occurrences, and one mandatory
SWDIO lattice via. Producer attribution does not authorize retirement.
Independent disposable cleanup now uses complete native evidence, exact
ownership and pad partitions, strict native non-regression and length/skew/via
budget non-regression. A protected unrelated finding no longer vetoes safe
cleanup. Fresh partial improvements may commit while native DRC remains failed.
Before and final ownership censuses have separate board bindings.

Ordinary routing also receives freshly verified required ports at retained
inner-ball signal barrels, with every legal other signal layer available.
Joined pad-tree attachments cannot bypass the required port. The exact ports
survive typed unified-input replay. This is local launch consumption, not
package-boundary channel planning or field exclusion.

`build/p21-dangling-cleanup` completes the unchanged CopperProbe source with
237/237 signal connections, zero native missing connections, zero hard geometry
errors, zero islands, all 43 mandatory sites preserved, and 18 dangling findings
(seven tracks, 11 vias). All 25 eligible signal ports verify. Copper View confirms
SWDIO's original dogbone now carries onward In2.Cu copper: its track length falls
17.637 to 8.273 mm and its via count from three to two. ADC matching passes the
unchanged 12.7 mm limit; SENSE and connector-side TX1 maximum-length warnings
remain. Total copper is 2,856.372 mm with 357 vias, against 2,844.896 mm and
354 vias in P20. This is a tradeoff, not a universal length improvement:
D18 grows 13.12 mm, FX_V33 6.21 mm and ACQ_EN 4.29 mm. Per-net measurements
are in the normal case's `per-net-comparison.json`. The aligned manufacturer
comparison remains 169 pads, 180 degrees and zero fit RMS; the reference's
other board, net assignments and layer roles are not adopted as equivalent.

A saved-P20 native trial additionally verifies whole removal of the redundant
ADC_D9 overlap: four mutable tails can be removed, 22 findings become 18,
zero gaps remain, and no routing budget worsens. This overlap improvement is
newer than the P21 executable. Its full fresh-source run is P22.

**Final P22:** `build/p22-dangling-cleanup` verifies 237/237 signals, zero native
missing connections, zero hard geometry errors/islands, all 43 mandatory sites,
and **17 dead ends: six tracks and 11 vias**. Every final census row is freshly
bound and protected ordinary-escape copper. ADC matching still passes. Copper
is 2,544 tracks, 357 vias and 2,856.322 mm. The final cleanup removes five
unprotected track occurrences after feedback; its native result remains failed
because of the protected findings. Wall time is 1,207.1 s under concurrent load;
this is not a speed comparison. Frozen compiler SHA-256:
`DCC7D4A6D95EF246F276AE3A21172D228A128014137F162FF7DD67C86E25A560`.
Fresh final native PCB SHA-256:
`2FB8C4EB78EA90BCCAE8C311AAAC653E2BFE5841141C0303B8B0111712E4C9B2`,
under `build/pf-nat/p22/native-cleanup/round-001`. Source, macro and all measured
Rust file hashes still match the frozen input manifest.

Dangling counts improve on ADC_D10 (1 to 0), ADC_D2 (2 to 0), ADC_D3 (2 to 1),
ADC_D9, CC1, SWDIO and XTALIN (each 1 to 0). Three newly unused retained track
legs appear on ADC_SDATA, CC2 and XTALOUT (each 0 to 1). These are finding
migrations, not new missing connections. Exact per-net before/after lengths,
via counts and dangling counts are in P22's `per-net-comparison.json`.

All nine other final ladder cases retain their control connection counts,
zero hard errors and identical copper counts/lengths. Both CopperLedRing and
CopperAssetTracker remain native-clean and their PCB exports are byte-identical
to P20. Two example exports have changed model-library metadata; their copper
metrics are unchanged. The existing round LED and FX10 bridge opens remain one
and four. The actual ordinary-input replay retains all 76 nets' detailed length,
track and via counts and SWDIO's new route. It marks three distribution-zone
nets connected where the main pipeline marks them deferred, so its status
fingerprint is not asserted equivalent to full native pipeline completion.

Final verification: 1,391 default workspace tests pass, zero fail, five remain
opt-in in the default run (`build/p22-workspace-tests.log`). Both real-board
cleanup opt-ins were additionally executed with fresh KiCad: disposable cleanup
on P20 and optional-prefix feasibility on P21. The final routing/report unit
suites pass 527 and 48 tests. Workspace all-target release Clippy with warnings
denied, formatting and whitespace checks pass. Builds/tests use locked offline
dependencies, release with LTO off and eight codegen units, matching the routing
comparison profile. CopperProbe still has only its 229 pre-existing archive
deletions; authored consumer sources were not changed by this work.

The strict optional-prefix feasibility trial on P21 tests every remaining net.
Eight findings can be removed/trimmed together: 18 become 10 with zero native
gaps or islands and no budget regression. The candidate is **not adopted**:
three original ordinary-prefix tracks and four vias change. It still needs a
replacement admitted prefix and fresh dependency validation. ADC_D1, ADC_D5
and ADC_SDATA whole-removal/trim proposals lose explicit pad partitions;
ADC_CLKM, VBUS, PWR/SW and NEG_FB trials introduce native violations. Their
failures remain explicit rather than being waived by connected-net status.
The actual FX10 windows and aligned reference overlays are retained in the
normal P21 case. No board, macro, placement constraint or routing limit changed.

The remaining work is two distinct transactions. First, admit the replacement
optional prefix from its original clean source, preserving every mandatory
lattice/macro/required-array occurrence, then freshly validate all affected
supply/return dependencies before accepting the feasible subset. Native
connectivity alone is insufficient. Second, repair useful launch tails rather
than deleting their connecting segments: ADC_D1's F.Cu centreline is x=83.100 mm
beside a via at (83.250, 22.875) mm, but continues to y=23.300 mm; ADC_SDATA's
x=79.750 mm launch passes a via at (79.625, 12.000) mm. The aligned kit provides
completed via exits, whereas these generated launches continue beyond their
off-centre land contacts. An exact, width-aware contact trim needs its own
partition/native checks and renewed ownership; a guessed annulus cut is not
authorized. The seven residual via findings additionally need their exposed
launch legs assessed together, since bare via removal introduces new findings.

## Main integration, 2026-11-06

Merged upstream main `dbdd016a903cf3d6f9a89968e33b8a758fa4529b` with the
accumulated routing work. Bus tuning preserves both upstream's planned arched
track lengths and via-depth accounting. A combined regression checks actual
arched copper, connected endpoints, unchanged via contacts and final DRC
length matching. The antenna-open-end field is explicit in the native fixture.

The release workspace run accounts for 1,385 default tests after targeted
reruns. Loopback, temporary Git and three checkpoint-move cases pass with the
required access; the new arc/via fixture passes after correcting its target-pad
position. The other 1,379 tests passed in the full run. Formatting and workspace
all-target Clippy with warnings denied pass, including a final check of the
changed test module. Two optional tests also pass: the native KiCad bridge
including its changed-copper negative control, and exact round-trip of the
fresh actual placement checkpoint. Two optional tests remain unrun because
their specific final/ordinary/critical or unified saved inputs were not
supplied. Details are in normal `build/merge-main-*` test logs and
`build/p20-main-merge/merge-validation.json`.

All nine other ladder cases finish in `build/p20-main-merge-ladder`.
CopperLedRing, CopperAssetTracker, CM4, nRF52, full-vertical and both browser
cases have zero signal opens and zero hard geometry errors. The round LED
example retains one open; FX10 bridge retains four. Consumer copper lengths
and via counts match the preceding control. Tests, compilation and routing
ran concurrently, so timings do not establish a performance change.

The same bounded CopperProbe entry and board as `p19-cc2-rotation` finishes
in `build/p20-main-merge` with 237/237 signal connections, zero native missing
connections, zero hard geometry errors and zero isolated-copper findings.
There are 2,566 tracks, 354 vias, no arcs and 2,844.896 mm of track copper,
matching the accepted pre-merge copper counts and length. Native DRC still
reports nine dangling tracks and thirteen dangling vias; the existing ADC
match and SENSE/TX1 length warnings remain. Wall time is 1,224.2 s.
Fresh KiCad evidence is `build/pf-nat/p20/verification-001`, PCB SHA-256
`dd95b4074b92608fb0f55e7f5569a091148ef2dc74d0ac142c07c6af3463db17`.
The Rust executable SHA-256 is
`DB46DF900F2D6AB4ECC1A60999C27C7A6066DDA35667C812A0817A5AC99A3F53`
(release, LTO off, eight codegen units). Exact board/macro/main bindings are
in the standard case's `input-binding.json`. Connection closure remains
separate from native DRC completion and electrical qualification.

### Native gap repair and joint routing: zero gaps, quality unfinished

Follow-up: the generic native bridge fix closes D18 on the unchanged C7 source,
reducing two native gaps to one, with 24 dangling findings, all 43 required
dogbones retained and no other per-net native or length-review regression.
Inferred fill grouping had merged contacts that KiCad reported disconnected;
proposal generation consequently stopped with `no_eligible_surface_bridge`.
For such a native pair the bridge planner now uses separate bare-copper roots
to find owned anchors. Exact commit and fresh native acceptance are unchanged.
A physical regression fails before the fix and passes afterwards.

Moving only C_FX10_C3 to the aligned reference supply-pad position closes FX_P0
and D18, but introduces ADC_OVR: one native gap / 25 dangling findings. ADC_OVR
routes alone on saved inputs. The bus-first stage freezes its neighbours before
ADC_OVR can negotiate with them. `bus_bundles=true` connects ADC_OVR but regresses
FX_RESET and V33 in the bounded replay; it is not advanced. With only
`buses_first=false`, the full pipeline reaches **zero native gaps / 23 dangling
findings**, retaining all 43 dogbones. That full source/configuration run uses
the frozen 5BBC Rust compiler. The new production compiler C282 repeats the
complete configuration at **zero native gaps / 23 dangling findings**, with a
byte-identical final PCB. All 43 required inner dogbones and admitted prefix
occurrences remain preserved. No source budgets are relaxed.

Connectivity is improved but quality is unfinished: the all-joint candidate's
ADC copper skew is 24.98 mm against 5 mm, versus 9.36 mm in the two-gap control.
Several ADC lengths exceed 35 mm. `length_priority=true` keeps replay
connectivity but does not improve overall lengths. SENSE remains 9.87 mm against
4 mm; its five pad centres span 7.97 mm in Y. The next algorithmic experiment is
to negotiate from the initially short bus routes without freezing them; SENSE
needs passive-cluster placement review. No relaxed source budgets or promotion.

Release workspace tests pass 1,265, with all three prerequisite tests then
passing explicitly; Clippy and production build pass. All eight matched ladder
PCB exports are byte-identical to the control. Initial concurrent timing flags
on full-vertical/LedRing are retained; serial repeats report no regressions.
These repeats do not establish a speedup. Records remain in the existing root
under `c7-native-gap`, `c3-single-reference-route`, `c3-single-all-joint` and
`c3-joint-native-final`. The last is the combined production confirmation.
See that root's `native-gap-results.md` for the comparison and remaining limits.

The all-layer reference inspection finds no `VDDIO_CTRL` fill group in the
local C7/E3 window. The visible In4 regions have other net names. Accordingly,
the imported reference does not yet prove a power-plane join between these two
branches; an earlier working hypothesis of such a join must not guide a repair
without resolving this import/reference gap. New view legends list area net,
layer and fill-versus-outline counts, without inferring contact. Native issue
details now expose the feedback-global stop reason, diagnostic and board digest.

### Partial-forest routing: four-to-two native gaps, factorial complete

The blocked-seed hypothesis is reproduced by a new physical regression: a pad
enclosed by foreign copper on both layers prevents the old router from joining
three mutually reachable groups. The test fails on the old implementation.
The candidate continues with another terminal tree only after frontier
exhaustion; a missing-access group likewise no longer suppresses other joins.
Budget/deadline stops do not restart. Each join or new seed consumes a group,
so at most N-1 connection searches remain, with unchanged kernel/window caps.
Partial copper now participates in existing negotiated conflict checks; partial
ordinary incumbents prefer more joins before length. Exact commit still checks
each path and never marks these forests complete. Failure diagnostics identify
source groups/pads, and `issues` locates the blocked source as well as targets.
No board-specific selector, recovery stage or new routing option is added.

Four routing regressions pass, covering terminal-order rotation, missing access,
budget/deadline stops and partial-copper conflicts, plus a new issue-location
test. The full release correctness suite passes 1,262 tests, zero failures,
three prerequisite-dependent tests ignored; all three then pass explicitly,
including the native bridge with its changed-copper negative control and real
placement/unified checkpoint round trips. Workspace/all-target Clippy with
warnings denied and formatting checks pass. Correctness builds use LTO off /
codegen-units 8. The normal thin-LTO / codegen-units 1 production build also
passes; its FX_P0 replay PCB is byte-identical to the correctness-build replay.

The frozen candidate is `build/p17-physical-first-candidate/partial-forest/copper.exe`,
SHA F6F1E5B7304BBB366557C0E52518418BE863AC79D869E5BD057D68DB8C9C81C0;
its 463-file source manifest is CB04AED97C90325AEAA70A497425BF9C556E01F758FBC11BDF570375FBC430D7.
On the identical saved C7-case inputs, `replay-unified --only FX_P0` changes
zero committed new tracks/vias to 16 tracks/two vias (22.209462 mm). It retains
two disconnected routing forests and names C_FX10_C3.1 as the blocked source;
the net correctly remains incomplete. Whole-report replay open counts include
other unselected nets and are not the FX_P0 gap count. Full standard bounded
runs and the source/solver factorial are complete. Records stay in the existing
case directories. The production compiler is
`build/p17-physical-first-candidate/partial-forest/production/copper.exe`, SHA
5BBC4F82588EA2797749F1F456AACD432143F7F41C546648B25B49D17BFBC668.

A three-pose C3/E3 reference adaptation passes exact placement. C57=10 nF and
C58=100 nF are confirmed on schematic sheet 7. The new C3 supply pads are at
(78.720,21.5452)/(78.720,22.5152) mm, shifted 0.2572 mm west from the aligned
reference and using 0.97 mm pitch. E3's 100 nF moves/rotates into the adjacent
column, with its supply pad facing its barrel. Footprints, values, pin bindings,
macro, zones and routing rules are unchanged.

| Source / router | Native opens | Other native findings | Open nets |
| --- | ---: | ---: | --- |
| C7 control / old | 4 | 23 | FX_P0: 4 |
| C7 control / partial forest | 2 | 24* | FX_P0: 1; D18: 1 |
| C3/E3 repack / old | 2 | 26 | ADC_D5: 1; D18: 1 |
| C3/E3 repack / partial forest | 2 | 26 | ADC_D5: 1; D18: 1 |

All four primary arms use production thin-LTO / codegen-units 1 builds and
retain all 43 required inner sites and admitted source prefixes. Retaining
local dogbones does not establish complete escape channels to the boundary.
The combined repack/router PCB is byte-identical to the repack-only PCB: no
additive benefit in that source arm. The repack is not promoted because it
breaks previously connected ADC_D5 and leaves more dangling copper.

*The solver-only production run's original report records 190 other findings:
24 dangling track/via findings plus 166 disabled-library warnings. Two fresh
native checks of the exact export, rules and generated footprints from a short
project path confirm 2 opens / 24 other findings with no library warnings,
including a check using the normal project-directory working convention.
The original report remains intact. This isolates a directory-context problem;
general long-path support has not been fixed or qualified.

The best unchanged-source candidate improves FX_P0 from four native gaps to
one, while D18 regresses from zero to one. The remaining FX_P0 gap includes
C_FX10_C3.1; D18 has a distribution gap. Other 91-net census entries and the
ADC/SENSE length-budget reviews are unchanged. ADC_D5 in the repacked case
exhausts its frontier rather than reaching its search budget. The aligned
reference D2 exits on F.Cu with no local via, so another via is not yet a
reference-backed repair. Compare the support-bank surface corridor before
choosing the next placement or routing change. No active consumer promotion
or electrical/manufacturing qualification.

The consumer ladder options were stale relative to the current Makefiles:
AssetTracker requires `fine_regions=dense`, both consumers use one feedback
iteration/five router iterations, and LedRing uses managed footprint modules.
`bench/ladder.json` now reflects those settings. Fresh same-profile frozen-router
controls and candidate runs produce byte-identical PCBs on all eight ladder
entries. LedRing and AssetTracker pass native connectivity/DRC; existing
example failure statuses and the FX10 bridge's four signal opens remain
unchanged. Initial concurrent timings flagged four rungs. Fully serial repeats
of those four report no regressions, but large preparation/startup variation
precludes a speedup claim. Old consumer benchmark numbers are not current
controls. See the existing experiment root's `forest-results.md` and
`forest-factorial-comparison.json` for the completed comparisons and raw-report
limits.

### Reference-guided M10 launch and VDDD pocket: five native gaps

The matched four-arm source comparison is complete. All board runs use the
same frozen 8FF Rust compiler and bounded routing settings as the six-open
J3/M8-held control; no solver change is included in these source trials.

| Source arm | Native opens | Other native findings | D18 / VDDD / GPIO opens |
| --- | ---: | ---: | --- |
| J3 repack / M8 held | 6 | 21 | 2 / 3 / 1 |
| North In4 VDDD pocket only | 6 | 20 | 3 / 2 / 1 |
| M10 explicit source launch only | 7 | 21 | 3 / 3 / 1 |
| Launch and pocket together | **5** | 21 | 3 / 1 / 1 |

The manufacturer board fits all 169 package pads with 180-degree rotation and
zero RMS residual. Its M10 via is off-centre and feeds the reference's split
power distribution. In our saved control the adjacent sites contain FX_V33
and GND vias, while M10 remains pending `sites taken`. The source adaptation
adds a 0.1 mm F.Cu neck and a standard host 0.45/0.20 mm through via at
(86.075,14.925 mm), plus a B.Cu public port. All original 112 macro tracks,
two vias, 12 ports and protected regions remain unchanged. The small priority-2
VDDD pocket carves In4 locally; continuous In1 ground intent is unchanged.
The reference uses smaller 0.4064/0.2032 mm vias and a different family member
(4014 versus 4013), so this is an explicitly experimental host-rule adaptation.
The current native reference import is 28BAA; equivalence to the historical
0048 import in the old macro provenance is not established (363 versus 334
imported footprints, matching track/via/zone totals). Allegro via spans in the
import are hardcoded F.Cu/B.Cu; this is not a repeated source-padstack audit.
See `build/reference-inputs/fx10-source-import-provenance.json` for those limits.

All three completed variants retain all 43 required inner dogbones through the
final board. M10 becomes a verified source launch in both launch variants.
Together the launch and pocket close the M10/L11 and north distribution gaps;
only C_FX_F9 remains disconnected on VDDD. D18 regresses by one native gap,
and F7/GPIO is unchanged. ADC and SENSE length reviews are unchanged across
these three arms, including 9.36 mm stored ADC copper-skew against 5 mm.
This is a connectivity improvement, not a qualified board or a generic
escape-aware support-placement implementation.

An ideal cell-centre M10 source-launch control also passes placement/macro
geometry. Furthermore, `sites taken` is emitted only when M10 had nonempty
exactly checked candidates but the joint flow assigned none to it. This is
allocation competition, not evidence that every initial cell failed geometry.
Which cells and competing ball assignments caused this omission still needs
an exact allocation audit. Grid resolution or physical infeasibility alone
does not explain it. Do not infer via source ownership from proximity.

Native per-layer views now list visible via net, centre, diameter, drill and
layer span, prioritizing the highlighted net while retaining foreign via
context. This observational edit changes only `copper-report/src/view.rs`
relative to the frozen 462-file B182 manifest. Report/view tests, CLI integration,
Clippy and standard FX10 bridge parity pass; saved SVG geometry is byte-equal.
The diagnostic C43 compiler is archived separately and is not used to route
these source trials. This view edit is included in the subsequently completed
full-workspace test run described above.

Records remain under the existing
`build/p17-physical-first-comparison/reference-support-placement/courtyard-repack/`:
`m10-factorial-summary.json`, each arm's 91-net `connection-comparison.json`,
source/checkpoints and aligned reference views. A local F9 In4 pocket finishes
at 5 opens / 20 other findings, retaining all 43 dogbones, but leaves F9's
capacitor disconnected. The final local view has no VDDD tap at that cap.
The reference also has B.Cu supply islands, but adding B.Cu to this same host
pocket is rejected after full native verification: **9 opens / 21 other**,
all 43 dogbones retained. VDDD becomes native-connected, while D18 regresses
3->5 and GPIO 1->4. A rectangle that closes one rail can obstruct neighbouring
distribution; reproducing reference islands requires joint rail/support-space
planning under host rules, not simply enabling another pour layer.

The initial reference-style C7 pair fails host courtyard clearance against F9;
the 0.03 mm south adaptation passes placement and completes full routing/native
at **4 opens / 23 other findings**, retaining all 43 dogbones. Relative to the
five-open combined M10 control, D18 3->0, VDDD 1->0 and GPIO 1->0 improve;
FX_P0 0->4 regresses. ADC_RESET gains one dangling track and FX_VBUS one dangling
via; ADC and SENSE budget reviews are unchanged. This is a mixed candidate,
not a clean replacement for the control. The four new P0 gaps are all on one
rail, including its bulk/filter feed and package/support fragments.
The official schematic sheet 7 resolves the exact values: C52 is 100 nF, C46
10 nF. The new pair keeps those value-to-position associations, with 0.03 mm
south displacement and the 10 nF pad 0.0455 mm farther outward to give the host
0.97 mm pitch. C9 moves into the vacated old C7 pose. Both accepted and rejected
placement outcomes remain visible. Existing consumer sources are not promoted.

The new P0 failure exposes a generic partial-routing fault: ordinary
`route_net_once` always seeds `components[0]`, then returns immediately when
that tree cannot reach any remaining group. Here the source is the isolated
C_FX10_C3 support; search exhausts after 114 expansions without hitting its
budget, leaving four groups unjoined. The other groups are not independently
joined after that failure. A generic forest/alternate viable seed could retain
more useful partial copper, but is not implemented or measured in this trial.
The reference comparison also shows a placement repair to investigate: its
C3 support pads are below the C3 barrel at aligned (78.9774,21.545) and
(78.9774,22.467) mm; our isolated 10 nF pad is at (77.020,19.000), across other
rail/ground tracks. Current E3 placement conflicts with a literal reference
C3 pair, so the repair needs a joint C3/E3 support check. Before/after views
are `c3-reference-pin.svg` and `c3-boxed-support-reference.svg` in the existing
comparison directory. No power-net remapping or width-budget relaxation is
proposed. Source-group identity should also be included in search diagnostics;
the current failure lists remaining targets and access counts, hiding the seed.

### Dense-cap spacing and reference-style J3 trial: completed mixed improvement

The extra 0.2 mm is an authored routing-space allowance beyond the locked
courtyard, not an established selected-MLCC/assembler requirement. The locked
0402 has a 1.82 x 0.92 mm courtyard with 0.15 mm land-envelope excess. KiCad's
small-part courtyard guidance and JLCPCB's adjacent-0402 spacing guidance do not
establish a need for a further blanket 0.2 mm. This does not qualify assembly.
The explicit source trials retain footprints and copper rules unchanged.

A J3 bank at 0.97 mm pitch, near the aligned reference's 0.9222 mm, passes exact
placement and native placement DRC without courtyard/clearance findings. Its
100 nF supply pad matches the reference position and its 10 nF pad is shifted
0.0478 mm south. E3 and M2 supports are repacked; a back-side placement-only
keepout protects the local J3 dogbone/onward pocket. Current checkpoint copper
shows the shared J3-to-capacitors feed. Full routing/native trials are complete.

Removing the extra margin alone is insufficient: movable C_FX10_M8 entered the
inner field and caused 41/43 coverage in the zero-gap arm and 40/43 in the J3
arm. Those arms were rejected early. Holding M8 at the fresh original-gap
control's outer-bank pose (88.25,18.75 mm, back, 90 degrees) restores final
43/43 coverage and exact retained-prefix preservation in both revised arms.
The fresh original-gap, zero-gap/M8-held and J3-repack/M8-held runs finish at
8/21, 11/24 and 6/21 native opens/other findings respectively. The repack closes
ADC_D1, ADC_OVR and FX_V33, but increases FX_VDDD two to three native gaps.
Its six remaining gaps are D18 two, FX_VDDD three and FX_GPIO one. The 21 other
findings redistribute across nets. ADC_D1 still exceeds 35 mm (39.25 mm), other
ADC overlengths remain, and the saved ADC copper-skew review is 9.36 mm against
5 mm. Keep this as a connectivity candidate, not an unconditional board winner.
This is a source experiment, not a generic
joint Rust support placer or boundary-channel proof. Matched routing budgets
disable placement refinement/feedback in all fresh controls. The older TX2
13/25 report is historical context, not an identical placement-search control.
See `build/p17-physical-first-comparison/reference-support-placement/courtyard-repack/results.md`
and its 91-net public projections. The current active consumer has not been
replaced with this unqualified candidate. No Rust solver/library source changed.

### Current diagnostic revision: final verification passed

The four observational diagnostic files are now active. Native `issues --net`
counts, locations, objects and history come from matching final findings, while
unfiltered output remains unchanged. Connected fallback offers now retain both
lengths, skew, warnings, comparison ranks and keep/reject reasons, including a
last-kept snapshot after the 16-row trace cap. Scope excludes lower producer
proposals and distinguishes later outcomes. The mandatory-first allocator draft
remains isolated and unmeasured; it is not included in this revision.

The final compiled-input manifest is frozen at
`build/p17-physical-first-candidate/scoped-diagnostics/final/source-manifest.json`,
SHA-256 `b182a147c0a06588c99891691c34251f3688268a0a68f475615f1dfb22bd5bbc`
(462 files), with zero drift in the final audit. The normal release compiler
(thin LTO, codegen-units 1) is `scoped-diagnostics/final/copper.exe` beneath the
same candidate directory, SHA-256
`8ff25779d2b2905e51ffaa4318b301696883e097c0dbe3072d06e7610ca22349`.
Formatting and all-workspace/all-target Clippy with warnings denied pass.
Minimal lint fixes preserve short-circuit order, ownership, search values and
diagnostics; no lint suppressions were added.

The final full workspace correctness run passed 1,256 tests with zero failures
and three prerequisite-dependent tests ignored. It used release with LTO off,
codegen-units 8, no filters and permitted localhost access. All three ignored
checks were then run explicitly and passed: native KiCad bridge (including its
changed-copper negative control), saved placement roundtrip and actual unified
checkpoint roundtrip. These are correctness results; the normal production
build passed separately. Earlier socket-denied and interrupted runs are not
counted as passes.

Test-only fixes project the original eight escape golden fields against the
unchanged fixture and isolate concurrent source fixtures by process and call.
Only the two scratch-fixture test files differ from the prior BB1275 compiler's
compiled inputs; production sources are identical. The final compiler reproduces
the B705 and BB1275 standard FX10 bridge PCB bytes, all 46 per-net metrics and
all recorded quality/policy checks. That bridge entry does not run native DRC.
On both saved CopperProbe reports, unfiltered native issue JSON is exactly equal
to B705 while corrected CC1/XTALIN/XTALOUT per-net counts pass. Records are
`final/native-cli-parity.json`, `final/final-source-audit.json`,
`build/p17-scoped-diagnostics-final-workspace.status.json`,
`build/p17-prerequisite-checks-status.json` and
`build/p17-ladder-scoped-diagnostics-final-on/fx10-bridge-parity.json`.

This revision does not implement full inner-field exclusion, verified boundary
channel reservation or early qualified power islands. All 43 connected inner
ball obligations are preserved in the CopperProbe trials below; that is local
dogbone retention, not proof that every ball has a legal route to the package
boundary. The isolated mandatory-first allocator has 14 passing targeted tests
but still needs its bounded board comparison before promotion.

### TX2 board-budget trial: local success, power regression remains

The completed B705 source-only trial redistributes 55.6+16 mm to 51.6+20 mm.
Its regenerated placement/topology and global-guide fingerprints exactly match
the control, establishing controlled attribution after fresh placement. TX2
selects 19.562545/19.499078 mm, skew 63,467 nm against 63,500 nm, with no warnings.
The existing cross-cap mapping gives chain tracks of 66.717878/66.717877 mm
(1 nm difference). The declared 71.6 mm track allocation and 76.132 mm PCB bound
are unchanged. USB2 is 6.944734 mm longer per member and uses two rather than
four group vias; the other five selected pairs are unchanged.

All 43 mandatory inner sites and all 254/99 prefix track/via occurrences remain.
Whole-board native results are still unqualified: gaps 12 to 13, other findings
26 to 25. The sole additional gap is FX_VDDD 4 to 5 near M10/L11. BOOT loses one
dangling via; dangling tracks redistribute across ADC_RESET, BOOT, XTALIN and
XTALOUT. All 91 per-net counts sum to the global report. See the trial's
`final-comparison.md` and `final-connection-comparison.json`.

The new gap names C_FX_L11.1 at (85.811,14.085) mm against the L11 front stem.
The control has a B.Cu lead from this capacitor to C_FX_M10.1; the trial does not.
That difference already exists in the exported board and both native snapshots,
not just a later refill. Both unified inputs lack this onward feed, and the exact
retained source/ordinary/lattice copper-only prefix is identical across arms.
This is not a deleted retained dogbone or proof of losing a previously qualified
powered package terminal. The manufacturer reference shares an In4 PWR1 pocket
between L11/M10 and feeds its outer capacitor bank; our source retains GND on
In4, merely allows FX_VDDD there, and declares no FX_VDDD distribution zone.
Accepted-site power generation needs explicit domain ownership and early
integration rather than enabling the old island option blindly. The aligned
audit and views are retained under the existing trial's `fresh-placement-run`.

### Scoped retention and terminal direction: measured mixed result

The follow-up source is promoted and frozen in
`build/p17-physical-first-candidate/scoped-terminal`, executable SHA-256
`b70556078f7d221066a6866e13723ca4cd3e0218e0c9c584ed86450681d600be`.
It scopes the experimental retention policy to censused mandatory inner-ball-grid
obligations, preserves legacy ordinary-fanout cleanup on non-applicable boards,
and reports incomplete or unsupported census explicitly. Exact original-witness
and admitted-occurrence checks remain strict. A separate correction uses each
selected terminal's actual component origin when constructing paired outward
normals; search quotas and physical design rules are unchanged.

Before promotion, 497 routing library tests passed (two ignored), 38 report
library tests passed, the CLI checked, and formatting/whitespace checks passed.
The corrected cleanup fixture uses legal distinct drills and still tests duplicate
track occurrences. Five terminal-orientation tests include a physical negative
control for the old direction under unchanged search quotas.

The optimized same-source/placement CopperProbe run completed on 2026-11-05 at
00:42 UTC. All 43 mandatory inner sites remain allocated and every admitted
occurrence survives final cleanup: 254 tracks and 99 vias, zero missing. The
requested local policy is satisfied; it does not certify boundary escape.
Native gaps improve 13 to 12 (D18 two to one), while other native violations
regress 25 to 26 (13 dangling tracks and 13 dangling vias). Public per-net native
filtering initially establishes newly present CC1 and XTALIN dangling-track
membership, while XTALOUT loses that membership. The corrected diagnostic CLI
then verifies all 91 exact per-net native counts: CC1 0 to 1, XTALIN 0 to 1,
XTALOUT 1 to 0 dangling tracks, and D18 2 to 1 gaps. All other 87 nets have
identical per-code counts. Per-net sums match both global reports. See
`build/p17-physical-first-comparison/scoped-terminal-native-exact-count-comparison.json`.
No explicit net-connectivity status changes occur across
91 nets; 26 nets change whole-net copper metrics. The comparison is
`build/p17-physical-first-comparison/scoped-terminal-connection-comparison.json`.

Selected TX2 connector lengths improve from 29.076751/29.076750 mm to
18.057070/15.899520 mm, but skew regresses from 1 nm to 2.157550 mm against the
0.0635 mm limit. This is an unqualified mixed result, not a completed detour fix.
RX2 shortens about 4.13 mm per member and removes six group vias; RX1 lengthens
about 7.90 mm per member while staying within its authored budgets. The first
failed check in rejected-candidate summaries does not establish that a balanced
candidate was discarded. Preserve ranking pending a bounded full-candidate
selection ledger with both lengths, skew and comparison reasons.

The nine-board retention-ON ladder is complete, including all 287 public net
comparisons. Eight boards reproduce legacy OFF copper byte-for-byte; FX10
reproduces 93B, retaining all 49 inner sites. No explicit connectivity changes
occur. LedRing restores native 0/0 and AssetTracker restores 2/0, removing nine
and 23 dangling violations respectively. Both modem interior fields remain
explicitly outside the ball-grid census; geometry parity is not a qualification
waiver. Workspace correctness testing is still running. The default thin-LTO
workspace build was interrupted after one hour of compilation and zero executed
tests: Cargo repeats LTO over 113 workspace test targets. Its interrupted status
and log are preserved. The replacement uses the same `cargo test --workspace
--release --locked --offline --jobs 2` with explicit
`CARGO_PROFILE_RELEASE_LTO=off` and `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=8`.
Production routing measurements retain the frozen normal-release B705 binary;
the test-profile override is not production performance evidence. This revision
does not implement unresolved field protection, verified
boundary channels, early power-island integration or optional BGA fanout lifetimes.

A source-only TX2 constraint trial started with B705 and fresh placement on
2026-11-05 around 00:49 UTC. It reallocates 55.6+16 mm to 51.6+20 mm while
preserving the 71.6 mm track bound, four allowed through vias at 1.133 mm upper
thickness and 76.132 mm PCB bound. Via/skew limits are unchanged. Routing-rule
changes invalidate placement reuse in the current CLI, so results cannot isolate
the budget split from fresh placement. The old selected skew is still invalid;
no improvement is assumed. The isolated source and normal command are under
`build/p17-physical-first-comparison/tx2-budget-repartition`.
Its critical stage accepts a TX2 repair with both the target and displaced USB2
group within budgets; final selected paths/native results remain pending. All
43 local inner sites allocate and before/after-critical observations lose no
local escape or bypass dependency. Compare public global placement and guide
fingerprints before attributing any final improvement solely to the budget split.

Four observational diagnostic files are prepared for promotion after the B705
workspace suite finishes. The fallback ledger records both member lengths,
skew, all available warnings, the exact existing ranking tuple and keep reason.
Its first 16 trace rows and omitted count are supplemented by a full last-kept
offer, even beyond the cap; later group outcome remains separate. It covers
freshly validated connected fallback offers, not every lower producer or early
successful route. Three focused route tests verify those bounds and unchanged
physical output/fingerprint. All 42 report library tests pass. The copied CLI
builds, and its unfiltered native output is byte-identical to the old CLI on
each of the two saved inputs. Net filtering now rebuilds counts/locations/objects
from whole matching final findings and filters history before clipping it. Stable
issue IDs still select the global group in the viewer; use the filtered window
for a focused view. Both diagnostic reviews are clear; root source is unchanged.

The next allocator audit found equal source-edge cost for mandatory inner balls
and optional outer supply balls. A contested site can therefore go to optional
coverage while an inner obligation reports `sites taken`. An isolated checked
dominant-penalty assignment and mandatory-first exact commit patch passes 14
targeted tests, including a 512-case independent small-graph oracle, contested
old-flow negative control and shared-rail via-budget priority. Root source is
unchanged; board improvement is unmeasured. A ball with no legal candidate stays
unresolved, and packages still allocate serially.

Field protection needs per-terminal runtime ownership before ordinary escapes
and decouplers are planned, not a late netless keepout. Distinguish a retained
local launch from a freshly verified path to an assigned boundary port. The
43/43 local-site count must not release the field. A permission for a particular
pad/candidate must not authorize unrelated same-net transit, especially GND and
shared supplies. Exact clearance enforcement and search/cache context must agree.
Power contacts require an actual declared same-net domain and fresh physical
contact; proposed islands and unions of unrelated zone domains are insufficient.

### Exact-witness revision: preservation verified, final routing unqualified

The optimized follow-up is frozen under
`build/p17-physical-first-candidate/exact-witness`, executable SHA-256
`93b704cc00a5d66af2b0ca4a8784fc87a31fecafe09023941a67edf7f7b4e094`.
On the same CopperProbe source and placement it strictly admits all 343 generated
bypass tracks and 22 vias that AB6F rejected; the same 12 originally exhausted
associations remain pending. Before/after-critical geometry checks lose no local
escape or dependency. The exact original-path regression passes in the optimized
unit executable, including replacement-path rejection, two original producer
batches, stale context, missing edge and source-rule changes.

Final output preserves every occurrence in the admitted frame: 254 tracks and
99 vias, zero missing. Those counts include the immutable source, ordinary local
escapes and lattice contributions; they are not counts of new inner dogbones.
All 43 allocated inner dogbones remain present on this partial board. Boundary
channels, unresolved field protection and power contacts are not certified.

This is not a final-routing improvement: native gaps/other violations change
from AB6F ON 10/17 to 93B ON 13/25. A18 closes, FX_VBUS opens; D18 and FX_VDDD
also gain gaps, while FX_GPIO partially improves. The complete public comparison
is `build/p17-physical-first-comparison/exact-witness-connection-comparison.json`
(182 CLI projections over 91 nets; 54 copper-metric changes).

The nine-board retention-ON ladder exposes a blocking lifetime error: LedRing
changes from native 0/0 to 0/9 (eight dangling vias, one dangling track), and
AssetTracker from 2/0 to 2/23 (23 dangling vias). Ordinary unused fanout was
turned into permanent hardware along with mandatory inner-ball launches. The
revision is unqualified. An isolated follow-up explicitly scopes this experimental
policy to actual mandatory inner-ball-grid obligations, reports requested/effective
policy and unsupported census separately, and leaves non-applicable ordinary
fanout on its legacy mutable path before admission. It does not waive proof or
solve optional escape lifetimes on actual BGA boards. That follow-up has now
been promoted and frozen for the optimized verification described above.

The full release suite was stopped while building after direct testing exposed
the known invalid cleanup fixture and native consumer benchmarks rejected the
revision. Its log and explicit interrupted status are retained; no full-suite
pass is asserted. The new cleanup regression initially
used coincident drills and was correctly rejected by fresh drill-spacing DRC;
the isolated correction uses distinct legal barrels and retains duplicate-track
occurrence checks. Report tests (32), formatting,
whitespace checks and real native-CLI checkpoint viewing pass. The view now shows
the previously hidden declared dogbone when Held is absent, with an explicit
view-only scope legend.

The reference-informed support trial has a fresh legal placement checkpoint with
the known M8 pose retained. Early allocation
regresses from 43/43 to 41/43 mandatory inner sites: GND U_USB.D3 has no
legal site and U_USB.L4 has its sites taken. A legal support placement therefore
does not establish legal escape capacity. This candidate cannot satisfy the
mandatory-inner contract; its final per-net/native comparison provides useful
negative evidence. Further manual pose nudges are not a substitute for joint
support/site/channel planning.

That support trial has now completed: native gaps/other violations improve from
13/25 to 7/19. FX_V33 (two gaps), FX_VBUS (two), ADC_D1 and ADC_OVR close, with
no new explicit/native connectivity regression. Remaining native gaps are
FX_VDDD (four), D18 (two) and FX_GPIO (one). All 14 selected USB route lengths,
via counts and connectivity remain unchanged, including the TX2 overlength.
The trial remains unready because its initial mandatory allocation is 41/43.
The final public comparison is the trial's `connection-comparison.json` (91 nets,
42 metric changes); longer totals on newly connected nets are not automatically
regressions. GND/V33 were explicitly partial but native-connected in both arms.

The long-detour audit identifies a separate generic terminal-preparation bug.
For the two separate TX2 capacitors, both `Collector::ports` and
`local_via_ports` compute the outward vector relative to only the first part's
origin. At the measured east-facing pads this gives (0.48, -0.75) mm instead of
the per-owner average (0.48, 0) mm. This biases the bounded recessed-via candidate
quota northward. A separate isolated draft threads each selected terminal's exact
owner and preserves same-component behavior, budgets and clearance checks. It
does not yet establish a legal route below 16 mm. The measured GND via near
(127.293375, 15.5) mm and USB_DM/DP copper beside the connector are also real
obstacles; snapshot immutability does not establish authored ownership or permit
their deletion. Broader portal-ranking changes remain deferred pending this
focused correction and measurement.

### P16 measured failure and P17 early physical ownership

P16 optimized Rust is frozen in `build/p16-local-prefix-candidate`, SHA-256
`6243f398bc572d9fb5da0a5148eebc8f49d52d4a0a63e32c6d4542b31d85fc1f`.
The fresh F672 nine-board control and P16 retention-OFF ladder have byte-identical
exported PCBs, physical board digests and quality metrics on every board. LedRing
remains native 0/0; AssetTracker remains 2 native gaps and no other native
violations. Timing overlaps differ and do not qualify a speed improvement.

The same-placement CopperProbe combined array/via OFF and ON arms both finish
with 9 native gaps, 10 other native violations, 2718 tracks, 345 vias and
2857.343350 mm of explicit track. All 91 public per-net metrics/statuses and the
physical board digest match. ON rejected physical admission at 278 seconds with
`stale_witness_context` and fell back; this is a negative admission experiment,
not successful reservation. The standard FX10 bridge ON case likewise rejected
all five admission attempts; its 46 per-net measurements and PCB bytes match OFF.
Standard bridge benchmarking does not include native verification.

The CopperProbe trace also locates an earlier concrete fault: the initial
decoupler batch adds a D18 via at (84.550001, 23.274999) mm that blocks ADC_D8's
previously legal F.Cu stem from (84.25, 22) to (84.25, 23.3) mm. Thus this arm
loses an escape before critical routing. The earlier SWDIO/ground example was
a different measured configuration; neither observation establishes every open
net's cause. Planning decouplers against proxies and adding actual escape copper
only before critical routing is too late.

P17 now places real ordinary and lattice copper in the decoupler planning frame
when retention is requested, before the normal bounded decoupler candidate search.
This mode keeps those reservations unchanged and does not use flow-level
mask-release/displacement retries. Only explicitly created bypass suffixes belong
to Held. Raw producer batches retain exact source, temporary-mask occurrences,
copper ownership and original proof context; final binding freshly proves the
same associations. If a Held proposal fails, it cannot discard an independently
admitted fixed prefix or reappear through a late join. Initial admission failure
is explicitly unresolved and is not reported as retained copper.

The active allocator now reports every connected ball on recognized grids,
requires inner dogbones independently of abstract surface hints, and qualifies
rule widths/breakouts, spans, budgets and existing returns. Unsupported coupled
launches, missing returns and source proofs remain pending. Array flow and site
allocation share source power/return classification. Allocated ranges locate
actual pieces/barrels, and the power-island consumer freshly validates them.
These remain local-site proofs, not boundary-channel certificates.

P17 optimized Rust is frozen with executable SHA-256
`ab6f821f704ca55dd570794da7b89372a91defeaa5469e2851a4aa893aa5c691`.
It passes 53 focused copied-debug tests, 487 broader copied routing unit tests
(2 ignored), 48 release integration tests and active all-target release checking.
The full workspace release suite was interrupted during compilation when its tool
runtime ended; it has no passing result. The two CopperProbe arms were likewise
interrupted during critical routing and restarted with the same frozen executable,
source and placement; interrupted logs remain alongside their restarted logs.
All nine OFF ladder cases have now completed: eight retain P16 PCB bytes and
public quality/budget measurements, including native 0/0 LedRing and the existing
2/0 AssetTracker result. FX10 alone changes, with the authored skew regressions
described below. The interrupted nrf52 case was resumed directly because the
native bench harness cannot reuse an existing case directory; its finished log
is `route.resumed.log`, and exact command/source provenance is retained. Timing
under overlapping compilation does not qualify a performance improvement.

The actual ON arm allocates all 43 connected inner balls, 58 new sites in total,
and freshly verifies two existing source launches. Five outer ground balls remain
unsited. It admits the complete local prefix, with no invalid escape or dependency
geometry before critical routing. It rejects the Held bypass suffix because some
declared generated copper is absent from the freshly selected witness; preserving
the prefix works, but this is not successful bypass retention or final closure.
Audit found a concrete mechanism: the handoff validates original producer paths,
then replaces them with newly searched paths, potentially orphaning their original
copper. An unmeasured follow-up preserves and freshly revalidates exact original
paths after explicitly owned temporary-mask removal. Same-surface provenance and
actual board-level suffix admission still require verification.

The initial all-ball/provenance work was developed in the existing isolated source
copy and then promoted; no original consumer or library source was changed. Field
protection, complete boundary channels and early power-island flow integration
remain unfinished. Existing field penalties are advisory; an ordinary-router-only
guard would also miss earlier critical and decoupler proposals.

The next revision also carries the typed admitted prefix through the pipeline and
accepted placement-feedback payload into final CLI cleanup. The previous
single-layer-via sweep protected source/macros/arrays only, so a General net that
joined elsewhere could lose its unused dogbone after routing. Exact occurrence
floors now protect the admitted copper, while ordinary unused vias remain eligible
for cleanup. A final occurrence audit is reported separately from native geometry
and prevents a missing prefix from being reported as complete. Its regression
includes duplicate owned stems/barrels and a disposable ordinary via. This change
is outside the frozen P17 comparison and awaits the resumed full release suite.

Checkpoint viewing now reconstructs declared fixed copper when Held is absent,
checks that the declaration's source equals the checkpoint board, and explicitly
labels its viewing-only scope. All 32 report unit tests pass, including loading a
saved checkpoint and checking the resulting escape trace and scope legend.

The field/boundary audit establishes the next algorithmic dependency: a package
remains pending boundary access after local sites are allocated. A runtime
reservation must identify exact pads/candidates rather than exempt an entire net;
the same exact policy must govern early decoupler, critical/pair and unified
proposal/commit paths. Owner-sensitive masks also require owner/policy epochs in
the current width/clearance-only open-cache key. Joint channels must account for
actual barrels, rule widths and breakout lengths before exposing boundary ports
to area routing. Power contacts require independently qualified plane/island
connections. These are implementation requirements, not implemented guarantees.

The frozen P17 same-placement CopperProbe runs have now completed native refill:

| Net | P16 combined control | P17 retention OFF | P17 retention ON |
| --- | ---: | ---: | ---: |
| A18 | 0 | 0 | 1 |
| ADC_D1 | 1 | 1 | 1 |
| ADC_OVR | 0 | 1 | 1 |
| D18 | 2 | 2 | 1 |
| F5_M | 2 | 2 | 0 |
| FX_GPIO | 1 | 2 | 2 |
| FX_V33 | 2 | 0 | 2 |
| FX_VDDD | 1 | 5 | 2 |
| **Native unconnected items** | **9** | **13** | **10** |
| **Other native violations** | **10** | **11** | **17** |

These are native gap items, not net counts. The P17 OFF/ON contrast holds the
all-ball allocator and shared supply classification constant. It improves exit
preservation at the first critical boundary (OFF loses ADC_D8/CC_OUT2 and four
bypass dependencies; ON loses none), but is not an overall qualified winner.
Both P17 arms are worse than P16 on the displayed final native totals. The final
single-layer sweep removed 13 OFF and 11 ON vias; no exact final-prefix audit was
available in that frozen compiler. The pending cleanup fix is therefore excluded
from any claim that every admitted occurrence survives the final output.

The public comparison contains 182 saved CLI projections over 91 nets: 65 change
copper metrics, none changes projection availability, F5_M improves explicit
connectivity, and A18/FX_V33 regress. Other native improvements include partial
FX_VDDD/D18 gains, which must not be reported as completed nets. The record is
`build/p17-physical-first-comparison/connection-comparison.json` and its native
per-net counts come from complete `copper issues --stage native` projections.

Issue N6 in P17 ON locates FX_V33's J3 dogbone to the back-side C_FX10_J3 pad:
U_USB.J3 (83.5, 20.5) mm to C_FX10_J3.1 (86.05, 19.3) mm. The original bounded
physical-prefix batch already left this link pending among 12 exhausted
supply/return associations, before all Held copper was rejected. Revalidating
original completed witnesses cannot alone solve those originally pending links.
The aligned native reference overlay is
`build/p17-physical-first-comparison/both-retain-on/fx-v33-reference.svg` (PNG
reviewed); its 169-pad fit is 180 degrees with zero RMS. The retained dogbone and
its In2 segment do not connect the back-side capacitor; accepted-site power
islands were not enabled in this run. Board/macro support and reference power
topology remain under investigation.

The long USB detour is not solved by local retention. Selected critical-route
lengths for the connector TX2 pair worsen from 20.889955/20.826488 mm OFF to
29.076751/29.076750 mm ON, both above the authored 16 mm limit. These remain
review issues (`DRC-MAX-LENGTH`), distinct from native connectivity. In contrast,
selected TX1 IC-side lengths improve from 51.934087/51.997555 mm to
39.987215/39.987215 mm with no signal transitions (the group's via count includes
reference returns). Saved whole-net copper lengths include retained/partial
stems and must not be substituted for these selected route lengths.

The standard P17 OFF FX10 example retains four explicit opens and zero exact DRC
errors but introduces two authored pair-skew violations (SSRX1 0.342008 mm and
SSTX1 0.942013 mm against 0.15 mm), absent in P16 OFF. All 46 public per-net
connectivity measurements match; 18 change copper metrics. Its shorter total
copper therefore does not establish improvement. This example does not run
native KiCad verification. Completed unaffected ladder boards, including native
0/0 LedRing, retain P16 PCB bytes and quality measurements.

The P17 retained standard FX10 arm does admit its local prefix (49/49 mandatory
inner balls), with zero invalid escapes/dependencies before and after its first
critical pass. Its small Held batch binds without rejection. The four explicit
opens remain; the two new OFF SSRX1/SSTX1 skew violations disappear ON. All 46
saved connectivity flags match OFF, while 11 nets change copper metrics. This is
an example-specific improvement, not native verification or complete boundaries.

The J3 reference audit confirms both support values (100 nF and 10 nF) on the
manufacturer schematic and pinned source contract. Reference J3's V3P3_FX barrel
contacts a filled In4/PWR1 pocket and short B.Cu support traces; L11/M10 similarly
contact a shared VDDD pocket. Net-highlighted reference-only views were necessary
to establish that rail identity; the all-net magenta overlay alone is insufficient.

Two bounded placement checks reject direct source adaptation, each in 8 seconds:
moving only the two J3 caps to aligned reference positions collides with fixed
C_FX_E3; freeing E3 within its existing region then exposes a J3-to-J3 collision.
The reference row pitch is 0.9222 mm, while the host courtyard height (0.92 mm)
plus authored 0.2 mm additional gap requires at least 1.12 mm. Stretching south
encounters C_FX_L3 and moving north encounters M2/L5 supports. These are actual
host assembly/placement constraints, not search-budget failures. The next source
revision must jointly repack the affected support cells using host spacing and
plan the filtered pocket/feed/return contacts; it must retain both capacitor
values and the USB macro. Neither rejected trial underwent routing. Commands,
source hashes and logs are under the existing comparison's
`reference-support-placement` and `reference-support-placement/joint-repack`.

Public comparisons are retained in
`build/p16-local-prefix-comparison/connection-comparison.json`,
`build/p16-ladder-current-off/comparison-to-f672.json` and
`build/p16-ladder-current-fx10-on/connection-comparison.json`.

### Required inner-ball contract and current experiment

Every connected inner ball must receive a legal owned dogbone, or retain an
existing freshly verified source-macro escape, before ordinary routing may use
the inner field. This includes signal, power and ground balls. The dogbones are
real retained copper through partial routing and final export. A failed later
route cannot release them; any necessary replacement is a fully validated atomic
replacement of the affected joint plan. Unresolved package fields remain locally
protected while unrelated routing continues. A local via site is not yet a
verified channel or boundary port.

The P16 experiment first tests the existing local-prefix integration using a
frozen optimized Rust compiler and exactly reused CopperProbe placement, with
retention OFF and ON. Its early critical checkpoint must establish whether
physical admission succeeded; fallback must not be reported as retained copper.
The existing benchmark ladder supplies the regression control. Whole-board
native connectivity and all per-net budgets remain the final comparison.

The allocator audit found silent exclusions for non-General critical signals,
abstract-selected surface balls and unsupported source cases; supply recognition
also differs between array flow and via allocation. Its pending-site count is
therefore not an all-ball readiness census. A separately isolated next revision
will account for all connected inner balls and qualify width/breakout, via-span,
budget and return requirements. Coupled launches and unproved source shapes
remain explicit unresolved cases. It is excluded from the P16 compiler being
measured. Complete channels, owner-aware field protection and early accepted-site
power integration remain unfinished.

### 2026-11-04 reference-style escape ownership correction

The current priority is joint inner-ball dogbones, legal boundary channels and
power contacts before general routing can consume that area. Earlier array-flow,
via-lattice and island features do not enforce this contract: array flow discarded
per-pad paths; lattice copper was represented to critical routing by netless
keepout proxies and joined afterwards; islands were generated after critical
routing. A real first-critical GND tap occupied an ordinary SWDIO escape's exact
via endpoint. The manufacturer overlay shows separate signal and ground dogbones.

The five-arm comparison in `build/p15-escape-reference-comparison` uses the same
frozen Rust F672 compiler, source, placement and routing bounds throughout:

| Arm | Native unconnected items | Other native violations |
| --- | ---: | ---: |
| Baseline | 4 | 5 |
| Array flow | 7 | 16 |
| Via lattice | 16 | 12 |
| Both | 9 | 10 |
| Both plus ball-grid power islands on In4.Cu | 10 | 10 |

Baseline native gaps are D18 (two), SWDIO and V33. Both closes SWDIO/V33 but
adds ADC_D1, F5_M, FX_GPIO, FX_V33 and FX_VDDD gaps. Adding islands reopens
CORE11 and adds a D18 gap. ADC_DCO and GND already close in this same-compiler
baseline, so their closure cannot be credited to islands. None is an accepted
improvement. Shorter partial routes are not length improvements.

New source retains abstract per-pad flow paths and boundary order, explicitly
physically unverified. Typed local-site prefix admission and critical replay have
focused copied-debug coverage (22 tests); full active release checking passed
before the subsequent flow integration. The flow now attempts actual physical
prefix admission behind the existing opt-in `retain_escape_copper` setting,
binds bypasses in that same frame, retains ordered occurrences and avoids joining
lattice copper twice. Failed fresh admission is explicitly reported and uses
the legacy conservative path; it is not a complete escape-first guarantee.
The coherent copied-source routing unit snapshot passed 464 tests (two ignored).
After the final combined-frame, context-provenance and power-site additions,
37 focused tests passed: three flow tests, eleven original-pad ledger tests,
four accepted power-site tests, fifteen prefix/replay tests and four abstract
flow-path tests. The latest active source passes all-target release checking.
These are copied-debug runtime checks and active release compilation, not a new
optimized CLI or routing benchmark. The benchmark ladder remains pending.

The combined flow regression also exposed and fixed a masked-context bypass
witness mismatch. It first validates the original association in its prior
context, then freshly proves that same link and return target on the mask-free
physical prefix. It never patches an old witness digest. Original-pad capture
retains the raw producer frame and exact declared mask occurrences; cleanup
cannot invent a formerly blocked original path. Original lattice barrels can
retain their original pad anchor, while replacement anchors remain explicitly
ordinary-owned.

Two further integration steps remain: power islands must consume accepted site
ownership without inventing cells for unsited balls, and pending inner-ball areas
need explicit owner-aware routing permissions across search, exact admission,
attachments, tuning and replay. A netless rectangular keepout self-blocks escape
work; ordinary raster ownership can also be bypassed by neck-down exceptions.
Local dogbone/site admission does not certify a legal onward boundary channel.
The accepted-site-only island API is implemented and passes four focused tests,
but is not yet wired into the early flow phase. It supports the current
half-pitch/default-through-via lattice contract, reports missing lattice contacts
without fabricating seeds, and preserves authored zones/cuts. Manufacturer-offset
sites and macro-owned supply exits require their own explicit provenance; missing
lattice proof is not a native-open claim. Early integration must regenerate guides
and preserve the derived-zone ownership through placement feedback/replay.

The public 455-projection comparison is
`build/p15-escape-reference-comparison/connection-comparison.json` (91 nets per
arm). All fourteen USB members retain identical lengths, via counts and
connectivity; TX2 connector lengths remain 20.889949/20.826474 mm against 16 mm.
A native filled F.Cu/In4.Cu/B.Cu overlay is retained at
`build/p15-escape-reference-comparison/both-power-islands/reference-escape-power.svg`:
U_USB:U1 aligns all 169 pads at 180 degrees with 0 mm fit RMS. Layer names match;
stackup/electrical equivalence is not inferred from that fit.

### 2026-11-04 upstream integration and fresh controls

The assessment branch merged `origin/main` at `93cf19e` in `4e3da88`, then
fetched and merged the newer `7df887c` in `3a24cef`. The latter changes only
manual release tooling; it does not change the frozen routing measurements.
Local routing/view/diagnostic fixes were reapplied with upstream released-escape
revalidation, full-width collision checks and back-side pad rotation preserved.
The original CopperScript checkout, CopperLib edits and installed CLI were left
untouched. CopperProbe's active board and macro remain unchanged; its existing
archive deletions are the previously authorized cleanup.

The clean-main executable is frozen in `build/p10-main-baseline` (SHA-256
`96777f38a02532c1e7cce1f471255a1847a59f549aa255a9bcff97c13d49678e`).
Its original CopperProbe run has 12 native unconnected items and zero exact
geometry errors. The best existing source variant, freshly placed and routed
with the same executable, has five native unconnected items: ACQ_EN, two D18
contacts, SWDIO and V33. All seven USB groups are natively connected; GND and
FX_VBUS are also connected. This is not closure: USB2 skew is 0.939967 mm against
0.5 mm, ADC bus skew is 6.758293 mm against 5 mm, and ADC_D0, SENSE and the
TX2 connector segments exceed their length limits. TX1M_IC is now 43.716561 mm
against its 63.6 mm limit, without relaxing that limit.

The match-group metric includes via depth, while `summarize --net` reports trace
length. ADC_D8's 34.800829 mm of tracks and 36.795630 mm matching length are
therefore different supported measurements, not evidence of a stale report.

Clean-main ladder results are retained in `build/p10-main-ladder`; the LedRing
setup failure was resolved by restoring its exact pinned KiCad footprint cache,
without changing dependency pins or source. Its separate retry has zero opens,
zero exact/native DRC findings and passing requirements. The combined comparison
baseline records this provenance. Main retains one round-led-ring open, four
fx10-bridge opens and two AssetTracker native gaps. The small browser boards,
nRF52, full-vertical and CM4 have zero opens and exact errors.

The integrated candidate is frozen in `build/p10-integrated-candidate` (SHA-256
`c409f9c56c76596bb6179c4b6695181ba1474cd99094b95eee74b22fc03190ab`).
The nine-board ladder completed with unchanged open counts, exact error counts,
native gap counts where verified, via counts and warning-code counts. Eight
boards have identical total trace lengths. The opt-in via-lattice FX10 example
is 10.68 mm longer: SSRX1_N and SSRX1_P each grow by 5.123386 mm (13.04 to
18.16 mm), remaining connected within their declared pair budget. This is a
length regression despite the benchmark's acceptance comparator reporting none;
the remaining approximately 0.433 mm is outside those two members. Public
per-net projections are retained in its `net-comparison.json`.

The C409 original-consumer and best-source runs were stopped during placement
feedback before final native verification. Three completed detailed trials gave
no full-score improvement (original source: one equal-open/worse-length trial;
best source: one worse-open and one equal-open/worse-length trial). Their logs
and `stopped.json` retain this negative result; neither run qualifies as a
completed source or performance comparison. The next controlled comparison
holds placement fixed and disables placement feedback in both compiler arms.

The C409 full workspace suite completed
with one stale repair-JSON expectation: the new report correctly includes
`restoration: null` when neighbour restoration was never attempted. The new
revision updates that expectation and explicitly asserts the typed restoration
is absent. Other compiled tests passed, including the editor and rotated-back-pad
interop test. The new revision passes all-target release checking and formatting;
the updated reporting suite passes all 30 tests. After widening only the new
Clock test's authored channel to accommodate the default 5 mm coarse grid,
all 427 routing unit tests pass (two ignored). The test still requires a real
critical improvement that blocks a stem while its via remains legal, and proves
complete rejection/rollback. All 13 selected CLI integration tests also pass,
including whole-route budget framing, local native windows, separate layer panels,
native back-pad interop, issue provenance and editor child cleanup. A release
build and final formatting check pass.

The new compiler is frozen in `build/p10-closure-candidate` (SHA-256
`137a5aaff49aa033a9c2ab163dbcc99fdf3c3af5b3f2f4cb5223c8cc96b4cea0`).
Its nine-board ladder completed with the same connectivity, exact/native DRC,
via counts and copper lengths as C409. The comparator flags runtime regressions
on nRF52, FX10, LedRing and AssetTracker against clean main. Concurrent workloads
prevent an isolated speed comparison, but do not establish that these regressions
are harmless; performance remains unqualified. The controlled best-source routing comparison
uses `build/p10-c409-best-fixed` and `build/p10-closure-best-fixed`, the identical
CLI-validated placement and congestion-feedback budget zero in both arms.
Source, macro, placement, executable hashes and exact arguments are retained in
each existing run's `command.json`. These runs exclude placement and are not a
fresh full-source/performance qualification. The current whole-workspace suite
has not yet been rerun; the completed broad suite belongs to C409 as above.
Timing overlaps compilation and other runs and is not an isolated
speed comparison. The routing-only run excludes placement and cannot qualify a
fresh consumer source change. No final integration decision has been made.

Both routing-only comparison arms completed: C409 has four native gaps on D18,
GND, SWDIO and V33; 137A has four on ADC_DCO, GND, SWDIO and V33. D18 closes,
but the clock regresses, so this is not a net closure improvement. Both have
zero other native violations. Total retained copper changes from 2842.179960 mm
and 370 vias to 2821.009741 mm and 358 vias; shorter copper on a disconnected
net is not an improvement. All seven USB groups remain connected. Matching
warnings that disappear because a group member is open are not passing results.
The complete public-CLI comparison covers all 91 design nets (182 per-net
projections) in `build/p10-fixed-connection-comparison.json`. All 14 USB members
are identical. Of 38 nets with changed metrics, the 86 nets complete in both
saved explicit/native scopes have aggregate track length +0.338177 mm and nine
fewer vias. The whole-board 21.170219 mm reduction includes removal of the
29.257934 mm clock route. ADC_D0 improves from 35.308186 to 34.893974 mm, clearing
its individual 35 mm track limit; complete ADC bus matching is still unassessable.

The actual `critical_retry_checked` event establishes that ADC_D0 and SWDIO
escapes are already geometrically invalid before the guarded retry. The retry
would additionally invalidate ADC_D11 and ADC_OVR and is correctly rejected.
The first invalidating operation is not yet established. Passive checks at the
initial decoupler batch, adopted internal swaps and first critical pass are the
next diagnostic change; geometry/return validation alone does not certify the
original pad-access promise.

A single-clock replay of the actual saved pre-critical input reproduces the
ADC_DCO failure in 9.9 s (`build/p12-dco-source-replay`, frozen 137A). U_USB.A1
has zero target accesses; four local attempts each exhaust after expanding and
discovering only the start. Other generated critical routes are absent from
this replay, so they are not required to reproduce the blockage. This localizes
the fault to prepared input geometry/reservations rather than middle-route
search effort. Exact first-step blocker identity remains to be collected; the
aligned manufacturer view alone does not establish which reservation blocks it.

An isolated dev-profile diagnostic compiler adds bounded observations of already
rejected first edges only when no successor is admitted. Successful, partially
explored and unobserved searches collect no detailed blockers. Three focused
tests pass, including unchanged search results and exact/ambiguous source-region
matching. Public `copper issues` exposes the resulting diagnostics. A second
replay uniquely matches the actual blocker to `board.copper_keepouts[80]`, ID
`escape-reserve-via-12`, with no matching macro protected region. The same static
region blocks all eight first directions. Its square spans x=77.55–78.20,
y=21.30–21.95 mm; the nearest corner is 70.711 micrometres from A1's centre,
inside the requested 75 micrometre track half-width. This identifies a prepared
via reservation as the immediate clock blocker, rather than middle-route search
exhaustion. The ID is an observed source label, not independent ownership proof.
The drafts remain outside the active release source; the dev replay's timing is
not a performance comparison.

The next revision emits passive `ordinary_escapes/geometry_checked` progress
events at the initial plan, initial decoupler batch, adopted decoupler retries
and before/after the first critical pass. Each event bounds net/blocker records
and identifies the first illegal track or via, including its owned occurrence.
It labels original package-access proof `not_checked`; telemetry errors are
reported as unavailable and cannot change adoption. A specific internal-swap
bug is also corrected: replanning now sees the ordered original lattice prefix
before added bypass copper. The old `added_elements(retry, base)` contribution
alone omitted that lattice. A real accepted-swap regression is in the combined
release test run; the correction's actual-board effect remains unmeasured.
All 431 routing unit tests pass in `build/p12-routing-unit-tests.log` (two
ignored), including that real swap, layer scheduling and passive source-mask
regressions. In a separate copied debug workspace both regressions fail with
their fixes removed and pass after exact restoration/rebuild. The prefix mutant
leaves an illegal replacement group; the queue mutant puts B.Cu at rank 45
instead of 2. The completed release CLI is frozen in
`build/p12-layer-fair-candidate`, SHA-256
`5004c28c5d8af4d78faf239b8ca47469ecf232901e245f910f0468280c832faf`.
Its same-placement board comparison and nine-board ladder are running. The isolated first-step
blocker draft is a separate diagnostic revision and is not covered by these tests.

The 5004 controlled run localizes the first observed geometric damage: initial
fanout, the initial decoupler batch, all four adopted internal swaps and the
pre-critical frame have zero invalid escape groups. `after_first_critical`
reports ADC_D0 and SWDIO invalid. SWDIO's reserved front stem runs from
(84.250,16.000) to (84.625,15.625) mm. New GND via occurrence 159 occupies that
exact endpoint; GND track 613 runs there from (85.000,16.000), U_USB.L9. The
blockers' immutable flag means locked in this snapshot, not authored ownership.
The critical stage preserves the ordinary masks, and the plane stitch uses
the normal clearance index, so missing effective proxy coverage is the current
inference. The aligned single-clock replay view also shows this early GND tap;
the kit keeps the adjacent SWDIO and ground dogbones separate. A physically
admitted prefix would make the existing plane index respect the actual owned
escape copper, but that integration still needs original-pad proof and typed
checkpoint ownership; a role label alone is insufficient.

The via-only octagon experiment and the tested first-step/source-region
diagnostics are now applied after the frozen 5004 compiler. Three copied debug
geometry tests pass: exact integer disk containment including odd nanometres,
the actual square-corner false positive, and genuine collisions remaining
blocked. Track masks and the existing whole-piece suppression heuristic are
unchanged to isolate via geometry. The active combined release suite/build is
running in `build/p13-routing-unit-tests.log` and `build/p13-build.log`; 431
passing tests belong to 5004, not this new revision. Via masks still encode the
board minimum gap rather than universally proving higher per-net/drill rules.

The initial V33 native-proposal census contains 15 unique front-layer proposals,
11 on In2.Cu and six on B.Cu. The latter layers first reach queue positions 26
and 30, outside the initial eight-trial allowance. This establishes scheduling
starvation, not legal or natively accepted alternate bridges. A layer-fair
interleave using the same candidates, within-layer ordering and trial allowance
is implemented after 137A and still awaits completed verification. The census
covers its initial input only; later native inputs remain explicitly uncovered.

Fresh source-only trials are isolated in `build/p11-sense-compact-source` and
`build/p11-tx2-bank-south-source`. The first compacts four SENSE support poses
without changing its 4 mm limit; the second moves both TX2 capacitors 0.75 mm
south without changing polarity, layer or the 16 mm limit. Initial cache failures
were resolved with the exact pinned module. Subsequent placement processes and
the layer-fair build stopped without final results. Their logs are preserved;
the source trials were restarted in `run-restarted` with the frozen 137A compiler.
There is no source-trial acceptance result yet. The original consumer is untouched.

Both source trials subsequently completed with frozen 137A. The compact SENSE
layout reduces its connected, zero-via trace from 9.873105 to 6.214332 mm, still
2.214332 mm over its unchanged 4 mm limit. All 91 nets were compared: only TIP
(-0.884559 mm), BUF_INPUT (+0.207158 mm) and the already-open GND (+1.500000 mm)
also change; every via count and all 14 USB nets are unchanged. Native gaps
remain ADC_DCO, GND, SWDIO and V33, with zero other native violations and no
courtyard/overlap finding. This is an improvement, not acceptance. Its view
shows counted track copper inside same-net pads and a local tree; an inactive
owned-copper pruning draft is being tested, without claiming that redundant
object removal shortens the physical copper union or meets the 4 mm limit.

The TX2 south shift is rejected: native gaps increase four to eight, opening
both RX1 members and both TX2 connector members, plus two dangling RX1 tracks.
Complete USB groups fall seven to five. Connected RX2 grows 1.656852 mm per
member, TX1 IC about 4.01696 mm per member, and TX2 IC 0.417308/0.480779 mm.
The connected groups retain their track limits; vanished TX2 length warnings
are unassessable, not a pass. Per-net results and hashes are in each source
trial's `connection-comparison.json`. Both trials use fresh placement against
a reused-placement control; that scope is explicit. The original source and
external library repositories remain untouched.

The following reviewed changes were applied after the frozen C409 compiler and
test executables finished compilation. They are included in 137A and covered by
its focused tests above; the C409 workspace suite does not validate them. The first test
attempt stopped at sandbox-denied loopback access; the rerun with loopback access
passes the editor, viewer and new rotated-back-pad/native-export regression.
The old failure left an editor child holding the binary open, so the test
now also owns child cleanup through a drop guard, including on panic.

Implemented changes and remaining limits:

- Bus matching previously used track-only lengths in `tune_bus_skew`, unlike the final
  tracks-plus-via-path check. The implemented fix aligns that metric
  while retaining track-only maximum-length room and read-only ownership.
  Clock ownership is a separate unresolved limitation, and the board's ADC_DCO
  does not currently declare tuning amplitude.
- Critical and bypass-completion retries can improve their own stage score
  while invalidating promised ordinary package exits. ACQ_EN and SWDIO are
  initially escaped, then absent after the final join. Their applicable kit
  reference uses front dogbones and local barrels; moving TP_SWDIO alone does
  not reproduce that structure. `build/p10-critical-escape-guard-draft` adds
  fresh complete-candidate validation and transactional rejection, with authored
  keepout multiplicity preserved. Which transaction first invalidates each edge
  is not established by retained diagnostics. A rejected retry can retain an
  existing critical open; it is an invariant repair, not proven closure.
- D18 and V33 native gaps lie inside authored plane extents. Saved fills show
  fragmentation by foreign traces, so simple rectangle enlargement is not
  justified. Implemented native proposal counts/queue ranks establish that the
  existing initial bounded repair allowance does not reach the V33 alternate
  layers. Their legality and native acceptance remain to be measured. Visible
  vias alone do not establish actual queued proposals.
- Length issue B5 previously framed pad markers and clipped the TX2 connector
  detour. The implemented whole-route budget view includes its retained copper
  and preserves local native/package windows.

The raster helper now sets pixel dimensions only on the root SVG. Native view
roots can omit width/height; the old unanchored replacement changed a background
rectangle instead and enlarged a tiny raster. Fresh D18 and TX2 PNGs were
visually checked for sharp geometry and separate front/back panels.

Status: approved for step-by-step implementation, 2026-10-30. P0.1 is implemented
and verified with focused tests and the first small-ladder entry. The first P3
native acceptance fix passes its unit, fresh KiCad and two small-ladder regression
checks. A full CopperProbe rerun accepts both D18 bridges: native opens fall from
12 to 10. Mechanical checkpoint decoding is also implemented and verified on
the actual placement and unified replay inputs. Overall routing closure remains incomplete.

Completed viewer step: the native viewer patch retains and draws aligned reference copper,
renders saved fills with holes and unfilled zone outlines, gives every layer
an independently clipped SVG viewport with a frame and shaded separation,
and reports actual SVG capabilities. Regression tests cover rotation/mirroring,
reference geometry, fill holes, panel clipping and the public CLI. All 13
`copper-report` tests and both `copper --test view_cli` integration tests pass in
the release profile. `cargo fmt --all --check` and `git diff --check` pass.
The native GND view and a saved-export overlay were visually checked; the latter
matches all 169 U_USB pads at zero fit RMS. The historical manufacturer import
was missing; its exact source has now been recovered and freshly imported/aligned
(see section 5).
The reported F.Cu-to-B.Cu bleed was reproduced in the old TX1 image, which
predated clipping. TX1 combined/front/back views have been regenerated. A
long-track regression and a raster pixel check confirm that front-layer copper
does not enter the gap or the back-layer viewport; the combined image was also
checked visually. The old TX1 image is retained as `tx1-package-before-panels`.

The round-led-ring ladder entry before/after this patch is identical: 50/51
connections, 0 exact DRC errors, 47 vias, 591.871 mm, and byte-identical PCB
SHA-256 `8eaff1170a0081e07869ac6337b837bc2f51a251be55eefaff63a1ffb63efa8d`.
Outputs: `build/p0-view-before-cached` and `build/p0-view-after`. Times were
132.9 s and 73.4 s; concurrent compilation/compression and cache warmup prevent
treating that difference as a speed improvement. The earlier uncached attempt
failed dependency resolution and is not a routing measurement. This is a focused
check, not the full consumer ladder required before merge.
The separate-viewport follow-up (`build/p0-panels-after`, 65.0 s) reproduces
the same metrics and byte-identical PCB; no routing regression was measured.

Disk-space blockage was cleared by removing two unused Zig toolchain downloads
under CopperScript's `build/review-20261024` and transparently compressing older
CopperBench build outputs; source and evidence contents were retained.

### Native acceptance repair (P3)

The first solver correctness patch replaces cross-export track/via UUID identity
with complete geometry, duplicate occurrence numbers and total geometry
multiplicity (changed duplicate populations are rejected as ambiguous). It retains violation
multiplicity, checks both evidence bindings and unchanged source intent, and
reports the individual native acceptance rejection reasons. Tests cover index
reordering, added bridges, moved/resized/reassigned copper, duplicate occurrences,
unknown objects, stale evidence and the existing open/island guards. A separate
explicit KiCad 10 test verifies fresh reports on a small bridge fixture.
All 190 `copper-route --lib` release tests pass, including the explicitly enabled
native test. Fresh KiCad reports show opens 1→0 and dangling violations 4→2;
the unchanged, reindexed dangling copper is accepted, while changing its width
is rejected as `native_new_violations`. The test also proves that the former
UUID-key comparison rejects the improving fixture. Review identified and closed
the prepend-duplicate identity ambiguity. Log: `build/p3-native-identity-tests.log`.
The original CopperProbe 12-open result remains the before baseline; the full
consumer verification below is separate from this small regression fixture.

The rebuilt CLI reproduces byte-identical PCBs on both measured small-ladder
entries: round-led-ring remains 50/51 connections, 0 exact DRC errors, 47 vias,
591.871 mm (83.3 s versus 65.0 s); nRF52 remains 35/35 signal connections,
0 exact geometry errors, 42 vias, 347.531 mm (73.5 s versus 115.1 s).
Both comparisons report no regression; timings do not establish a speed change.
Runs: `build/p3-native-identity-led`, `build/p3-native-identity-nrf`, and
`build/p3-native-identity-baseline` (nRF52 control). The initial nRF52 offline
cache misses stopped before routing and are not routing measurements.

Saved-placement replay failed before routing because the Rust board-state
decoder rejects nonempty `body_overhangs` (`state_codec.rs`); constraints were
not stripped. The standard source pipeline was therefore rerun in
`build/p3-native-identity-copperprobe-full`, with fresh placement/critical/final
checkpoints and `--unified-save-inputs`. Faithful mechanical checkpoint decoding
is an additional iteration-enabling task; saving a state does not prove replay
support for every authored field.

**Full CopperProbe result:** native trials 1 and 2 accept D18 bridges with no
displaced nets or cleanup removals. Both the ADC-side and FX10-side native D18
gaps disappear. Native opens fall 12→10, dangling tracks 10→9, and dangling vias
remain 1. Remaining native opens are GND (2), ACQ_EN, ADC_D0, FX_V33, SWDIO,
USB_RX1M/P and USB_TX1M/P_IC. No new native violation or island was admitted.
The original budgets, fixed/macro copper and via count (370) are retained;
tracks increase 2262→2274, with total length 2598.993→2608.187 mm.
Raw explicit-copper DRC still lists deferred zone nets including D18; native
whole-board closure is false, so those report findings are not globally reconciled.
The command exits 1 for the remaining routing/requirements failures, not a crash.

Placement content digest and critical-input content digest match the baseline.
The PCB entering verification-001 is byte-identical in both runs, SHA-256
`ede06b32821d2af378b3af557735b6b2799c38568ff37a02cc76ff91a7e16907`.
This isolates the native adoption change from earlier routing/placement behavior.
The full source run takes 1449.8 s (24m10s), versus 1998.2 s before; the acceptance
fix does not explain faster unchanged earlier stages, so no speedup is claimed.
Native output: `route-report.native-30baafcb6953` within the new run.
Matched native copper views were regenerated and visually checked:
`build/copperprobe-full-rust-m2/views/d18-{adc,fx10}-before.{svg,png}` and
`build/p3-native-identity-copperprobe-full/views/d18-{adc,fx10}-after.{svg,png}`.
The ADC bridge is on F.Cu; the FX10 bridge joins existing copper on In2.Cu.

The missing `BodyOverhang` constraint (`USB_NOSE`, `J_USB`, named edge
`USB_END`, interval 11.5–23.5 mm, depth 2 mm) is now decoded in both the active
placement reader and the older unified replay reader. Both share strict field,
owner, interval, depth and audit-reason validation. Named edges must match an
actual straight outer-boundary segment in either orientation, even when no
allowance references them; allowances also pass the existing body-outline helper.
The original outline remains unchanged. Unknown fields and malformed collections
fail explicitly. Placement board/options key checks are retained, and a changed
binary is recorded as before.

Focused tests cover both decoders, reversed edges, invalid unreferenced edges,
unknown owners/edges, malformed fields, duplicate identities, unchanged fixed
placement intent and the actual placement/unified input files. The real files
must re-encode to their original values, including the placement content digest.
The public `route-board --reuse-placement ... --stop-after placement` command
now reuses `candidate-01`, records `code_changed=true` and retains placement digest
`sha256:9f032f86707773bab02edbf705a68eff7ddf9586c5440463067b114a7d2fb09d`.
It exports a deliberately stopped run (exit 1), not a routing success.
Output: `build/p4-checkpoint-placement`.

The actual unified state now runs through `replay-unified --only ACQ_EN
--set maximum_iterations=1 --set time_budget_s=10`, completing in 3.9 s.
It reproduces one remaining pad group with zero exact-geometry DRC errors;
the selected net remains incomplete. `copper issues` reports `R1 NET-UNROUTED`.
Output: `build/p4-checkpoint-replay`; its `acq-en.svg` was generated by
`copper view` and visually checked after rasterization, with distinct F.Cu/B.Cu
panels. This subset is diagnostic and does not replace the last full-board result.

The two small ladder entries report no regressions and byte-identical PCBs:
round-led-ring remains 50/51 connections, 0 exact DRC errors, 47 vias,
591.871 mm (107.3 s); nRF52 remains 35/35 signals, 0 exact DRC errors,
42 vias, 347.531 mm (91.2 s). SHA-256 values match the P3 measurements above.
Outputs: `build/p4-checkpoint-led` and `build/p4-checkpoint-nrf`.
Compilation and benchmark overlap make these timings unsuitable for speed claims.
Final release verification passes 206 tests: 6 PCB decoder tests, 192 route
unit tests (including fresh native KiCad and actual placement reuse), 7 mechanical
codec integration tests (including the original unified checkpoint), and 1 public
replay CLI test. The final route test log is `build/p4-checkpoint-tests.log`.
The first in-flight build exposed the new unused-edge regression test before its
dependency rebuild; the final rebuilt suite above passes it. Formatting and diff
checks pass. Full consumer-ladder validation remains required before merge.
Populated assembly envelopes, mechanical references and derived `zone_fills`
remain unsupported by the placement reader. Unified replay retains its explicit
refusal of datums/attachment bindings until their semantics are validated; no
such bindings occur in these CopperProbe inputs.

P0.2 and search diagnostics are now implemented; targeted verification and the
full context comparison pass (2026-10-31). `copper issues RUN --issue ID --json` exposes bounded
solver details with scope, source-report path and truncation status. Critical
candidate rejections are explicitly separate from retained-board DRC. Native
issues carry bridge-history inputs and search phases; placement trials expose
the baseline/candidate global gate and deltas. The ordinary replay report now
includes the same detailed-net section as `route-board`, including budget flags
and exact unresolved-pad labels for focused views.

USB telemetry now counts compatible/incompatible terminal collars, distinguishes
search-slice, aggregate-budget and frontier stops, and records closest approach
with heading and bounded rejected goal connections. The closest rejected goal
geometry is retained even if earlier examples fill the sample budget. Blockers
explicitly mean routing-snapshot locks; authored ownership remains undetermined.
These counters and reports do not change search order, routing rules or budgets.

The pre-change isolated critical replays both connect RX1 and TX1 IC at 60,000
states (`build/p5-diagnostics-before-{rx1,tx1}`; 9.7/12.4 s). This is diagnostic
subset success, not full-board acceptance: full routing context must be retained
when explaining the original pair failures. The four-net ordinary control is
`build/p5-diagnostics-before-ordinary` (one iteration, 120 s outer limit, 3.1 s
actual), reproducing ACQ_EN, ADC_D0, FX_V33 and SWDIO opens with zero exact geometry
DRC errors.

The initial diagnostic verification passed 212 route unit tests (including native KiCad and the
actual placement checkpoint), 17 report tests, and 4 public CLI tests. Logs:
`build/p5-diagnostics-unit-tests-final.log` and `build/p5-diagnostics-cli-tests.log`.
The first test run caught a pad-label formatting defect (dataclass display rather
than `component.pad`, fixed in production) and an incorrect tied-distance USB
sample assertion (fixed in the test without changing the truthful diagnostics).

After-change targeted replays preserve byte-identical copper:

| Replay | Result | PCB SHA-256 |
| --- | --- | --- |
| four ordinary nets | same four opens, 0 exact geometry DRC | `105343fadce893e569301282d2f716c7f96b287f5cfeeeebe2a9c0ec321fef65` |
| isolated RX1 | connected | `17ea49c97ea1983260495c099a1cca675572d5272ff46748ccf50df1038e8c59` |
| isolated TX1 IC | connected | `788656694b115b9ead2baba740c5e94154472fbb3413fa434918db48b746c252` |

The ordinary diagnostics now locate ACQ_EN at U_USB.K7, ADC_D0 at U_USB.K2,
FX_V33 at C_FX10_J3.1, and SWDIO at U_USB.K9. Each has 6 target accesses and
ends in frontier exhaustion after 3 search windows, with no outer deadline;
expanded-state totals are 663309, 430361, 407694 and 378121 respectively. These
are sums across windows, not violations of the 400000 per-search cap. The final
selected search-budget flags are false. These observations establish bounded
graph exhaustion, not physical infeasibility. Current/reference copper views
for ACQ_EN and FX_V33 were generated and visually checked under
`build/p5-diagnostics-ordinary`, aligned on all 169 package pads at zero RMS.

The small ladder remains byte-identical to P4: round-led-ring 50/51 connections,
0 exact DRC, 47 vias, 591.871 mm (109.1 s); nRF52 35/35 signals, 0 exact DRC,
42 vias, 347.531 mm (104.8 s). Outputs: `build/p5-diagnostics-led` and
`build/p5-diagnostics-nrf`; both report no regressions. Concurrent work prevents
timing-based performance conclusions. The full run in `build/p5-diagnostics-full`
reuses the exact placement checkpoint; its initial critical-input digest matches
P3 (`sha256:b1031af014b61a2fa7c8bbf72b519d69adb02a7da4251d2055763b457e926a8e`).
Its retry input also matches P3 (`sha256:1744cdb1fbe56bd6374d22b3653ae8e7cd0c108602fd5e10b8c46a6d1edc3c12`).
The completed run took 1273.9 s and produced byte-identical copper to P3
(`f059375d158b389babe816ae89364f1ce31e0b7a33a7b855fb8c94662256b65a`).
It retains 8 signal opens plus 2 GND native gaps, zero exact geometry DRC errors,
5 length warnings, 9 native dangling tracks and 1 dangling via. The diagnostic
changes therefore do not constitute a routing repair or qualification pass.

The full-context records now establish these distinct failure mechanisms:

- TX1 IC: 0 pair searches/states. Front collars have 0 compatible and 24
  incompatible combinations across three pitches. Back-layer capacitor
  transitions exist, but the U_USB side produces none. Its bounded local planner
  samples 196 sites, finds 16 legal via sites and makes 32 path calls: 20 frontier
  exhaustions and 12 state limits. This localizes missing terminal construction;
  it does not establish that no legal via location exists.
- RX1: 24 pair searches use all 60000 states, with 510 compatible and 420
  incompatible collar combinations. The closest retained approach is 0.65928 mm
  from the goal and rejects lane construction: its generated tail reverses
  direction by 180 degrees, violating the adjacent-turn rule. A separate candidate
  2.656059 mm from the goal has an exact B.Cu clearance conflict with FX_USB18's
  track at x=87.850 mm and C_FX10_M4.1 near (87.850,23.000) mm. Its rejected segment
  runs (88.638902,23.647500) to (87.552500,22.561098) mm. The reference-aligned native
  view was inspected at `build/copperprobe-full-rust-m2/views/usb-agent-p5-rx1-goal.png`.
  These are separate failure mechanisms; neither proves physical infeasibility.
- GND: native bridge trials 3–8 all start at U_USB.N6. Both all-copper and
  protected-copper searches have 3 legal initial steps, exhaust the frontier
  after 87 states (trial 6: 91), and make no final-connection attempt. Sampled
  rejected steps encounter USB_TX2P_IC, USB_TX1M_IC, FX_USB18 and reserved regions.
  These are bounded snapshot-lock samples, not exhaustive obstruction or authored
  ownership proofs. The 8-trial budget ends without trying N4. The fresh
  `gnd-reference.svg/png` was inspected with separate F.Cu/B.Cu panels and zero-RMS
  manufacturer alignment; the reference has local ground dogbone/via structures
  absent from these consumer balls.
- FX_V33 has no declared distribution zone and therefore has zero native bridge
  trials. Its ordinary-route failure must not be reported as a failed native
  bridge search.
- All 4 placement trials are rejected before detailed routing. Baseline and
  candidate have the same 3 global failures (FX_USB18, FX_VDDD, GND), zero
  overflow and zero added/resolved failures. The absolute global-success gate
  therefore prevents measuring whether these R_ACQ_EN/R_RESET_ADC moves help
  detailed routing; the report does not establish that the moves would help.

Additional access-preparation diagnostics now retain candidate coordinates/layers,
effective widths, region/neck flags and actual open-probe/local-escape decisions,
including flood cutoffs assumed open and the shared local-escape allowance.
The final route suite passes 216 tests, report suite 19 tests and public CLI suite 4 tests
(`build/p5-diagnostics-local-final-tests.log`, `build/p5-diagnostics-report-final.log`).
Grouped routing details are bounded to 16 selected records with total count and
truncation metadata; legacy replay source paths and placement displacement units
are preserved. The final combined log includes the public CLI tests.
The final four-net replay (`build/p5-diagnostics-access-ordinary`) again produces
byte-identical copper to the pre-change control. Its access data rules out the
suspected optimistic-open-probe skip for these four selected failures: every
probe exhausts its frontier below the 600-node cutoff, and local escape runs
with shared allowance remaining. Results by pad:

| Pad | Probe visited | Local stop | Local path attempts / found |
| --- | --- | --- | --- |
| U_USB.K7 (ACQ_EN) | 17 | radius limit | 0 / 0 |
| U_USB.K2 (ADC_D0) | 36 | consecutive miss limit | 4 / 0 |
| C_FX10_J3.1 (FX_V33) | 44 | radius limit | 0 / 0 |
| U_USB.K9 (SWDIO) | 13 | radius limit | 1 / 0 |

All six nearest candidates remain in the local pocket. FX_V33 has one
neck-only candidate and SWDIO three; ACQ_EN/ADC_D0 use nominal-width candidates.
This identifies bounded local escape generation as the next investigation;
candidate node flags and these stops do not identify an exhaustive blocking set
or establish physical infeasibility. The four-net selection/order was preserved
because the local-escape allowance is shared across the router.

Final local-search instrumentation closes the remaining counter gap. The combined
route/CLI tests pass (`build/p5-diagnostics-local-final-tests.log`), including the
synthetic biased-radius case, and `build/p5-diagnostics-local-ordinary` again
preserves the same PCB hash. The real replay confirms premature radius stopping:
direction-weighted sorting leaves 13 eligible in-radius endpoints untested for
ACQ_EN, 7 for FX_V33, and 25 for SWDIO. Eligibility means an open-region node
with a free nominal-width owner; these endpoints have not yet been exact-path
tested. ADC_D0's four actual local attempts exhaust their frontiers after
760/680/760/680 states; SWDIO's one attempt exhausts after 243. All are far below
the 20000-state cap. Bounds remain radius 2 mm, step 25 µm, detour 0.5 mm.

The next concrete generic fix is to filter candidates by raw radius before
direction-biased ranking (or skip out-of-radius candidates), preserving the
attempt/miss/state bounds. This diagnostic pass deliberately preserves the old
selection behavior so the before/after comparison remains controlled. The defect
is now demonstrated on real inputs; whether testing the skipped endpoints closes
these nets must be measured separately. It does not explain ADC_D0's four failed
local paths, which remain a separate geometry/reference investigation.
The final small-ladder rerun also passes without regressions and with the same
PCB hashes: `build/p5-diagnostics-local-led` (88.5 s) and
`build/p5-diagnostics-local-nrf` (85.3 s). Final verification totals 239 selected
tests, including fresh native KiCad, plus the controlled replays and ladder runs.
Formatting and diff checks pass. The full consumer ladder remains required
before merge; this diagnostic pass has not completed all routing repairs.

The implemented ordinary-search patch returns typed outcomes from both uniform
and nested A*: found, frontier exhausted, or expansion limit, with actual
expanded-node counts. The path-only wrapper and cutoff/target ordering remain.
The selected `NetRoute` retains the unresolved required connection's reason
through retries and best-snapshot restoration into
`DetailedNetResult.search_budget_exhausted` and diagnostic strings. Outer
deadlines and missing access are distinct; optional plane-contact failures and
failed windows followed by success do not set the final flag. Both kernels,
budget boundaries, scratch reuse, successful wider retries, discarded routes
and optional contacts are tested. Historical reports' false flags cannot
establish that their searches stayed within budget.

### Connected detour repair investigation (2026-10-31, in progress)

Resume review (2026-11-04): fetched main at `93cf19e`, 89 commits ahead of
the assessment merge. It includes released-escape revalidation, installed
decoupler hooks and live negotiation progress (`2ffb787`, merged by
`73dd0f9`). Preserve distinct local fixes and measure the integrated compiler
before designing another recovery path. Original CopperProbe board and USB
macro hashes are unchanged; its 229 archive deletions are the authorized
cleanup. CopperProbeParts-hybrid and the LedRing/AssetTracker checkouts are
clean. Other modifications in the main CopperScript and CopperLib checkouts
are separate work and are not incorporated by editing those checkouts.

Recovered completed trials reject two pending experiments. Retry-only plane
mask protection (`build/p9-plane-mask-front`, 1518.30 s excluding reused
placement) has five native gaps versus the old front control's four: ACQ_EN
closes, but GND and V33 become open; SWDIO and both TX2 IC nets remain open.
Its two failing unit fixtures used outer-layer zones excluded by the default
stitch options; a corrected internal-layer fixture is saved in the draft.
The production experiment and active tests are removed, rather than adopted
on a synthetic result. The eastward TX2 capacitor move
(`build/p9-detour-board-front-ic-priority-caps-east/full-current`, 1963.87 s
including placement) has six native gaps: two D18 contacts, GND, SWDIO and
both RX1 nets. It is rejected. Neither source trial is copied to the consumer.

The current integration is merge `4e3da88`, whose committed source tree matches
main `93cf19e`; the assessment patch remains uncommitted. Backup stashes are
`9426e02f5fe8b25680a64b3f6797adb73962b2b3` (before integration) and
`2dd367f7ac9bd19e21e41e08de9ba346b71912c8` (integrated patch). The merge keeps
upstream streaming events and the local selected-failure diagnostics together.
New upstream tests pass their real retained track prefix to pair routing;
all-target workspace checking passes. An additional interop regression compares
saved/native geometry for a rotated back-side rectangular pad and F/B clipping.
The full release suite is still running. A clean-main compiler was frozen at
`build/p10-main-baseline/copper.exe`, SHA256
`96777f38a02532c1e7cce1f471255a1847a59f549aa255a9bcff97c13d49678e`.
Fresh ladder and unchanged-consumer runs are under `build/p10-main-ladder`
and `build/p10-main-probe`; LedRing's missing exact pinned footprint cache was
restored locally, with a separate retry under `build/p10-main-led-ring`.
The installed compiler is a separate November 3 build (`2d3a757f...`), so it is
not overwritten or used as this comparison's unidentified baseline.

Review also confirms the lattice-prefix omission spans witness refresh,
released-escape validation and later strict admission. `macro_source` rejects
ordinary/lattice copper, so adding the prefix only to helper frames does not
fix the complete path. The proposal in
`build/p10-fixed-prefix-draft/design.txt` reuses explicit fixed-prefix admission
and preserves ownership, checkpoint context and required multiplicity. It is
not an implemented routing fix. Current critical retry selection still needs
whole-access preservation; a new controller or an unsafe partial prefix patch
is not added.

The combined P8 candidate exposes a critical-router defect beyond package
escape: a connected pair with length/skew warnings skipped the existing
bounded rip-up loop. A repair could also commit a connected but over-budget
restoration, moving the detour to its neighbour. Aggregate search exhaustion
was hidden whenever the returned fallback was connected.

The P9 candidate includes connected warned pairs in the existing repair loop
and calls a budget repair compliant only when both trial groups meet their
profiles. A warned connectivity fallback remains a separate, explicit result.
Nested-exit hints are transactional, and accepted reroutes invalidate obsolete
tuning history. Connected fallbacks now report aggregate budget exhaustion. Public
budget issues include bounded selected-route search and repair details;
rejected trial geometry remains distinct from retained-board DRC.

Real-geometry tests exercise successful shortening, rejection when the
restored neighbour exceeds its limit, and rollback with an immutable input
macro. A fixture placement error was corrected before the final rerun.
All 259 selected library tests pass (230 route, 21 report, 8 PCB); two route
tests remain explicitly ignored. The frozen P7 executable reproduces the P8
combined TX1 detour from the saved full critical stage in 231.36 s, using the
original 6000-state setting and inherited 10x escalation. No search limit has
been increased.

The repair-guard-only executable (`86a9f28c16d88e81cebef75e66416fd15c00c736dc7c0d2833a3d698720913b5`)
replay takes 316.04 s and regresses seven connected USB groups to six: TX1
stays open. Its TX2 rip-up trial still finds the same 109.13 mm generated
route, correctly reports 60000 states/exhaustion, and rejects the length miss.
Three other bounded rip-ups do not connect it. This is a measured rejection
of the guard-only candidate as a board fix, not successful detour elimination.
The times overlap compilation/other benchmarks and do not establish speed.

An isolated second candidate adds complete-net pair max-length warnings using
the actual retained prefix, separate from search obstacles/prepared branches,
and length-aware scheduling of existing resumable frontiers. All 237 route
tests pass, including five prefix/retuning cases and two scheduling cases.
However its saved-input replay (`de2151dfaa6a1c865bc2dd97610ec40981d0d68ca878c63f99377a67207a3665`,
309.53 s) still connects six USB groups and loses even the old overlength
candidate after ripping up TX2 at 60000 states. The scheduling experiment
and its two tests were therefore removed, preserving the earlier search
and goal-tail diagnostics. A synthetic improvement does not establish a
consumer improvement.

The final candidate retains complete-net pair max-length accounting and
defers connected warned two-group transactions while trying the remaining
existing repair slots. Only an originally open target may retain such a
fallback; a connected target cannot transfer its budget violation to its
neighbour. A compliant repair wins immediately. Otherwise the deferred
choice compares signed whole-board warning delta (subtracting the original
blocker's warning count), then total resulting copper length. Only the
selected transaction publishes copper, nested-exit hints and tuning
invalidation. Warned fallbacks retain their warnings, exhaustion flags and
explicit selection reason. The core executable is
`52d742c06a6deb9a0e5d57bc0ac65eaf27df3d5dde8839a6276128669426a252`.
Its saved combined-board replay restores all seven USB groups and the exact
old PCB hash `b1d48cec52dd5c96135b56f11d249a4e4d0d01e8f5e010b087a69e06dc1d51fb`.
TX1 remains 112.861708/112.798240 mm overall, correctly marked as a warned
connected fallback after all four bounded trials, with 60000-state exhaustion.
It is not a successful short-route fix. The 711.49 s elapsed time overlaps
compilation/other runs and does not establish isolated performance.

The workspace release suite passes 604 tests. Three explicitly enabled native
KiCad/saved-checkpoint tests also pass. One unrelated unsupported movable
decoupler placement fixture stays ignored. Reporting-only additions then expose
the restoration's own state/exhaustion, diagnostics, collars, transitions and
frontier attempts in `repairs[*].restoration`; null means it was not attempted.
The affected 238 route/22 report tests and three public CLI tests pass. Blocker
identities and snapshot ownership survive the existing bounded issue projection;
legacy absence stays unknown. On cap0, the target connects after 3276 states
without TX2, then restoring TX2 consumes 60000 states and fails, including
clearance against the newly generated TX1 copper. Those rejected trials are
not retained-board DRC. A per-net `copper summarize REPORT --net NET --json`
projection is validated so compliant retained lengths need no per-failure
report-inspection script. Its 26 report and five CLI tests pass; exact integers,
zero/false, missing legacy values, open stubs and unchanged default summaries
are covered. The active CLI was installed with the locked offline Cargo target;
its SHA256 is `1197dd5fca07f86b0f55c9d2a4624fbdc82371444985c311af3d895c912c4bdc`.
The preceding restoration-only build is `230d1beaf2d6d1a1136a5af41ee7cdc356f34f9767fb4538b28a8a8c358adc0b`.
Neither reporting addition changes routing decisions.

The core candidate's smaller ladder preserves all seven exported PCB hashes
(`build/p9-ladder-final/copper-identity-comparison.json`): round LED example,
nRF52, full-vertical, CM4, FX10 bridge, CopperLedRing and CopperAssetTracker.
The two missing pinned example caches were restored from verified local
copies without dependency changes. A complete current-source/native run on
the unchanged original CopperProbe source also reproduces its control PCB hash
`f280e9b744b0cfefafb3d6c9eedae07b6989db18e508348e1dd8439275e6a95a`
and ten native gaps (nine dangling tracks, one dangling via, no geometry
violations). These controls retain their existing opens. Timing comparisons
overlap other work and are not a speed result; the CopperProbe run takes
1913.77 s under that contention, so an isolated 30-minute result is unproved.

Consumer inspection also found reversed TX1 capacitor orientation. In P8
combined, pad 1 (IC) is at x=128.230 mm and pad 2 (connector) at x=127.270 mm,
forcing doubled-back IC lead-ins. The manufacturer C6/C7 orientation, after
the verified package alignment, puts IC pads west of connector pads. The
isolated `build/p9-detour-board-cap0` changes only the two rotations 180 to 0
and their comment, retaining all dimensions, nets and budgets. Fresh placement
confirms IC pads at x=127.270 mm and connector pads at x=128.230 mm. Full
frozen-P7/native trials show that this geometric cleanup alone is not a repair:

| Consumer arm | Native gaps | TX1 IC | TX2 IC |
| --- | ---: | --- | --- |
| P8 combined | 3 | Connected, 112.86/112.80 mm against 63.6 | Connected |
| cap0 | 5 | Open, length compliance N/A | Connected |
| cap0 + TX1 priority 29 | 5 | Connected, within 63.6 mm | Open, length compliance N/A |

Both cap0 arms close GND L8 but open ACQ_EN; D18 and SWDIO persist. The other
five USB groups stay connected. TX2 connector length worsens from about
20.37 mm to 21.37/21.42 mm in the priority arm against its 16 mm limit.
`build/p9-board-connection-comparison.json` contains CLI-derived contacts,
USB/ADC warnings and timing scopes. Neither source arm is adopted.

An isolated TX1 critical replay connects in 132 states, and both IC pairs
together connect within their limits in 214 states. These are scoped
diagnostics, not native whole-board closure. A cohort omitting TX2 connector
also connects TX1, but changes earlier paths and leaves USB2 open, so it
does not prove a single omitted blocker. Early launch/transition blockers
are recorded but do not currently feed repair priority; that gap is real,
but an untried TX2-connector hypothesis remains unproved.

The cap0 restoration also exposes a finite-search allocation problem:
`resume_pair_candidates` gives all remaining aggregate states to the closest
eligible frontier. One TX2 frontier consumes 49440 of 60000 states while
another live frontier receives only its initial 500. Fair bounded resumption
is the next isolated engine experiment, retaining the original aggregate cap.
The closest frontier stops 1.695892 mm from the package-side end without a
tail attempt because the 0.25 mm grid's handoff radius is 1 mm. Its obvious
two-leg continuation crosses retained USB2 copper, so enlarging that radius
is not a demonstrated repair. Likewise, sampled package return-via rejection
counts do not establish missing access: this restoration has 16 package
transitions and 14 direct front contacts. No speculative macro via is selected.
The fair-resumption candidate (`06f87a11ca791352800261bc019028acc597a725fe97752bfe136807d59570cd`)
passes both real-copper allocation regressions and all 240 route library
tests (two ignored), but the matched cap0 full critical replay still leaves
TX1 IC open. It exports exactly the control PCB hash
`42b497cadbb24d744bcfc4d564da86b0050962d35bc465e40a42fdaf95094a72`.
Its 340.10 s overlaps other work and is not an isolated speed result.
The scheduling change and its two tests are reverted from active source;
the normal `build/p9-fair-cap0` run and draft retain this negative result.
The aligned reference tripod cannot be copied into the present fixed scene:
TX2P's via at (89.1164,16.9000) mm lands inside back-side C_FX10_M6.1
(FX_USB18), whose actual pad bounds are x=88.70..89.26, y=16.69..17.31 mm.
TX2M's via at (89.1164,16.0946) mm overlaps the RX2M macro's 0.145 mm front
tail at y=15.8025 mm even before clearance. A reference-like transition would
therefore require coordinated macro and support-placement changes, followed
by exact host-rule validation; no raw-reference transplant is selected.

ACQ_EN and SWDIO are genuine lost K7/K9 package exits, rather than native
identity errors. Both were planned initially; the front control later drops
their reserved copper. Aligned reference signal dogbones occupy cells now
used by generated GND L8/L9 return-plane vias. The reference GND alternatives
conflict with current FX_USB18 back copper and SWCLK inner-layer copper, so
they cannot simply be transplanted. The critical retry releases broad
ordinary masks and selects by critical failures alone; exact released-pad
attribution is not yet reported. A focused draft preserves the admitted masks
only for the retry's early plane stitch, using existing plane-only keepouts;
critical search still gets the same bounded release. Connectivity benefit
remains unproved. This does not by itself establish transactional preservation
when critical signal copper, rather than plane copper, consumes an escape.

The final front TX1 skew is correctly accounted: RX1's accepted open-target
repair restores TX1 with the explicitly warned 1.479155 mm skew; the final
tuning credit is zero. No prefix-skew or stale-tuning fix is justified by this
scene. Separately, unified critical results currently leave the numeric state
counter at an uncounted zero and discard failed exact-clock retry diagnostics.
A diagnostic-only draft will distinguish unknown counts and retain those
failure causes without changing selected copper.

A further current-Rust PHY-cap trial puts TX1 capacitors at (92.5,18.75) and
(92.5,20.25) mm, reallocating the same 71.6 mm allowance as 8 mm IC and
63.6 mm connector. Its final native run has six gaps, still including TX2 IC,
and takes 1032.54 s after 493.07 s of earlier placement work. Public retained
metrics give TX1 IC 6.938113/6.938117 mm and connector 49.826112/49.889583 mm;
TX2 connector is 29.008966/28.945503 mm against 16 mm. It is rejected, also
because [AN238422, section 7.1](https://www.infineon.com/assets/row/public/documents/24/42/infineon-hardware-design-guidelines-and-schematic-checklist-for-ez-usb-fx5-fx5n-fx10-fx20-applicationnotes-en.pdf)
distinguishes cable-connected near-receptacle placement from the embedded-link
near-PHY exception. A clean route alone would not qualify that adaptation.

The polarity-inversion source trial preserves connector-side placement and every
original routing budget. TX2 macro ports are M above P; the capacitor inputs
are P above M. It intentionally swaps only the two IC-side capacitor endpoints,
using the differential-pair polarity inversion permitted by AN238422. PHY pins,
macro, connector-side nets and physical constraints stay unchanged. ERC passes.
The completed current-Rust/native trial under
`build/p9-detour-board-cap0-priority-tx2-inverted/full-current` takes 1662.77 s
including fresh placement and still has five native gaps: ACQ_EN, D18, SWDIO
and both TX2 IC nets. It has seven dangling tracks and one dangling via,
with no native geometric violations. TX1 IC is 51.607950/51.544481 mm, within
63.6 mm; TX2 connector remains 21.373916/21.422690 mm against 16 mm. This
trial is rejected as a full-board fix. The original consumer remains untouched.
Both prepared critical passes leave TX2 IC open. A separate
current-Rust critical replay of the unchanged priority control also leaves
TX2 IC open, with all six other USB groups connected, so this failure is not
explained solely by the source arms' compiler difference.

A final focused side-placement trial retains the inversion and all pad XY
coordinates, moving the two TX2 capacitors from back/180 to front/0. Existing
front courtyards leave room, and no source region forces the back side.
This removes the mandatory IC F-to-B transition; the connector lands are
through-hole/all-layer, and their existing short segment had no vias, so no
connector-via removal is claimed. Every budget and connector-side placement
requirement stays unchanged. Local pad-antipad qualification remains an
explicit pre-existing gap. The trial uses the installed current compiler in
`build/p9-detour-board-cap0-priority-tx2-inverted-front/full-current`.
It finishes in 1800.95 s with four native gaps: ACQ_EN, SWDIO and both TX2 IC
nets; D18 closes. There are seven dangling tracks and one dangling via,
with no exact/native geometric violations. TX1 IC lengths are
50.096339/51.575499 mm, but retained skew is 1.479155 mm against 0.0635 mm.
TX2 connector remains 20.889949/20.826474 mm against 16 mm, with one via on
each net. This arm is not adopted. The next focused source test changes
only both TX2 IC priorities from 21 to 29, routing both IC pairs before USB2
and RX2. The isolated two-IC result and the observed USB2 crossing motivate
this ordering hypothesis; all other source geometry and limits stay fixed.
The completed `build/p9-detour-board-front-ic-priority/full-current` run takes
1577.88 s and retains all seven native-connected USB groups with no USB skew
warnings. TX1 IC is 55.663414/55.726891 mm within 63.6 mm, and TX2 IC is
51.260723/51.197251 mm within 55.6 mm (two signal vias per net). This removes
the 112.86 mm detour without transferring a USB open. TX2 connector remains
20.889949/20.826474 mm against 16 mm. Three native gaps remain: FX_VBUS,
GND and SWDIO; these include new power gaps, despite ACQ_EN and TX2 closing.
Five dangling tracks and one dangling via remain, with zero exact/native
geometric violations. ADC skew, ADC_D0 length and SENSE length still warn.
The source stays isolated. The next source arm moves only both TX2 capacitor
centres from x=125.9 to x=129.5 mm: exact transformed front courtyards have
1.54 mm clearance from the connector courtyard. This aims to shorten the
connector segment while preserving every budget and electrical endpoint.
A north-edge receptacle was investigated
but not implemented because it changes the documented probe-left/USB-right
layout and unresolved enclosure/mating access would need separate validation.

A separate In2-layer candidate is placement-only. Its two TX1 IC rules add
In2 adjacent to In1 ground, but SI screening estimates 94.9877 ohm against
90 ohm +/-5%, so it is not an accepted electrical adaptation. Manufacturer
TX1 exits remain on F.Cu. The pair max-length correction does not address
unequal-prefix skew or all single-ended accounting. Neither an eliminated
detour nor complete board budget compliance is established yet.

### Board and macro co-design investigation (2026-10-31)

Concrete consumer-source candidates were implemented and tested under
`build/p8-board-macro/`, using the frozen P7 Rust executable. The active consumer
was not edited. Each project copy preserves dependency versions/content and
assets, with only the local replacement path made absolute and re-locked offline.
Module checksums match the original. Changed physical identities correctly refuse
the old placement checkpoint, so every arm received fresh placement/global checks
and its own checkpoint before the standard balanced six-layer/native full run.
No allocator flags, rule relaxations or congestion-feedback trials were used.
The fresh control reproduces the exact P7 PCB hash and its ten native gaps.

Three agents prepared independent consumer edits:

- **USB board:** fix C_USB_TX1M/P at (127.75,18.75)/(127.75,20.25) mm,
  front/180 degrees, within the existing near-receptacle region. Values, nets
  and profile constraints remain identical. The source-only two-pair replay
  connects both segments, but control also does, so that replay is not evidence
  of a full-context launch repair.
- **Ground macro:** add GND bindings for N4/N6, 0.20 mm diagonal-then-east
  ground paths and 0.45/0.20 mm F-to-B vias at (87,19.95)/(87,18.45) mm,
  with proved external ports. Existing USB tracks, ports and protected regions
  are byte-equivalent as parsed source arrays. An initial farther-east via
  placement was rejected against fixed C_FX10_L5; the revised sites pass binding,
  port proof, placement and exact DRC. These vias serve local ball contacts;
  they are not the later USB transition return vias.
- **J3 power board:** move the 10 nF C_FX10_J3 to (83.5,21.4) mm, back/180,
  and rotate the 100 nF C_FX_J3 from 270 to 90 degrees at its original center.
  This remains within the original pin-associated region. It is rejected as
  an overall regression after full routing.

The combined arm joins only the USB-board and ground-macro edits, not the J3
trial. All five full native runs completed:

| Arm | Native gaps | Dangling tracks/vias | Max-length warnings | ADC match warning | Vias |
| --- | ---: | ---: | ---: | --- | ---: |
| Control | 10 | 9/1 | 5 | Skipped: ADC_D0 open | 370 |
| USB board | 10 | 8/1 | 3 | Skipped: ADC_D0 open | 351 |
| J3 power board | 12 | 10/3 | 6 | Blocked: ADC_D10 open | 344 |
| Ground macro | 5 | 7/0 | 9 | 12.143992 / 5 mm | 395 |
| Combined | 3 | 6/0 | 7 | 8.000934 / 5 mm | 389 |

All have zero exact geometry errors, zero native clearance/short/hole/annular
errors and zero islands. Counts are missing native contacts, not unique nets.
Detailed contacts, budgets and quality are in `connection-comparison.json`.
Routing wall seconds are 1567.072/1681.744/1473.055/1616.61/1034.001 respectively;
they exclude fresh placement and overlap other jobs, so they are not performance
claims. Exact reports include stage-explicit opens later joined by native fill;
those must not be counted as additional final native gaps.

**Clean individual gain:** USB-board keeps the same missing contacts while
shortening both TX1 connector tracks from 12.485174 to 5.962021 mm, within 8 mm.
It removes both length violations. TX1 IC still has zero compatible collars,
24 incompatible combinations and zero main-search states in the prepared scene.
The clean replay offers additional PHY exits that prepared copper removes.

**J3 rejection:** ADC_D0 closes at 33.786735 / 35 mm, but ADC_D10 gains two
gaps and RESREF one. J3 supply isolation moves from the 10 nF capacitor to the
100 nF capacitor; it is not eliminated. ADC_D8 adds a 39.140295 / 35 mm violation.
No direct capacitor collision is established for the newly failed signal paths.

**Macro gain and limitation:** N4/N6, ACQ_EN, ADC_D0, FX_V33 and TX1 IC close.
New gaps appear on D18 and GND L8; SWDIO and RX1 remain. The TX1 repair displaces
TX2 IC, which is restored as a very long connected soft-budget fallback:
128.020165/128.020166 mm of full explicit copper against 55.6 mm. Control already
found TX1 after ripping TX2 but could not restore TX2. Thus the TX1 gain must not
be described as unique launch reachability created by the new ground vias.
New source GND contacts and changed preparation do make the overall repair
transaction possible. Fresh control/macro capacitor and testpoint poses match.

**Combined result:** all thirteen ADC members and all seven USB groups are
natively connected. Nine former gaps close (N4/N6, ACQ_EN, ADC_D0, FX_V33,
RX1 M/P and TX1 IC M/P); two new contacts remain open, and SWDIO persists:

| Remaining native contact | Located objects |
| --- | --- |
| D18 | B.Cu fragment centered (77.875,14.250) mm to C_FX_C7.1 |
| GND | F.Cu fragment centered (85,16) mm to U_USB.L8 at (85,16.75) mm |
| SWDIO | TP_SWDIO.1 to U_USB.K9 |

The combined USB budget problem moves to TX1 IC: 112.861708/112.798240 mm
against 63.6 mm. Its retained zero-via F.Cu route circles south/west of the
package, then north/east to the capacitors; this is actual retained copper,
not merely a rejected trial. TX2 IC returns within 55.6 mm (about 45.19 mm
including the macro). RX1 connects with critical lengths 47.499037/47.592191 mm;
all numerical pair skew checks pass. TX2 connector remains 20.374396 / 16 mm.
ADC_D0 is 35.575956 / 35 mm, ADC_D8 36.043471 / 35 mm, and SENSE
9.873107 / 4 mm. ADC matching adds via paths, so its lengths differ from
track-only max-length accounting. Unfinished impedance/return/pair-gap verifier
coverage remains explicit; native clearance is not electrical qualification.

An independent review confirms no new copper-clearance exemption. The asset's
`internal_clearance_nm=0` controls cluster courtyard spacing, not foreign copper.
New GND/USB clearance measurements (0.135 mm track, at least 0.1725 mm via)
carry the applicable terminal's existing 2 mm breakout provenance and 0.1 mm
allowance. Native checks enforce the exported lower bound, while exact DRC
enforces the scoped rule. The 0.45/0.20 mm fabrication limits are preserved.

Reviewable patches apply cleanly to the unchanged source control:
`tx1-cap-alignment.patch` is the isolated clean improvement;
`combined-experiment.patch` is a promising connectivity baseline, not a completed
repair. Both are under `build/p8-board-macro/`. Original board SHA-256 remains
`0726afb6163416476c65b3bd6259d181048fc15fefc6265a1d40ae393d97dbfc`.
The combined source SHA-256 is
`5e12d9b02753647bc82174081d79d51ec49c0d92603630eaf124c81b7fd8b652`;
the new macro asset SHA-256 is
`963f0cc85e42b72d806a610704a05831ea75d2440eb844c818d4df9e22c1aa9f`.

Next work should preserve the successful local contacts/cap alignment while
planning short TX1/TX2 IC launches and transitions together in the prepared
scene. Do not accept a result that merely transfers the long detour between
channels or increases their limits to match it. Follow with the three located
contacts and ADC timing/length correction. Aligned manufacturer/native views
are saved under the candidate view folders; the L8 reference shows a local
dogbone missing from the current result. The two-pose J3 candidate is not carried
forward. No Rust algorithm or default was changed during this investigation.

### Historical allocator gains and revised scope (2026-10-31)

The user has opened CopperProbe's authored constraints and placement to revision.
Treat those choices as design variables while retaining source-backed electrical
requirements. Earlier allocator gains must inform that work: the escape plan
records E2 array flow reducing full-board open nets from 25 to 21 and vias from
522 to 496. Its later five-run FX10 measurements show via allocation reducing
mean opens from 12.6 to 5.4, or 4.6 with exact raster. Array flow was neutral in
that later measurement. The full-board lattice-plus-exact run regressed from
8 opens with exact raster to 13; these are different experiments, not evidence
that the allocators never helped.

Both allocators are present in Rust but were off in the recent assessment runs.
Nested resolution and exact ball-field raster were already on: 0.25 mm base,
0.125 mm around U_USB. The next comparison should account for the current
control, array flow, via allocation and their combination on a common input,
including effects on non-FX10 nets and native continuity. Reuse those mechanisms
when revising the package/support/USB floorplan before inventing another escape
algorithm. No new allocator comparison has been executed by this reassessment.

### Parallel repair candidates (2026-10-31)

Allocator comparison follow-up: latest fetched main `1e4da62` (performance
implementation `dbee97f`) is merged at `1703b78` on
`copperprobe/allocator-main-comparison`; all prior working fixes were restored.
The four-arm comparison uses one frozen merged executable, the same original
placement checkpoint, balanced six-layer profile and eight native bridge trials.
Detailed placement feedback is disabled in every arm to isolate allocation.
Control, array-flow, via-lattice and both flags must be compared by final native
net/contact identities as well as signal completion and exact geometry. Parallel
wall times will not establish a performance speedup. Board constraints remain
identical during this comparison; redesign proposals follow the measured result.
The merged release validation passes 285 selected tests plus formatting/diff
checks (`build/p7-main-tests.log`, `build/p7-main-build.log`). The frozen
executable is `build/p7-allocator-comparison/copper.exe`, SHA-256
`2b65dc77dd7079bd89616953d34e732965585e35ce2d7470305426450506ad49`.
The common board source SHA-256 is
`0726afb6163416476c65b3bd6259d181048fc15fefc6265a1d40ae393d97dbfc`.

All four runs completed. Their native gap counts, from public `copper issues`
projections, are in `build/p7-allocator-comparison/connection-comparison.json`.
Zero means no native unconnected-item finding on that net; it does not establish
length/skew compliance. Counts are missing contacts, not unique open nets.

| Net/contact scope | Control | Array flow | Via lattice | Both |
| --- | ---: | ---: | ---: | ---: |
| ACQ_EN | 1 | 0 | 0 | 0 |
| ADC_D0 | 1 | 0 | 0 | 0 |
| SWDIO | 1 | 0 | 0 | 0 |
| GND | 2 | 3 | 2 | 0 |
| FX_V33 | 1 | 1 | 1 | 2 |
| USB_RX1M/P, TX1M_IC/P_IC, total | 4 | 4 | 4 | 4 |
| ADC_D1 | 0 | 0 | 1 | 1 |
| ADC_D2 | 0 | 0 | 1 | 0 |
| ADC_DCO | 0 | 0 | 1 | 1 |
| ADC_OVR | 0 | 0 | 0 | 1 |
| CORE11 | 0 | 0 | 2 | 0 |
| D18 | 0 | 0 | 2 | 2 |
| F5_M | 0 | 0 | 2 | 2 |
| FX_GPIO | 0 | 0 | 1 | 2 |
| FX_P0 | 0 | 4 | 0 | 0 |
| FX_RESET | 0 | 2 | 0 | 1 |
| FX_VDDD | 0 | 0 | 2 | 1 |
| V33 | 0 | 0 | 1 | 0 |
| **Total native gaps** | **10** | **14** | **20** | **17** |

| Other measurement | Control | Array flow | Via lattice | Both |
| --- | ---: | ---: | ---: | ---: |
| Native dangling tracks/vias | 9/1 | 26/0 | 12/5 | 20/2 |
| Exact geometry violations | 0 | 0 | 0 | 0 |
| Maximum-length warnings | 5 | 10 | 6 | 10 |
| Vias | 370 | 339 | 350 | 335 |
| Track length, mm | 2604.114253 | 2688.545934 | 2523.932771 | 2566.740397 |
| Wall seconds (parallel) | 1180.422 | 1110.450 | 1084.275 | 1003.395 |

All retain zero native islands. Control PCB bytes match P6 exactly, SHA-256
`f280e9b744b0cfefafb3d6c9eedae07b6989db18e508348e1dd8439275e6a95a`.
The old one-trial feedback run had rejected its move, so its retained geometry
is also a valid identity control; wall times are not comparable performance
evidence. Source hash is unchanged after all four runs. No default was changed.

Array flow's additional GND gap is H3; N4/N6 remain. Both closes N4/N6 despite
neither receiving a lattice via at the initial planning step. The recurring
FX_V33 obligation is C_FX10_J3.1, although the opposite reported fragment changes.
ADC_D0 is connected at 41.646350/32.672465/37.773559 mm in array/via/both,
against 35 mm; only via lattice satisfies that individual length. Array flow
also reports ADC bus skew 15.125268 mm against 5 mm. Via/both bus checks remain
blocked by open members, not measured compliant. Both's connected-only bus
span is not a complete skew measurement. TX1 connector members shorten from
12.485174 mm each to about 11.009299/10.945834 mm but remain above 8 mm.

These experiments reproduce the historical pattern: all allocators recover
the three ordinary package accesses, but none improves whole-board continuity.
There is a concrete integration discrepancy before selecting source repairs:
via planning includes lattice copper in the bypass frame (`flow.rs` around
1834), while witness refresh/binding (around 1969 and 550) omits that prefix.
The via/both arms report stale supply witnesses and bind as keepouts; control
binds as copper. The exact missing witness edge is not exposed, so individual
new losses cannot yet be assigned to that discrepancy. Correct/retest this
integration before judging via allocation's full potential or attributing all
new supply/analog failures to the source floorplan.

Source redesign candidates, with measured motivation (line numbers in unchanged
`CopperProbe/board.copper`):

1. Joint ADC/FX10 poses (603–605), support and bus exit order, rather than only
   moving a resistor. The recovered D0 exit becomes overlength in two arms;
   other bus members trade opens/length/skew. Preserve the real capture timing
   requirements while reviewing authored 35 mm/two-via/5 mm-skew allocations
   (723–751); the measurements do not justify merely increasing those limits.
2. Repack coherent FX10 capacitor/feeder cohorts. Fixed bank poses (924–966,
   1140–1145) override the broad pin-associated backside regions (968–1012).
   Regressing P0/GPIO/VDDD/CORE11 and recurring J3 contacts motivate a coordinated
   supply/return/escape layout after witness integration is corrected. Keep
   filtered rails separate and make their distribution intent explicit: comments
   at 1080–1082 describe islands, but 1083–1089 only permit routing layers;
   they do not declare filtered-rail zones. Earlier blanket island trials also
   regressed, so this is not a recommendation to enable them globally.
3. Pair USB capacitor orientation/alignment within the near-receptacle region
   (TX1:1029), and revise TX2's fixed poses (1033/1035). RX1 and TX1 IC never
   close in any arm, while connector lengths violate their allocations in all.
   Preserve the hybrid Parts contract's complete path, skew, impedance and
   return obligations; review segment splits (757–765) with the whole channel.
4. Reconcile SENSE's five-terminal analog layout (631–639) with its 4 mm budget
   (829); every arm still measures 9.87 mm. That requires analog placement or
   requirement review, not another BGA allocator.

The macro binding at 1148 and imported protected rectangles also exclude all
four currently tested N4/N6 via sites. This is bounded candidate-set evidence,
not proof of infeasibility: historical scoped Python runs connected N4/N6/N8,
and the current combined arm closes both native gaps. Reference dogbone centers
also lie inside those rectangles, but its 0.4064 mm lands are below the current
0.45 mm fabrication-profile minimum. Joint ground/USB macro redesign remains
an optional validated experiment, not permission to shrink vias or delete
protection. Fresh native/reference views were inspected in the arm view folders;
the 169-pad alignment is exact, while reference stackup-role/import caveats remain.

The user requested parallel implementation. Three agents prepared ordinary
access, USB goal-tail and native-contact scheduling changes; the primary agent
prepared bounded placement admission. These are separate candidates under
validation, not a claim that the remaining opens are fixed:

- Ordinary access filters by raw radius before direction-weighted ranking,
  preserving ordering within the radius, exact checks and attempt/miss/state caps.
  Regressions require the previously skipped legal endpoint to be searched.
- USB adds at most two analytic four-leg goal tails for a 135-degree heading
  change, after existing candidates. The fixture tests rotations/reflections,
  exact pair clearance and completion within one existing expansion. TX1's
  incompatible-collar fallback remains a separate unproved hypothesis.
- Native bridge proposals rotate between distinct findings within each net,
  then between nets. Geometry dedup retains the original first proposal's
  direction and native identities; changing those would change the bounded
  search lattice. Total trials and acceptance/rollback checks stay unchanged.
- Placement newly admits non-successful coarse guides only when failure sets,
  overflow and terminal coverage do not regress. Existing successful-guide
  admission is preserved, including region-only guides; both paths still use
  the complete detailed score for retention. New typed global terminal coverage
  includes zero-access pads even when selected routes are empty. Old checkpoints
  omit this metadata; one cached fresh baseline recheck supplies it before
  comparing candidate placements. Legacy checkpoint serialization remains exact.

Synthetic placement tests exercise real proposals/global screening with a
controlled tail callback: rerun bounds, fixed poses, equal/worse-score rollback,
strict improvement retention and the legacy baseline refresh. They do not prove
real detailed routing improvement. Cross-review also corrected proposal-direction
deduplication and found the empty-selected-access limitation before validation.

The release type check passes (`build/p6-parallel-check-final.log`); the first
check caught a decoder error-type conversion, corrected before this pass.
Coordinated release tests are in `build/p6-parallel-tests.log`. The baseline
executable is retained under `build/p6-source-controls/diagnostics`; source copies
there and under `parallel-candidate` permit controlled comparisons without
discarding earlier uncommitted work. The fixed-input USB-cohort baseline is
`build/p6-before-usb-cohort`, using all seven USB groups and 60000 pair states.
The selected release tests pass: 226 route unit, 19 report unit, 4 public CLI,
7 mechanical/checkpoint integration and 19 global-routing integration tests
(275 total). The first integration invocation omitted `COPPER_UNIFIED_CHECKPOINT`;
the corrected invocation passes using the original saved input, with no code
change (`build/p6-parallel-integration-tests.log`). Fresh KiCad bridge acceptance,
legacy placement reuse, typed terminal inventory and placement-control fixtures
all pass. Formatting/diff checks pass.

Final independent reviews found no concrete correctness blocker in these four
changes. The USB candidate-prefix guarantee applies at each visited frontier;
an added earlier solution can still change the full route selected. Native
scheduling fairness covers distinct unseen proposal geometries within the trial
cap, not an unconditional attempt for every contact. These scope limits remain
part of the full-board interpretation.

`build/p6-parallel-ordinary` now attempts four local escapes for every target.
All exhaust their reachable frontier: ACQ_EN 426 states each; FX_V33
1650/1754/1754/1782; SWDIO 243 each; ADC_D0 760/680/760/680. Four consecutive misses
stop each search, within the unchanged bounds. The selection defect is fixed,
but these additional candidates do not close the four nets: PCB bytes remain
identical to P5. The USB-cohort replay also remains byte-identical to its control
(`e8a1fdf502f6223512ea3738cbd29c3770702318e24f3fa62298b51a20c53e19`):
6 of 7 groups connect, with USB_DM/DP failing. Its uniform 60000-state setting
does not reproduce the full pipeline's mixed-stage failure context, so this is
regression evidence, not RX1/TX1 closure.

Small ladder results remain byte-identical: LED ring 50/51, zero exact DRC,
47 vias, 591.871 mm (112.1 s); nRF52 35/35, zero exact DRC, 42 vias,
347.531 mm (109.5 s). Outputs are `build/p6-parallel-led` and
`build/p6-parallel-nrf`; both report no regressions. Concurrent work prevents
interpreting their elapsed-time differences as a performance comparison.

Full-board/native evaluation completed in `build/p6-parallel-full` with the
same reused placement and balanced six-layer profile, bounded to one detailed
congestion-feedback trial for this first admission/cost experiment. It took
2361.8 s (39m22s), exceeding the 30-minute target; P5 took 1273.9 s with no
detailed placement rerun. Default four-trial behavior and the complete consumer
ladder remain unmeasured, so this candidate is not ready to merge. The exact
candidate executable SHA-256 is
`f1e44c4fb4a6bf3d9bc3d348a582ff0effe343c33756981552c0e3bbdb764f9a`.

Final native opens remain 10 (eight signal gaps and two GND gaps), with nine
dangling tracks, one dangling via, zero exact geometry violations and five
length warnings. Both D18 bridges remain accepted. The final PCB SHA-256 is
`f280e9b744b0cfefafb3d6c9eedae07b6989db18e508348e1dd8439275e6a95a`.
Connected TX1 connector traces shorten from about 14.52 to 12.49 mm each but
still exceed 8 mm. Total explicit copper is 2604.114253 mm, 2266 tracks and
370 vias. No remaining connection closed.

The placement gate refreshed 509 terminal-coverage entries from the legacy
checkpoint and admitted the R_ACQ_EN move (0,-1 mm): the same three global
failures and zero overflow, with coverage preserved. Its full score had the
same leading failure counts, length 1448.693512 rather than 1436.637405 mm,
and 90 rather than 101 vias. Length precedes vias, so the move was rejected
and the incumbent retained. This proves admission and real rollback, not
placement improvement; the expensive unhelpful rerun motivates better proposal
selection and runtime control before default multi-trial use.

Native scheduling now gives N6 trials 3/5/7 and N4 trials 4/6/8; P5 spent all
six on N6. All exhaust search frontiers before their caps, with zero final
connection attempts. N6 takes 87 states; N4 takes 223/951/223 states per
snapshot, identical with all or only protected copper. Bounded samples include
FX_USB18 vias/pads, USB_RX1P copper and reservation regions; these are not an
exhaustive obstruction proof. Fresh native F.Cu/B.Cu views and the 169-pad,
zero-RMS manufacturer overlay are `copper-probe/gnd-reference.svg` and `.png`.

RX1 still uses 24 searches/60000 states with no accepted candidate. Its nearest
B.Cu attempt remains 0.659280 mm from the goal. The added turn family changes
that tail rejection from lane geometry to exact copper clearance: the 145 um
RX1M segment from (89.595701,21.604299) to (89.147500,21.156098) mm hits a GND
track at x=89.009999, y=20.424999..21.924999 mm. Tail candidates increase
912 to 923 and clearance rejections 2 to 13. The GND object is snapshot-locked;
authored ownership and whether it may legally move remain unresolved. TX1 IC
still has zero compatible surface collar pairs and zero main search states.

Read-only follow-up while the full run evaluates these candidates:

- TX1's simple terminal lists are both nonempty and same-sign. `ports()` only
  calls local fallback for an empty list, so joint incompatibility never requests
  an alternative. A compatibility-triggered, deduplicated bounded preparation
  experiment is warranted only after a fixture proves a legal opposite-sign
  collar. No such repair has been implemented or proved on the held full context.
- Ordinary local escape only generates tracks on the pad's layer; main A* does
  support vias. The reference dogbone/via and absent current ACQ target via
  motivate checking local pad-to-site and via/landing legality separately. The
  current observations do not prove a legal missing via or identify all blockers.
- P1 source review confirms TX1 capacitors are movable in their authored region,
  whereas TX2 capacitors are fixed. Connector-side length overruns are retained
  connected warning fallbacks (TX1 14.521642/8 mm, TX2 20.374396/16 mm). Recorded
  tuning contributions (2.010752/0.785598 mm) do not account for the complete
  excess. Preserve whole-channel allocations, via allowance and unresolved
  package contributions; independent segment-budget increases are not a repair.
  SENSE's five fixed terminals, front-only/no-via/4 mm contract and 9.87 mm result
  still need a valid conductive-set feasibility proof or reviewed analog source
  placement. No source budgets, fixed poses or electrical claims were changed.

### Parallel investigation findings (2026-10-30)

Three agents examined USB, supply/return, and ordinary-net/placement failures.
They used the saved run and selected code; no source changes or reroutes were
made. Fresh `usb-agent-*`, `power-agent-*`, and `ordinary-agent-*` CLI views
are in the existing run's `views/` directory. Priorities from this first pass:

1. **Native acceptance identity bug (P3, strongest bounded correctness fix).**
   D18 trials in native verification-002/003 reduce opens from 12 to 11 with
   zero islands and the same 10 dangling tracks plus one dangling via, but
   are rejected. `filled_feedback.rs:135–144,827` compares hard findings by
   type and UUID; `copper-pcb/src/kicad_pcb.rs:1289,1305` derives track/via
   UUIDs from vector indices. Three dangling objects have unchanged full
   geometry/net/layer but changed UUIDs: D18 `(79.75,18)→(80.375,18.625)`,
   VBUS `(132.94,18.75)→(131.94,18.75)`, and ACQ_EN via `(28.8625,5.95)`.
   First reproduce index renumbering in an acceptance regression. Fix identity
   comparison with exact mapped geometry and occurrence-aware ownership;
   changed geometry, genuinely new violations and increased multiplicity must
   still reject. Do not replace the invariant with warning-count comparison.
2. **Ordinary stop reasons are lost (P0/P4).** `unified_router/astar.rs:255,427`
   returns the same failure after hitting the expansion cap and exhausting the
   frontier; `search.rs:228` says only “no path,” and `route.rs:1025` hardcodes
   `search_budget_exhausted=false`. Preserve stop reason, expansions, and
   terminal-group access counts before choosing a repair or larger budget.
   ACQ_EN/ADC_D0/SWDIO had candidate access nodes; zero access is not established.
3. **USB failures need different experiments (P0/P2).** TX1 has nonempty collar
   sets with the same lane sign at both ends, so no compatible pair reaches
   search. Four F.Cu→B.Cu signal-via attempts fail clearance. RX1 exhausts its
   aggregate bound, but a retained B.Cu attempt approaches within 0.659280 mm
   of the goal; inspect goal-tail attachment rather than attributing everything
   to connector launch. Preserve proved macro ports and lane-sign legality.
   `immutable=true` in blocker diagnostics means snapshot locking, not proof
   of authored fixed/macro ownership (`routing_clearance.rs:459–462,1207`).
4. **Supply and feedback scope corrections.** GND N4/N6 have underlying fill
   but no connecting escape; their 87–91-state trials did not exhaust the
   4,000-state bound. FX_V33 has no declared distribution zone and is deliberately
   outside native-zone repair, so reproduce its bypass/ordinary failure.
   V33's explicit route was rejected but no final native V33 open was observed.
   Four retained placement proposals fail the absolute global-success gate
   (`congestion_feedback.rs:1103`); zero detailed reruns is real. Report their
   global failure deltas before changing that eligibility rule.
5. **SENSE feasibility is still unproved (P1).** It has five required pads,
   including R_TIP.2 and R_PROTECT.1. The earlier two-endpoint reasoning omitted
   free conductive intermediates. Its 9.87 mm route violates the 4 mm contract,
   but a sound lower-bound proof or source correction is still required.

Manufacturer geometry has now been recovered for fresh package-exit comparisons
(section 5). These findings do not establish full routing closure or qualification.

This is an execution plan for the failures measured on CopperProbe with
`rust-m2` revision `916f0bff1f2e5650421312bbc3681a7a2c4dd7e2`. The
[routing refactor plan](routing-refactor-plan.md) remains authoritative.
Work on joint package side/layer/order follows the
[escape planning plan](escape-planning-plan.md), including one PR per step,
its perturbation measurements and status updates. Do not start a competing
escape planner or revive the retired preparation/gate cascade.

Generic diagnostics and algorithms belong in CopperScript. Board placement,
electrical budgets and source zones belong in CopperProbe. Reusable package
contracts belong in CopperProbeParts if a change is justified. This work does
not expand CopperLib. Source edits and engine edits are separate changes.

## 1. Baseline and interpretation

The completed benchmark is labelled `rust-m2-916f0bf-full`. Local outputs are
under `build/copperprobe-full-rust-m2/` in the assessment checkout:

- `results.json` and `results.md`: command, compiler revision and measurements.
- `copper-probe/route-report.json`, its summary, `route.log`, exported PCB and
  native verification directories: authoritative results and rejected trials.
- `checkpoints/placement.state.json`, `critical-inputs.state.json`,
  `critical-retry-inputs.state.json`, `final-board.state.json`: saved stages.
- `views/`: outputs of `copper issues` and `copper view` for this same run.

These are existing, ignored benchmark outputs, not a new evidence bundle.
Do not copy board assets or historical evidence into compiler source control.
Before implementation, verify input identities and the target branch revision;
if either changes, establish a separately labelled baseline.

| Measurement | Baseline |
| --- | --- |
| Route outcome | Failed; a complete best-effort draft was exported |
| Signal connections | 8 open / 237; completion 0.9662 |
| Internal geometric DRC error count | 0 |
| Native KiCad findings | 12 unconnected items, 10 dangling tracks, 1 dangling via |
| Length-budget findings | 5 |
| Copper | 2,262 tracks, 370 vias, 2,598.993 mm |
| Wall time | 1,998.2 s (33 min 18 s) |
| Placement/global planning | Approximately 571 s |

The signal summary, explicit-copper open nets and native unconnected items
measure different things. Do not compare their counts as interchangeable or
present 96.62% as engineering qualification. Zero geometric errors excludes
the outstanding connectivity and length-budget findings.

Initial ordinary fanout reported zero pending escapes. This did not prove
continuation, paired transitions or plane connectivity. `ADC_DCO` failed during
an early critical pass but is absent from the final native open list; do not
plan a repair from that stale intermediate finding.

## 2. Failure register

IDs below refer to this baseline only; regenerate IDs for later runs.

| Failure | Established observation | Question to resolve before changing behavior |
| --- | --- | --- |
| USB RX1, C2 / N8–N9 | Connector B10/B11 remain open; transition/launch proposals fail clearance or keepouts; coupled search exhausts 60,000 states | Is a legal paired launch missing from candidate generation, excluded by layer/contact modeling, or obstructed by mutable copper? |
| USB TX1 IC segment, C1 / N10–N11 | Fixed FX10 breakout exists; no compatible paired continuation/transition is accepted; four signal-via proposals are blocked | Can the existing macro ports support a legal same-layer continuation or joint signal/return transition under current placement? |
| FX10 ground, N1 | N4 and N6 do not join the ground network after native refill | Which physical pad/track/via/filled-fragment connection is missing, and what owns its blockers? |
| D18, N4–N5 | Separate native gaps near ADC pins 2/35 and the FX10 C7 area | Is the missing element a supply feeder, a layer transition, or a contact to a filled fragment? |
| FX_V33, N6 | C_FX10_J3.1 is not joined to the supply copper | Can a legal local supply contact coexist with the USB breakout and return contacts? |
| ADC_D0, ACQ_EN, SWDIO, R1 | One remaining pad group per net cannot be reached; native opens confirm them | Is failure at terminal admission, exact local access, or the path between terminals? |
| V33, R4 | An exact-commit proposal is rejected against ADC_D2 near (68.25, 24.00) mm on In3.Cu | Does rejection information reach negotiation? Native final opens do not list V33; first reconcile the stage result with final filled connectivity |
| SENSE, B1 | 9.87 mm against 4 mm; fixed endpoint centers span almost 8 mm vertically | What is the intended electrical budget, and can the authored fixed layout satisfy it? |
| USB TX1/TX2 connector segments, B2–B5 | Both TX1 conductors: 14.52 / 8 mm; both TX2: 20.37 / 16 mm | Does length exceed the limit before tuning, after tuning, or when fixed/support copper is included? |
| Placement feedback | Four logged candidates end `global_failed`; zero detailed reruns and no accepted moves | Are valid local improvements vetoed by unchanged, unrelated global-access failures? Report screened proposals separately from detailed reruns |

Three coarse guide failures concern GND, FX_VDDD and FX_USB18. They are not
three additional proven final signal failures. Diagnose candidate generation
versus shared transition-capacity admission, and reconcile with final native
connectivity without suppressing unresolved findings.

## 3. Ordered work packages

Split independently reviewable fixes into separate PRs. A package is a work
sequence, not permission for one large cross-repository patch.

### P0 — Trustworthy diagnostics and bounded reproduction

1. Use `copper issues` then `copper view` as the default investigation path.
   Fix the Rust viewer's reference rendering, filled-polygon rendering, layer
   clipping and capability messages in the existing renderer. At baseline it
   computed alignment but discarded reference geometry, omitted fill polygons,
   and always printed a misleading Pillow fallback message. Implemented and
   checked as described in the status above.
2. Distinguish accepted-board findings from rejected-trial diagnostics; retain
   the latter with their trial/context identity. A rejected pair's clearance
   errors are not shorts in the accepted draft.
3. Reconcile stage and final status for V33 and ADC_DCO; distinguish screened
   placement proposals from executed detailed reruns in the trial counts.
   Report the full set of 69 decoupling obligations by supply/return/native
   status; `19/69 verified surface connections` alone is not a coverage verdict.
4. Retain existing placement/critical snapshots. Capture typed unified inputs
   through `--unified-save-inputs` for ordinary-net experiments; the baseline
   did not save them. If a Rust save/replay path is unsupported, fix that
   public diagnostic contract rather than fabricate a state or disable fanout.

**Accept:** native gaps can be located on the actual filled board; a reference
overlay visibly contains transformed geometry; omitted capabilities are
explicit; stage-to-final status is explainable. Diagnostic changes produce
identical routing copper and no extra solver work on the benchmark ladder.

### P1 — Resolve source-budget feasibility before solver tuning

1. In CopperProbe, review the source and engineering basis for SENSE's 4 mm
   limit and all five fixed terminals: R_TIP.2, R_INPUT.1, C_COMP_TOP.2,
   C_COMP_BOTTOM.1 and R_PROTECT.1. Calculate a conservative lower bound for the
   actual track-centerline-length accounting, including free traversal through
   every same-net pad/internal contact. Center distances and direct distances
   between two pad areas alone are insufficient. If incompatible, make an intentional source placement correction
   supported by the analog design, or document a justified budget correction.
   Do not silently move fixed parts or simply increase the limit to 9.87 mm.
2. In CopperScript, add an advisory, located length-feasibility diagnostic for
   contradictions that can be proved. Derive and test an admissible bound over
   the complete conductive-set model, allowing free intermediate conductors.
   A largest direct pairwise pad/group distance or an MST of pad centers can
   overstate required track length. Unproved cases remain unknown.
3. Audit USB segment budget accounting across macro copper, support ties,
   generated route and skew tuning. Preserve the source's end-to-end intent.
   Never create a passing result by moving length between accounting buckets.

**Accept:** SENSE has a reviewed feasible source contract; the diagnostic
identifies only proven contradictions and otherwise reports unknown, without blocking unrelated routing;
every accepted USB segment satisfies its actual length and skew limits after
all post-processing. No blanket limit, width, clearance or via-count relaxation.

### P2 — Legal USB launches and macro-port continuation

1. Reproduce RX1 and TX1 IC failures separately with `replay-critical`, using
   the saved original and retry inputs. Record port locations, lane order,
   admissible headings/layers, candidate signal/return vias, exact rejection
   reasons and blocker ownership. A single-pair replay is diagnostic; it may
   omit competing generated pairs and must not be called full-context closure.
2. Compare the actual geometry with the manufacturer board. For TX1, evaluate
   same-layer continuation from existing ports before assuming a front-to-back
   transition is required. For RX1, inspect both connector through-hole lands
   and the FX10 departure. Check approach direction and paired lane order at
   both ends, all allowed layers and required return-via reach.
3. Improve generic candidate families only where a legal opportunity is
   demonstrated: native plated-contact launches, finite port extensions,
   paired transitions and joint signal/return-site selection. Feed mutable
   blocker ownership into existing negotiation; do not add a recovery module
   or a selector for a CopperProbe net, pin or coordinate.
4. In a separate CopperProbe change, evaluate a small, bounded set of
   source-backed coupling-capacitor regions/orientations and USB corridors.
   The reference's nearby front-layer TX1 capacitors are a comparison, not an
   instruction to copy its floorplan. Preserve analog/mechanical intent and
   evaluate the complete IC-to-cap-to-connector path. Source-fixed TX2 parts
   may move only through an explicit source change.

**Accept:** RX1 and TX1 IC pairs connect with the other USB pairs present;
length, skew, gap, via limits and required returns pass; immutable macro copper
is unchanged. Neither ADC/power completion nor native findings regress. If
joint side/layer/order changes are necessary, implement them under the relevant
E4/E6/E12 escape-plan step and its measurement protocol.

### P3 — Supply and return continuity beside the package

1. Trace each N1/N4/N5/N6 native open through mapped physical objects and
   filled fragments. Classify missing local tap, disconnected feeder, layer
   transition, or fill isolation. Preserve distinct fragments even when their
   zone UUID or net name matches.
2. Compare FX10 N4/N6, C7 and J3 supply/return topology, support placement and
   via sites against the aligned manufacturer package. Inspect complete USB
   and decoupling context, not an isolated free-space pad.
3. Extend existing exact tap/contact candidates or negotiated feeder routing
   for the demonstrated cause. Reuse legal existing vias; regenerate mutable
   contacts transactionally, with fresh ownership and complete rollback.
   Do not enable the via lattice or power islands by default on the strength
   of a local win: prior escape-plan measurements show whole-board regressions.
4. Change declared zones or layer roles only through reviewed consumer source
   intent. Do not reintroduce automatic area-preparation gates or infer that
   a zone outline proves continuity.

**Accept:** all four grouped native supply/return issues disappear, every
affected decoupler retains its supply and return connection, and accepted USB
exits remain usable. No new opens, islands, geometry violations or lost required
via-array members appear in the full native check. Broader package-lattice
changes follow E7/E8/E11, one step per PR.

### P4 — Ordinary access and exact-commit negotiation

1. On typed unified inputs, inspect ADC_D0, ACQ_EN and SWDIO terminal groups
   and legal access candidates. Separate zero candidates, capacity rejection,
   raster reachability and exact-commit rejection in public diagnostics.
2. Where a legal path is missed, improve the existing exact access nodes,
   terminal-to-grid handoff or blocker negotiation. Preserve two-end bus
   ordering, length-match intent and the already connected data lines.
3. For V33/ADC_D2, prove whether the rejection is stale or still relevant to
   the final board. If still relevant, ensure the exact witness influences
   the next bounded negotiation round; preserve immutable obstacles and the
   best incumbent. Do not route through the clearance conflict.

**Accept:** ADC_D0, ACQ_EN and SWDIO close in the complete scene; V33 has a
consistent final status and native connectivity; no other ADC bus member or
power connection is lost. Package access changes use E8/E9/E10 contracts where
applicable, rather than duplicating those mechanisms.

### P5 — Make placement feedback discriminate useful proposals

1. Use the recorded four `global_failed` candidates to test the hypothesis
   that an unrelated coarse-guide failure vetoes a locally useful move.
   Record before/after affected nets, access capacity, source legality and
   global failures; do not assume this hypothesis is already established.
   The current `congestion_feedback.rs` requires global `Success` before
   calling `rerun`; its `spent` counter counts detailed reruns, explaining
   the baseline's four proposal events but zero spent trials.
2. Keep coarse guides as a bounded screening signal. Where safe and useful,
   permit the best legal candidates with unchanged unrelated guide failures
   to reach detailed evaluation; newly lost access, overflow and geometric
   violations remain explicit. Keep the existing candidate and reroute caps.
3. Score only validated candidate outcomes and restore the complete prior
   placement/copper/dependency state after rejection. Fixed poses, rigid
   groups, macro bindings and required arrays remain authoritative.

**Accept:** synthetic positive and negative cases distinguish unrelated guide
failures from actual regressions; actual trial counts match reports; a consumer
move is retained only on measured whole-board improvement. Extend E6 for
escape-aware placement feedback rather than adding a second controller.

### P6 — Final copper quality, runtime and integration

1. Recheck all five length violations after USB/source changes. Shorten or
   retune only with exact connectivity, pair-profile and ownership checks.
   Validate length again after smoothing, contact completion and native repair.
2. Classify the 10 dangling tracks and one dangling via by ownership. Use
   existing native dead-end cleanup only for eligible mutable copper. Repair
   required or immutable dangling connections; do not delete them to clear
   the report. Respect the refactor plan's bounded final native-repair policy.
3. Profile the now-correct pipeline. Prioritize placement/global screening,
   decoupling work and repeated critical attempts from measured inclusive and
   exclusive timings; do not sum nested event durations. Improve reuse only
   with exact input/context keys and invalidation.
4. Run the full benchmark ladder and fresh consumer source builds, then
   update implemented contracts, affected escape-plan status rows and measured
   baselines. Keep missing client test/simulation coverage explicitly open.

**Accept:** CopperProbe has zero unintended native opens, zero hard geometric
DRC findings, no dangling findings, all declared length/skew/return rules met,
and all required supply/return obligations verified, in at most 30 minutes on
the comparison machine. CopperLedRing and CopperAssetTracker do not regress;
cm4, full-vertical and nrf52 meet the parent plan's completion/time targets.
Routing acceptance does not imply SI/PDN, analog, assembly, CAM or prototype
qualification.

## 4. Execution and decision rules

- Start with P0 and the P1 source audit. Diagnose P2/P3 next in the same fixed
  complete-board context, then implement one demonstrated fix at a time.
  P4 follows the selected USB/power geometry; do not optimize against stale
  obstacles. P5 can be developed once its recorded failure is reproducible.
- For each solver change: a failing regression that detects removal of the
  fix, a bounded target replay, a fresh CopperProbe ladder run, and the full
  ladder before merge. Preserve the actual consumer `led-ring` and
  `asset-tracker` entries; `round-led-ring` is not a substitute.
- Measure original-input engine changes separately from consumer source
  changes. When compiler, dependency pins, source or physical rules change,
  old output is historical. The consumer's local parts replacement and all
  cached inputs must be available and byte-identified for reproduction.
- For escape heuristics use the existing five-run perturbation protocol and
  report median/spread and persistent failing nets. Hold geometry and work
  bounds fixed. A single favorable run is not a default-on decision.
- Reuse placement only when the CLI validates its inputs/options. Consumer
  placement changes require fresh placement. Do not pass `final-board.state`
  to `replay-unified`; it is not the typed unified-input contract.
- Separate known facts, hypotheses and acceptance results in each PR. A
  search bound exhausted is not geometric impossibility. Larger budgets are
  controlled diagnostic experiments, not the default repair strategy.
- Stop pursuing an approach if it merely trades open nets, violates fixed
  intent, requires blanket rule relaxation, or repeatedly loses on the ladder.
  Retain its measured result and choose another generic approach.
- Keep outputs in normal fresh `build/` benchmark runs. No new `verification/`
  bundles, one-off state codecs, board-specific engine branches or external
  publication are part of this plan.

## 5. Reference and replay entry points

Manufacturer comparison: KIT_FX10_FMC_001, PCB Design Data 01_00,
schematic 630-60643-01 Rev03, CYUSB4014-FCAXI. CopperProbe uses
CYUSB4013-FCAXI: same FCA169 package family, different device/board context.
The previously inspected local archived import was
`CopperProbe/archive/verification/fx10-dvk-escape-review-20261015/fx10-dvk.kicad_pcb`,
SHA-256 `0048ab472f58ceb4c9013a0dce9bc1d15f0445a03b4d96df2023e039a921d3a2`.
`U_USB:U1` matches 169 pads at 0 mm fit RMS, with a 180-degree rotation.
On resuming P0.1, this PCB was absent from that path and was not found in the
other copper repositories searched. The official source was subsequently recovered
from [Infineon's PCB design archive](https://www.infineon.com/assets/row/public/documents/30/60/infineon-hardware-design-file-kit-fx10-fmc-001-pcbdesigndata-en.zip).
The ZIP SHA-256 is `6b7d809118948fa75f4b492e990568a1aee4a45b02282b92a47ad51c81dab022`;
the Allegro PCB SHA-256 is `e19be5cf29b7922ec30895eaeb4ce1c2563ef54096aaa70fad18fd55619e0144`.
Both exactly match retained historical source metadata. KiCad CLI 10.0.6
`pcb import --format auto` produced `build/reference-inputs/fx10-dvk.kicad_pcb`,
SHA-256 `28baa67ef19402fc14e00ce3df255f4fce38cb361a7ca4c27e6494b0fc6add1d`.
Fresh `copper view --reference ... --align U_USB:U1` again matches all 169 pads
at 180 degrees and 0 mm RMS. The separate-layer overlay was visually inspected:
`build/copperprobe-full-rust-m2/views/usb-agent-recovered-reference.png`.
The importer reports 363 footprints versus 334 in retained measurements; both
have 12,786 tracks, 1,025 vias and 127 zones. Historical import equivalence is
not established. The retained Allegro through-via span caveat still needs a
source-padstack check before adopting via structures. Import command and source
provenance are in `build/reference-inputs/fx10-source-import-provenance.json`.
The earlier P0.1 saved-export overlay was not a manufacturer comparison.
Fresh comparison confirms front-only, zero-via TX1 ball-to-capacitor routes on
the kit, and 0.20 mm F.Cu N4/N6 ground dogbones offset −0.3328 mm in x/y into
0.4064/0.2032 mm land/drill vias. The latter connect to B.Cu capacitor returns
and underlying In1.Cu ground fill. CopperProbe's saved N4/N6 gap lacks the local
pad-to-via links despite underlying fill. These are observations, not selected
repair geometry. The kit's near-PHY TX1 capacitor placement is not transferable
without resolving applicability: the active FX10 contract specifies near-USB-C
placement for cable-connected devices. CopperProbe's TX1 capacitors have generated
poses inside the authored front region x=124–131, y=17–22 mm, rather than fixed
poses. IC-side `_IC` nets have a 63.6 mm limit; the saved 14.52/8 mm failures are
the distinct connector-side `USB_TX1M`/`USB_TX1P` nets (B2/B3).
The existing import/alignment does not establish transferable stackup,
clearance, fabrication or electrical qualification. Follow the
[reference comparison requirements](component-integration-guidelines.md#package-escape-faults-reference-comparison-is-required)
before selecting a package-exit repair.

Use the exact tested Rust executable via `copper` on PATH and absolute KiCad
and footprint paths as needed. CLI help is authoritative over historical
wrapper flags. Example diagnostic commands from the assessment checkout:

```console
copper issues build/copperprobe-full-rust-m2/copper-probe --stage native
copper view build/copperprobe-full-rust-m2/copper-probe --issue C2
copper view build/copperprobe-full-rust-m2/copper-probe --issue N1 --filled
copper replay-critical build/copperprobe-full-rust-m2/checkpoints/critical-inputs.state.json --only USB_RX1M --pair-states 60000 --report build/rx1-trial/route-report.json -o build/rx1-trial/board.kicad_pcb
copper bench bench/ladder.json --only copper-probe --out build/closure-candidate --label closure-candidate --kicad-cli KICAD_CLI
```

Set `COPPER_BOARDS` to the consumer checkout parent and `KICAD_FOOTPRINTS`
to the installed footprint library. Use a fresh output path for each run.
The critical replay keeps the pair's source contract but is not equivalent
to the complete stage order or native full-board verification. Unsupported
Rust replay/view paths remain explicit until P0 addresses them.
