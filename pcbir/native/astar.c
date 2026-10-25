/*
 * Grid A* kernel for pcbir.unified_router (routing refactor plan, P6;
 * multi-resolution lattice plan, S3).
 *
 * Pure computation with no C runtime dependency, so the same source builds
 * a Linux shared object and a Windows DLL without an MSVC installation:
 *
 *   python tools/build_kernel.py
 *
 * The Python router owns every array and passes them in; nothing here
 * allocates. Semantics match _Router._astar exactly on a uniform lattice and
 * _Router._astar_nested on a nested one (same costs, same tie-breaking
 * order, same legality rules).
 *
 * Lattices (pcbir.routing_lattice). Node ids are layer * plane + column. A
 * uniform lattice is one dense nx * ny block. A nested lattice passes its
 * block table: per block the first column, nx and ny; per column the block
 * index (block_of), the coordinates in base pitches from the base origin
 * (ux, uy) and the portal twin column (twins, -1 for none); per block the
 * 8 move lengths in base pitches (steps). Planar moves stay inside a node's
 * block; a twin is reached at cost 0 on the same layer.
 */
#include <stdint.h>

#ifdef _WIN32
#define EXPORT __declspec(dllexport)
int _fltused = 0;
/* Freestanding DLL entry for cross-compilers that require one. The usual
   /noentry build does not call it; this kernel has no runtime to initialise. */
int _DllMainCRTStartup(void *module, unsigned long reason, void *reserved) {
    (void)module; (void)reason; (void)reserved;
    return 1;
}
#else
#define EXPORT __attribute__((visibility("default")))
#endif

/* Freestanding build: the compiler may emit calls to these. */
#include <stddef.h>
void *memset(void *dest, int value, size_t count) {
    volatile unsigned char *p = (volatile unsigned char *)dest;
    while (count--) *p++ = (unsigned char)value;
    return dest;
}
void *memcpy(void *dest, const void *src, size_t count) {
    volatile unsigned char *d = (volatile unsigned char *)dest;
    const unsigned char *s = (const unsigned char *)src;
    while (count--) *d++ = *s++;
    return dest;
}

#define FLAG_NECK 1
#define FLAG_NECK_MID 2
#define FLAG_REGION 4
#define FLAG_TARGET 8

typedef struct {
    double key;
    double g;
    int32_t node; /* negative: completed target marker */
} entry_t;

/* Same order as Python's heap of (key, g, node) tuples, so both searches
 * break ties identically. */
static inline int entry_less(const entry_t *a, const entry_t *b) {
    if (a->key != b->key) return a->key < b->key;
    if (a->g != b->g) return a->g < b->g;
    return a->node < b->node;
}

static void heap_push(entry_t *heap, int *size, entry_t value) {
    int i = (*size)++;
    while (i > 0) {
        int parent = (i - 1) >> 1;
        if (!entry_less(&value, &heap[parent])) break;
        heap[i] = heap[parent];
        i = parent;
    }
    heap[i] = value;
}

static entry_t heap_pop(entry_t *heap, int *size) {
    entry_t top = heap[0];
    entry_t last = heap[--(*size)];
    int i = 0;
    int n = *size;
    for (;;) {
        int child = 2 * i + 1;
        if (child >= n) break;
        if (child + 1 < n && entry_less(&heap[child + 1], &heap[child])) child++;
        if (!entry_less(&heap[child], &last)) break;
        heap[i] = heap[child];
        i = child;
    }
    if (n > 0) heap[i] = last;
    return top;
}

/* Uniform lattice heuristic: integer octile over index boxes. */
static inline double octile(int x, int y, int tx0, int ty0, int tx1, int ty1) {
    int hx = x < tx0 ? tx0 - x : (x > tx1 ? x - tx1 : 0);
    int hy = y < ty0 ? ty0 - y : (y > ty1 ? y - ty1 : 0);
    return hx > hy ? hx + 0.41421356 * hy : hy + 0.41421356 * hx;
}

/* Nested lattice heuristic: the octile distance in base pitches from a
 * column's point (u, v) to the target box. key_f is the heap key of a cost
 * c in _astar_nested's evaluation order, (c + max) + 0.41421356 * min; h_f
 * is the bare distance max + 0.41421356 * min (added as c + h). */
static inline double key_f(double c, double u, double v, double tx0, double ty0, double tx1, double ty1) {
    double hx = u < tx0 ? tx0 - u : (u > tx1 ? u - tx1 : 0.0);
    double hy = v < ty0 ? ty0 - v : (v > ty1 ? v - ty1 : 0.0);
    return hx > hy ? (c + hx) + 0.41421356 * hy : (c + hy) + 0.41421356 * hx;
}

static inline double h_f(double u, double v, double tx0, double ty0, double tx1, double ty1) {
    double hx = u < tx0 ? tx0 - u : (u > tx1 ? u - tx1 : 0.0);
    double hy = v < ty0 ? ty0 - v : (v > ty1 ? v - ty1 : 0.0);
    return hx > hy ? hx + 0.41421356 * hy : hy + 0.41421356 * hx;
}

#define OVERFLOW() do { *n_touched = touched_count; return -1; } while (0)

/* The search. ``nested`` is a constant at both call sites and the function
 * is always inlined, so the uniform specialisation is the single-block
 * kernel with its former arithmetic. */
static inline __attribute__((always_inline)) int search(
    const int nested,
    int nx, int ny, int nl,
    const int32_t *owner, const int32_t *mid_owner, const int32_t *via_owner,
    const double *hist, const int32_t *occ, const double *via_hist, const int32_t *via_occ,
    const uint8_t *flags, int nid,
    const int32_t *layers, int n_layers,
    const double *move_base,
    double present, double via_cost,
    int bx0, int by0, int bx1, int by1, int tx0, int ty0, int tx1, int ty1,
    const int32_t *sources, const double *source_cost, int n_sources,
    const double *target_extra, const double *turn, const double *guide,
    const int32_t *const *via_transitions,
    int max_expansions,
    double *best, int32_t *parent,
    entry_t *heap, int heap_cap,
    int32_t *touched, int touched_cap, int *n_touched,
    int32_t *out_path, int out_cap,
    int n_blocks, const int32_t *blocks, const uint8_t *block_of,
    const double *ux, const double *uy, const int32_t *twins, const double *steps)
{
    static const int DX[8] = {1, -1, 0, 0, 1, 1, -1, -1};
    static const int DY[8] = {0, 0, 1, -1, 1, -1, 1, -1};
    /* Move lengths in pitches; the diagonal is Python's sqrt(2.0). */
    static const double STEP[8] = {1.0, 1.0, 1.0, 1.0, 1.4142135623730951, 1.4142135623730951,
                                   1.4142135623730951, 1.4142135623730951};
    const int last = 3 * (n_blocks - 1);
    const int plane = nested ? blocks[last] + blocks[last + 1] * blocks[last + 2] : nx * ny;
    const double INF = 1e30;
    /* Boxes as doubles for the nested comparisons with ux, uy (exact). */
    const double fbx0 = bx0, fby0 = by0, fbx1 = bx1, fby1 = by1;
    const double ftx0 = tx0, fty0 = ty0, ftx1 = tx1, fty1 = ty1;
    int heap_size = 0, touched_count = 0, expansions = 0, found = -1, i;
    uint8_t allowed[64];
    for (i = 0; i < nl && i < 64; i++) allowed[i] = 0;
    /* A layer index with bit 8 set may be entered only on target nodes. */
    for (i = 0; i < n_layers; i++) allowed[layers[i] & 0xff] = (layers[i] & 0x100) ? 2 : 1;

    for (i = 0; i < n_sources; i++) {
        int node = sources[i];
        int layer = node / plane, rest = node % plane;
        if (!allowed[layer]) continue;
        if (source_cost[i] >= best[node]) continue;
        if (best[node] >= INF) {
            if (touched_count >= touched_cap) OVERFLOW();
            touched[touched_count++] = node;
        }
        best[node] = source_cost[i];
        parent[node] = -1;
        if (heap_size >= heap_cap) OVERFLOW();
        double key = nested ? key_f(source_cost[i], ux[rest], uy[rest], ftx0, fty0, ftx1, fty1)
                            : source_cost[i] + octile(rest % nx, rest / nx, tx0, ty0, tx1, ty1);
        entry_t e = {key, source_cost[i], node};
        heap_push(heap, &heap_size, e);
    }
    while (heap_size > 0) {
        entry_t e = heap_pop(heap, &heap_size);
        int node = e.node;
        double cost = e.g;
        if (node < 0) { found = -node - 1; break; }
        if (cost > best[node]) continue;
        if (flags[node] & FLAG_TARGET) {
            if (!target_extra) { found = node; break; }
            if (heap_size >= heap_cap) OVERFLOW();
            entry_t t = {cost + target_extra[node], cost + target_extra[node], -node - 1};
            heap_push(heap, &heap_size, t);
        }
        if (++expansions > max_expansions) break;
        int layer = node / plane, rest = node % plane;
        /* The node's block: its row width, height and block-local index. */
        int b = 0, nxb = nx, nyb = ny, local = rest;
        if (nested) {
            b = block_of[rest];
            local = rest - blocks[3 * b];
            nxb = blocks[3 * b + 1];
            nyb = blocks[3 * b + 2];
        }
        int y = local / nxb, x = local % nxb;
        int target_only = allowed[layer] == 2;
        int here_in_region = flags[node] & FLAG_REGION;
        /* A neck-only node must not be left towards a node outside the region
           (that step would be drawn at full width). */
        int here_neck_only = here_in_region && (flags[node] & FLAG_NECK);
        const double *base = move_base + (nested ? (b * nl + layer) * 8 : layer * 8);
        const double *step = nested ? steps + 8 * b : STEP;
        /* Turn cost from the heading of this node's best parent (none after
           a via, a portal or at a source); the same approximation as the
           Python search. On a nested lattice the parent must lie in the
           node's block. */
        const double *turn_row = 0;
        if (turn) {
            int p = parent[node];
            if (p >= 0 && p / plane == layer && (!nested || block_of[p - layer * plane] == b)) {
                int d = node - p;
                for (int m = 0; m < 8; m++)
                    if (d == DY[m] * nxb + DX[m]) { turn_row = turn + 8 * m; break; }
            }
        }
        for (int m = 0; m < 8; m++) {
            int x2 = x + DX[m], y2 = y + DY[m];
            int dn = DY[m] * nxb + DX[m];
            if (nested) {
                if (x2 < 0 || x2 >= nxb || y2 < 0 || y2 >= nyb) continue;
                double u = ux[rest + dn], v = uy[rest + dn];
                if (u < fbx0 || u > fbx1 || v < fby0 || v > fby1) continue;
            } else {
                if (x2 < bx0 || x2 > bx1 || y2 < by0 || y2 > by1) continue;
            }
            int n2 = node + dn;
            if (target_only && !(flags[n2] & FLAG_TARGET)) continue;
            if (here_neck_only && !(flags[n2] & FLAG_REGION)) continue;
            int o = owner[n2];
            if (o != -1 && o != nid && !(here_in_region && (flags[n2] & FLAG_NECK))) continue;
            if (DX[m] && DY[m]) {
                int cell = node + (DY[m] > 0 ? 0 : -1) * nxb + (DX[m] > 0 ? 0 : -1);
                o = mid_owner[cell];
                if (o != -1 && o != nid && !(here_in_region && (flags[n2] & FLAG_REGION)
                                              && (flags[cell] & FLAG_NECK_MID))) continue;
            }
            double c = cost + base[m] * (1.0 + hist[n2]) * (1.0 + present * occ[n2]);
            if (turn_row) c += turn_row[m];
            if (guide) c += step[m] * guide[n2];
            if (c < best[n2]) {
                if (best[n2] >= INF) {
                    if (touched_count >= touched_cap) OVERFLOW();
                    touched[touched_count++] = n2;
                }
                best[n2] = c;
                parent[n2] = node;
                if (heap_size >= heap_cap) OVERFLOW();
                double key = nested ? key_f(c, ux[rest + dn], uy[rest + dn], ftx0, fty0, ftx1, fty1)
                                    : c + octile(x2, y2, tx0, ty0, tx1, ty1);
                entry_t ne = {key, c, n2};
                heap_push(heap, &heap_size, ne);
            }
        }
        double h = nested ? h_f(ux[rest], uy[rest], ftx0, fty0, ftx1, fty1) : octile(x, y, tx0, ty0, tx1, ty1);
        if (nested) {
            /* Portal: the coincident node of the other block, at no cost (no
               heading, turn or guide term); the target-only, neck and owner
               rules still apply to the twin. */
            int t = twins[rest];
            if (t >= 0) {
                int n2 = node - rest + t;
                int o = owner[n2];
                if (!(target_only && !(flags[n2] & FLAG_TARGET))
                        && !(here_neck_only && !(flags[n2] & FLAG_REGION))
                        && (o == -1 || o == nid || (here_in_region && (flags[n2] & FLAG_NECK)))
                        && cost < best[n2]) {
                    if (best[n2] >= INF) {
                        if (touched_count >= touched_cap) OVERFLOW();
                        touched[touched_count++] = n2;
                    }
                    best[n2] = cost;
                    parent[n2] = node;
                    if (heap_size >= heap_cap) OVERFLOW();
                    entry_t ne = {cost + h, cost, n2};
                    heap_push(heap, &heap_size, ne);
                }
            }
        }
        int vo = via_owner[rest];
        if (!via_transitions && vo != -1 && vo != nid) continue;
        double vpen = via_cost * (1.0 + via_hist[rest]) * (1.0 + present * via_occ[rest]);
        for (i = 0; i < n_layers; i++) {
            int layer2 = layers[i] & 0xff;
            if (layer2 == layer) continue;
            if (via_transitions) {
                const int32_t *mask = via_transitions[layer * nl + layer2];
                if (!mask || (mask[rest] != -1 && mask[rest] != nid)) continue;
            }
            int n2 = layer2 * plane + rest;
            if ((layers[i] & 0x100) && !(flags[n2] & FLAG_TARGET)) continue;
            int o = owner[n2];
            if (o != -1 && o != nid) continue;
            double c = cost + vpen * (1.0 + hist[n2]) * (1.0 + present * occ[n2]);
            if (c < best[n2]) {
                if (best[n2] >= INF) {
                    if (touched_count >= touched_cap) OVERFLOW();
                    touched[touched_count++] = n2;
                }
                best[n2] = c;
                parent[n2] = node;
                if (heap_size >= heap_cap) OVERFLOW();
                entry_t ne = {c + h, c, n2};
                heap_push(heap, &heap_size, ne);
            }
        }
    }
    *n_touched = touched_count;
    if (found < 0) return 0;
    int length = 0;
    for (int node = found; node != -1; node = parent[node]) {
        if (length >= out_cap) return -1;
        out_path[length++] = node;
    }
    /* reverse: source first */
    for (i = 0; i < length / 2; i++) {
        int32_t t = out_path[i];
        out_path[i] = out_path[length - 1 - i];
        out_path[length - 1 - i] = t;
    }
    return length;
}

/* Returns the path length written to out_path (source first), 0 when no
 * target was reached, -1 when a scratch buffer overflowed.
 *
 * Uniform lattice: nx * ny columns per layer, move_base of nl * 8; pass
 * block_of = 0 (n_blocks, blocks, ux, uy, twins and steps are then unused).
 * Nested lattice: nx, ny are the base block's; pass
 *   n_blocks  the number of blocks (base first, at most 256),
 *   blocks    n_blocks * 3 int32: first column, nx and ny of each block,
 *   block_of  per column, its block index,
 *   ux, uy    per column, its coordinates in base pitches from the base origin,
 *   twins     per column, its portal twin column or -1,
 *   steps     n_blocks * 8 move lengths in base pitches (move order below),
 * and move_base of n_blocks * nl * 8 ([block][layer][move]). The search and
 * target boxes are base-lattice index boxes on both. */
EXPORT int pcbir_astar(
    int nx, int ny, int nl,
    const int32_t *owner, const int32_t *mid_owner, const int32_t *via_owner,
    const double *hist, const int32_t *occ, const double *via_hist, const int32_t *via_occ,
    const uint8_t *flags, int nid,
    const int32_t *layers, int n_layers,
    const double *move_base, /* see above, matching the move order of search() */
    double present, double via_cost,
    int bx0, int by0, int bx1, int by1, int tx0, int ty0, int tx1, int ty1,
    const int32_t *sources, const double *source_cost, int n_sources,
    const double *target_extra, /* per node, or 0 */
    const double *turn,         /* 8 * 8 turn costs [incoming move][move], or 0 */
    const double *guide,        /* per node: cost per pitch step entering it, or 0 */
    const int32_t *const *via_transitions, /* nl * nl span masks; null: through-only */
    int max_expansions,
    double *best, int32_t *parent,
    entry_t *heap, int heap_cap,
    int32_t *touched, int touched_cap, int *n_touched,
    int32_t *out_path, int out_cap,
    int n_blocks, const int32_t *blocks, const uint8_t *block_of,
    const double *ux, const double *uy, const int32_t *twins, const double *steps)
{
    if (block_of && n_blocks > 0)
        return search(1, nx, ny, nl, owner, mid_owner, via_owner, hist, occ, via_hist, via_occ, flags, nid,
                      layers, n_layers, move_base, present, via_cost, bx0, by0, bx1, by1, tx0, ty0, tx1, ty1,
                      sources, source_cost, n_sources, target_extra, turn, guide, via_transitions,
                      max_expansions, best, parent, heap, heap_cap, touched, touched_cap, n_touched,
                      out_path, out_cap, n_blocks, blocks, block_of, ux, uy, twins, steps);
    return search(0, nx, ny, nl, owner, mid_owner, via_owner, hist, occ, via_hist, via_occ, flags, nid,
                  layers, n_layers, move_base, present, via_cost, bx0, by0, bx1, by1, tx0, ty0, tx1, ty1,
                  sources, source_cost, n_sources, target_extra, turn, guide, via_transitions,
                  max_expansions, best, parent, heap, heap_cap, touched, touched_cap, n_touched,
                  out_path, out_cap, 1, 0, 0, 0, 0, 0, 0);
}
