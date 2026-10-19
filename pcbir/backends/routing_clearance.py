"""Export source net clearances for native DRC and zone refill.

KiCad uses the last matching custom rule, not the maximum matching rule. Sort
by effective clearance so a pair of constrained nets retains the stricter value.
Breakout intent has no region in the native representation: export its lower
bound conservatively and retain CopperScript's authoritative region checks.
"""
import json
from decimal import Decimal


def routing_clearance_rules(board):
    values = []
    for rule in board.net_routing_rules:
        if rule.clearance_nm is None:
            continue
        clearance = rule.clearance_nm
        if rule.breakout_clearance_nm is not None:
            clearance = min(clearance, rule.breakout_clearance_nm)
        clearance = max(clearance, board.rules.minimum_clearance_nm)
        if clearance > board.rules.minimum_clearance_nm:
            values.append((clearance, rule.net))
    lines = []
    for clearance, net in sorted(values):
        # Exact string equality; source names are not netclass wildcard patterns.
        literal = net.replace('\\', '\\\\').replace("'", "\\'")
        condition = f"A.NetName == '{literal}' || B.NetName == '{literal}'"
        mm = format(Decimal(clearance) / Decimal(1_000_000), 'f')
        lines.extend([
            f'(rule {json.dumps("CopperScript clearance " + net)}',
            f'  (constraint clearance (min {mm}mm))',
            f'  (condition {json.dumps(condition)}))',
        ])
    return tuple(lines)
