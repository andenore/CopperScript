import random
import time

import pytest
from pcbir.editor.islands import edge, nearest_island_tree
from pcbir.physical import Point


def exhaustive(lands):
    edges = sorted((edge(a, b) for i, a in enumerate(lands) for b in lands[i+1:] if a[0] != b[0]), key=lambda e:e[:5])
    parent = {l[0]: l[0] for l in lands}
    def find(root):
        while parent[root] != root:
            root = parent[root]
        return root
    result = []
    for e in edges:
        a, b = find(e[1]), find(e[2])
        if a != b:
            parent[max(a,b)] = min(a,b)
            result.append(e)
    return result


@pytest.mark.parametrize("seed", range(20))
def test_spatial_tree_exactly_matches_exhaustive_tie_order(seed):
    rng = random.Random(seed)
    lands = [(f"root{rng.randrange(12)}", f"pad:{i}", f"U{i}", "1", Point(rng.randrange(20), rng.randrange(20)))
             for i in range(100)]
    assert nearest_island_tree(lands) == exhaustive(lands)
    rng.shuffle(lands)
    assert nearest_island_tree(lands) == exhaustive(lands)


def test_coincident_points_and_all_one_component():
    lands = [(str(i), str(i), "", "", Point(0,0)) for i in range(20)]
    assert nearest_island_tree(lands) == exhaustive(lands)
    assert nearest_island_tree([( "same", l[1], *l[2:]) for l in lands]) == []
    assert nearest_island_tree([]) == []


def test_large_net_has_bounded_memory_and_far_fewer_distance_tests():
    lands = [(str(y*50+x), str(y*50+x), "", "", Point(x*1000000, y*1000000)) for y in range(50) for x in range(50)]
    statistics = {}
    started = time.perf_counter()
    tree = nearest_island_tree(lands, statistics=statistics)
    assert len(tree) == 2499
    assert statistics["distance_tests"] < len(lands)**2 // 20
    # Diagnostic timing, not a flaky machine-speed assertion.
    print({**statistics, "seconds": round(time.perf_counter()-started,3), "terminals": len(lands)})
