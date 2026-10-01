# Routing review after physical-land closure integration

Reviewed 2026-10-01. Implementation: `c1d5e52` (R12).
Previous: [routing-review-pass2.md](routing-review-pass2.md).
Work list: [routing-review-todo.md](routing-review-todo.md).

## Outcome

The matched rerun preserves **zero KiCad unconnected items**, including all
package grounds and shields. Independent KiCad 10.0.6 refill/DRC reports no
short, clearance or dangling findings. The same eight library findings remain:
four library issues and four copy mismatches. All 313 tests pass, including
16 additions since pass 2 and an installed-KiCad single-logical-pin land-closure
case. No check was waived and no component was manually moved.

Feedback now evaluates closed physical lands. The accepted current-placement
trial reports zero signal failures; pass 2 reported USER_BUTTON before its final
land stitching. Its 16 bridge segments are installed inside the selected
pipeline before scoring; final stitching adds zero tracks. J_POWER.SH/J_SIM.SH
remain visible as surface-helper pending work but are separately resolved by
fresh board/export-bound independent filled-zone evidence.

This fixes candidate/reporting consistency, not routing aesthetics or electrical
qualification. Native explicit-copper checking still defers GND, route status
remains incomplete, and overall plane verification fails the eight library
findings. `fabrication_ready` remains false. USB/RF profiles are still absent.

## Implemented contract

The stitcher shares native DRC's exact physical-contact graph. Existing pad-edge,
interior-track, via and multilayer connections are reused; repeated numbers
alone do not connect lands. Only disconnected physical groups receive new,
bounded, clearance-checked surface bridges, respecting width and allowed layers.
Even one logical pin can contain physically disconnected lands and fail DRC.

Full and subset transactions perform closure before scoring. Added ordinary
copper and fresh actual opens update metrics, metadata, status and fingerprints.
Failed search flags, resource overflow and zone deferrals are not promoted to
success. Implicit default zone deferral also avoids futile placement retries.

Final reports distinguish `surface_pending_pads`, effective `pending_pads` and
`zone_verified_pads`. Reconciliation requires fresh board/export-bound evidence,
zero unconnected items/islands, and no non-library violations. It only resolves
declared zone-net references, never ordinary nets. Library-only findings do not
erase a zero-open observation, but remain overall signoff failures. Native DRC
findings and signoff tokens are not rewritten or waived.

## Matched run and independent evidence

Inputs/options match pass 2: 100 x 80 mm, candidate-01, six-layer JLCPCB profile,
1 mm pitch, two passes, 20,000-state search budget, 10x failed-net repair, fanout,
soft rip-up, constrained-pins-first, progressive guides, 5 mm plane-contact
radius, qualified GND via-in-pad, four full zone trials and six local attempts.
Source electrical connectivity, placement constraints and critical profiles
were unchanged. All six local attempts were rejected; the first full
current-placement/nearest-escape trial was accepted, with no early or late
pending plane pads and no failed signals.

The timing-only external wrapper records 1,364.5 s through independent KiCad
verification (about 22.7 minutes, excluding final file-write overhead).
Initial placement/global: 171.4 s; initial detailed: 422.0 s; initial land
closure/native DRC: 10.0 s. Accepted feedback placement/global: 58.1 s;
detailed: 562.5 s; closure/native DRC: 5.3 s. Repeated tests and the saved-board
replay ran concurrently. This is not a controlled performance benchmark; R9's
normal progress/checkpoint support remains unfinished.

The draft and companion project were copied to disposable `latest` artifacts,
then independently checked again with `pcb drc --format json --severity-all
--refill-zones --save-board`. Actual geometry was extracted with installed KiCad
Python, measured and rendered. Original user-owned drafts remain untouched.

Artifacts are outside the repository at:
`C:/Users/anden/Documents/Codex/2026-09-17/referenced-chatgpt-conversation-this-is-an/outputs/routing-review-pass3-r12-2026-10-01/`.
They include raw DRC, draft/refilled boards and projects, geometry, layer analysis,
six-layer/central/U_CC renders, phase timings and provenance. The earlier-board
replay is also retained: it adds no copper and resolves the two shields through
fresh verification without reusing an old signoff token.

## Repeated geometry and layer review

All 51 placements/rotations, 171 vias, 471 pads and 681 physical track segments
match pass 2. One GND segment has reversed start/end ordering; normalizing this
undirected representation gives zero track differences. Physical-board digest
is unchanged. Export/report digests need not be identical because serialization
order and generated identities differ; no historical token is reused.

| Layer | Signal length | Review retained from pass 2 |
| --- | ---: | --- |
| F.Cu | 643.95 mm | Dense modem-level and MCU exits persist. Busiest tiles remain x35–40/y30–35 (22.83 mm, six nets) and x45–50/y40–45 (20.23 mm, nine nets). |
| In1.Cu | 0 | Dedicated filled GND plane; zero independent ground opens, no ordinary tracks. |
| In2.Cu | 516.95 mm | Dominant signal layer; broad perimeter detours and the rail/enable hotspot near x55–60/y25–30 remain. |
| In3.Cu | 223.57 mm | Sparse, with Nordic-access density and long horizontal runs. Effective fallback policy still needs local/reportable handling. |
| In4.Cu | 148.72 mm | Sparse; CAN_RX/GNSS_RF share the horizontal corridor near x50–60/y45–50. RF cannot be moved just to balance density. |
| B.Cu | 128.79 mm | Sparse overall, with ACCEL_INT/MODEM_PWRKEY_CTL/USER_BUTTON in the MCU-side hotspot. |

The six-layer mosaic and U_CC access were inspected, and central crops were
regenerated. Density tiles remain proxies, not proof of channel-capacity
overflow. Pads in these diagnostic renders are rectangular approximations.
Signal length remains approximately 1,661.98 mm; front GND length 194.19 mm;
degree-two 90/45-degree turns 95/136; missing reported escape endpoints zero.

Detailed-stage metrics now include the previously final-only USER_BUTTON bridge:
463 tracks and 1,543.643271 mm versus 462 and 1,537.343271 mm. This accounting
change adds one 6.3 mm bridge to that stage's totals, not to actual final copper.
Stage totals exclude locked input and plane copper; the independently extracted
all-signal geometry above remains the quality-comparison reference.

MCU_MODEM_TX remains 3.65x its pad-separation baseline, MODEM_STATUS 3.60x its
pad-MST baseline, and USER_LED_2_A 2.66x. Long right-angle branches, orthogonal
fallback and sparse eligible layers still justify R5–R7. Global overflow remains
zero despite locally busy access channels; the critical route list is empty.

## Next

R12 is complete within its implemented scope. Next is R8: explicitly integrate
USB/RF profiles and protected repair behavior in this real-footprint example,
without inventing impedance/length limits from net names. Then address local
demand/fallback (R6), bounded feasible-route improvement (R5), protected tree
cleanup (R7) and operational timing/checkpoints (R9). Footprint, fabrication,
return-path and independent CAM qualification remain separate production gates.
