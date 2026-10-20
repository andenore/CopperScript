/*
 * Grid A* kernel for pcbir.unified_router (routing refactor plan, P6).
 *
 * Pure computation with no C runtime dependency, so the same source builds
 * a Linux shared object and a Windows DLL without an MSVC installation:
 *
 *   python tools/build_kernel.py
 *
 * The Python router owns every array and passes them in; nothing here
 * allocates. Semantics match _Router._astar exactly (same costs, same
 * tie-breaking order, same legality rules).
 */
#include <stdint.h>

#ifdef _WIN32
#define EXPORT __declspec(dllexport)
int _fltused = 0;
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

static inline double octile(int x, int y, int tx0, int ty0, int tx1, int ty1) {
    int hx = x < tx0 ? tx0 - x : (x > tx1 ? x - tx1 : 0);
    int hy = y < ty0 ? ty0 - y : (y > ty1 ? y - ty1 : 0);
    return hx > hy ? hx + 0.41421356 * hy : hy + 0.41421356 * hx;
}

/* Returns the path length written to out_path (source first), 0 when no
 * target was reached, -1 when a scratch buffer overflowed. */
EXPORT int pcbir_astar(
    int nx, int ny, int nl,
    const int32_t *owner, const int32_t *mid_owner, const int32_t *via_owner,
    const double *hist, const int32_t *occ, const double *via_hist, const int32_t *via_occ,
    const uint8_t *flags, int nid,
    const int32_t *layers, int n_layers,
    const double *move_base, /* nl * 8, matching the move order below */
    double present, double via_cost,
    int bx0, int by0, int bx1, int by1, int tx0, int ty0, int tx1, int ty1,
    const int32_t *sources, const double *source_cost, int n_sources,
    const double *target_extra, /* per node, or 0 */
    int max_expansions,
    double *best, int32_t *parent,
    entry_t *heap, int heap_cap,
    int32_t *touched, int touched_cap, int *n_touched,
    int32_t *out_path, int out_cap)
{
    static const int DX[8] = {1, -1, 0, 0, 1, 1, -1, -1};
    static const int DY[8] = {0, 0, 1, -1, 1, -1, 1, -1};
    const int plane = nx * ny;
    const double INF = 1e30;
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
            if (touched_count >= touched_cap) { *n_touched = touched_count; return -1; }
            touched[touched_count++] = node;
        }
        best[node] = source_cost[i];
        parent[node] = -1;
        if (heap_size >= heap_cap) { *n_touched = touched_count; return -1; }
        entry_t e = {source_cost[i] + octile(rest % nx, rest / nx, tx0, ty0, tx1, ty1), source_cost[i], node};
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
            if (heap_size >= heap_cap) { *n_touched = touched_count; return -1; }
            entry_t t = {cost + target_extra[node], cost + target_extra[node], -node - 1};
            heap_push(heap, &heap_size, t);
        }
        if (++expansions > max_expansions) break;
        int layer = node / plane, rest = node % plane;
        int y = rest / nx, x = rest % nx;
        int target_only = allowed[layer] == 2;
        int here_in_region = flags[node] & FLAG_REGION;
        const double *base = move_base + layer * 8;
        for (int m = 0; m < 8; m++) {
            int x2 = x + DX[m], y2 = y + DY[m];
            if (x2 < bx0 || x2 > bx1 || y2 < by0 || y2 > by1) continue;
            int n2 = node + DY[m] * nx + DX[m];
            if (target_only && !(flags[n2] & FLAG_TARGET)) continue;
            int o = owner[n2];
            if (o != -1 && o != nid && !(here_in_region && (flags[n2] & FLAG_NECK))) continue;
            if (DX[m] && DY[m]) {
                int cell = node + (DY[m] > 0 ? 0 : -1) * nx + (DX[m] > 0 ? 0 : -1);
                o = mid_owner[cell];
                if (o != -1 && o != nid && !(here_in_region && (flags[n2] & FLAG_REGION)
                                              && (flags[cell] & FLAG_NECK_MID))) continue;
            }
            double c = cost + base[m] * (1.0 + hist[n2]) * (1.0 + present * occ[n2]);
            if (c < best[n2]) {
                if (best[n2] >= INF) {
                    if (touched_count >= touched_cap) { *n_touched = touched_count; return -1; }
                    touched[touched_count++] = n2;
                }
                best[n2] = c;
                parent[n2] = node;
                if (heap_size >= heap_cap) { *n_touched = touched_count; return -1; }
                entry_t ne = {c + octile(x2, y2, tx0, ty0, tx1, ty1), c, n2};
                heap_push(heap, &heap_size, ne);
            }
        }
        int vo = via_owner[rest];
        if (vo != -1 && vo != nid) continue;
        double vpen = via_cost * (1.0 + via_hist[rest]) * (1.0 + present * via_occ[rest]);
        double h = octile(x, y, tx0, ty0, tx1, ty1);
        for (i = 0; i < n_layers; i++) {
            int layer2 = layers[i] & 0xff;
            if (layer2 == layer) continue;
            int n2 = layer2 * plane + rest;
            if ((layers[i] & 0x100) && !(flags[n2] & FLAG_TARGET)) continue;
            int o = owner[n2];
            if (o != -1 && o != nid) continue;
            double c = cost + vpen * (1.0 + hist[n2]) * (1.0 + present * occ[n2]);
            if (c < best[n2]) {
                if (best[n2] >= INF) {
                    if (touched_count >= touched_cap) { *n_touched = touched_count; return -1; }
                    touched[touched_count++] = n2;
                }
                best[n2] = c;
                parent[n2] = node;
                if (heap_size >= heap_cap) { *n_touched = touched_count; return -1; }
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
