"""Measure public cross-layer witness verification on saved routing inputs.

Read-only benchmark, built on workbench; no routing or qualification authority.
Run with the same input and link bound under both compiler revisions.
"""
import argparse
from pathlib import Path
from hashlib import sha256
import json
from time import perf_counter
from pcbir.workbench import load_state
from pcbir.decoupling_paths import verify_crosslayer_decoupling
from pcbir.routing_clearance import RoutingClearanceIndex


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('state', type=Path)
    parser.add_argument('--limit', type=int, default=8)
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if args.limit <= 0 or args.report.exists():
        parser.error('positive link bound and fresh report path required')
    saved = load_state(args.state)
    board = saved['board'] if isinstance(saved, dict) else saved.board
    index = RoutingClearanceIndex(board)
    poses = {p.reference: p for p in board.placements}
    links = [link for link in sorted(board.decoupling_links, key=lambda item: str(item.capacitor))
             if poses[link.capacitor.component].side is not poses[link.target.component].side][:args.limit]
    rows = []
    for link in links:
        started = perf_counter()
        result = verify_crosslayer_decoupling(board, link, index=index)
        seconds = perf_counter()-started
        paths = (result.supply_path, result.return_path)
        rows.append(dict(capacitor=str(link.capacitor), seconds=seconds,
            reason=result.reason, witness=bool(result.witness),
            path_sha256=sha256(repr(paths).encode()).hexdigest(),
            path_steps=[len(path.steps) if path else None for path in paths]))
        print(json.dumps(rows[-1]), flush=True)
    import pcbir.decoupling_paths as module
    document = dict(scope='Read-only path verification timing, no routing or qualification acceptance',
        source_sha256=sha256(args.state.read_bytes()).hexdigest(),
        module_sha256=sha256(Path(module.__file__).read_bytes()).hexdigest(), links=rows,
        seconds=sum(row['seconds'] for row in rows))
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(document, indent=2)+'\n', encoding='utf-8')


if __name__ == '__main__':
    main()
