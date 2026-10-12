"""Soft, footprint-local outward preferences for newly generated lead-ins.

These are geometric rankings, not electrical constraints or connectivity proof.
Central/through-hole lands have no inferred exit direction. Coincident lands
with conflicting directions are deliberately left unranked.
"""
from .physical import BoardSide, CopperLayer, PadKind, PadReference, Point, nm_from_mm
from .placement import transformed_local_point


class TerminalPreferences:
    def __init__(self, board):
        self.normals = {}
        assigned = {pad: net.name for net in board.nets for pad in net.pads}
        ambiguous = set()
        unit = nm_from_mm(1)
        for pose in board.placements:
            fp = board.footprints[pose.footprint]
            layer = CopperLayer.FRONT if pose.side is BoardSide.FRONT else CopperLayer.BACK
            for pad in fp.pads:
                net = assigned.get(PadReference(pose.reference, pad.number))
                if net is None or pad.kind is not PadKind.SMD:
                    continue
                x, y = pad.position.x_nm, pad.position.y_nm
                if not (x or y):
                    continue
                # Compare distance from the centre in footprint-local axes,
                # normalized by body size, then transform the chosen normals.
                ax, ay = abs(x)*fp.body_size.height_nm, abs(y)*fp.body_size.width_nm
                offsets = []
                if ax >= ay and x:
                    offsets.append(Point(x + (unit if x > 0 else -unit), y))
                if ay >= ax and y:
                    offsets.append(Point(x, y + (unit if y > 0 else -unit)))
                start = transformed_local_point(pose, pad.position)
                normals = tuple((end.x_nm-start.x_nm, end.y_nm-start.y_nm)
                                for end in (transformed_local_point(pose, p) for p in offsets))
                key = (net, layer, start)
                if key in self.normals and self.normals[key] != normals:
                    ambiguous.add(key)
                self.normals[key] = normals
        for key in ambiguous:
            del self.normals[key]

    def score(self, net, layer, points):
        """Rank both outward departure and reversed arrival; lower is better."""
        if len(points) < 2:
            return 0

        def departure(start, end):
            normals = self.normals.get((net, layer, start))
            if not normals:
                return 0
            dx, dy = end.x_nm-start.x_nm, end.y_nm-start.y_nm
            def rank(normal):
                nx, ny = normal
                dot = dx*nx + dy*ny
                if dot < 0:
                    return 3
                if dot == 0:
                    return 2
                return 0 if dx*ny == dy*nx else 1
            return min(map(rank, normals))

        return departure(points[0], points[1]) + departure(points[-1], points[-2])

    def ordered(self, net, layer, paths):
        # Stable sort preserves the original cheap-path order on equal scores.
        return sorted(paths, key=lambda points: self.score(net, layer, points))
