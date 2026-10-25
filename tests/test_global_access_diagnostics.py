"""Global failures distinguish geometry search from capacity admission."""
from pcbir import routing
from pcbir.global_terminals import terminal_access_failures
from test_detailed_routing import _board


def test_global_report_preserves_capacity_rejection_cause(monkeypatch):
    import pcbir.global_terminals as terminals
    real = terminals.register_terminal_capacity

    def reject_one(board, graph, accesses):
        graph, accepted = real(board, graph, accesses)
        key = next(key for key, choices in accesses.items() if choices)
        return graph, {**accepted, key: ()}

    monkeypatch.setattr(terminals, "register_terminal_capacity", reject_one)
    result = routing.route_global(_board())
    route = result.routes[0]
    assert not route.connected
    assert route.diagnostics[0].startswith("no legal bounded local/global access")
    assert any("shared transition capacity rejected every candidate" in d for d in route.diagnostics)
    assert not any("generated 0 candidate(s)" in d for d in route.diagnostics)
    assert "shared transition capacity" in result.to_json()


def test_global_report_preserves_empty_generation_cause(monkeypatch):
    monkeypatch.setattr(routing, "_global_pin_access_candidates", lambda *args: ())
    result = routing.route_global(_board())
    assert not result.routes[0].connected
    assert sum("local escape search found no candidate" in d for d in result.routes[0].diagnostics) == 2
    assert all("generated 0 candidate(s), registered 0" in d
               for d in result.routes[0].diagnostics[1:])


def test_partial_capacity_admission_is_not_reported_as_an_empty_terminal():
    generated = {("N", "A"): (1, 2), ("N", "B"): (), ("N", "C"): (3,)}
    accepted = {("N", "A"): (2,), ("N", "B"): (), ("N", "C"): ()}
    before = generated.copy(), accepted.copy()
    assert terminal_access_failures(generated, accepted) == {
        ("N", "B"): (0, 0), ("N", "C"): (1, 0)}
    assert (generated, accepted) == before
