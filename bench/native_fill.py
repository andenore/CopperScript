"""Profile the public native-fill decoder through workbench, without routing.

Run this same harness with PYTHONPATH selecting the before/after checkout.
It never modifies the PCB, heals a polygon, or establishes board qualification.
"""
from pathlib import Path
from hashlib import sha256
from time import perf_counter
import argparse
import json
import pcbir.rf_geometry as decoder
from pcbir.workbench import scene_of


def grid_fill(count):
    anchors=[(3+6*x,3+6*y) for y in range(count) for x in range(count)]
    extent=6*count+2
    walk=[(0,0),(extent,0),(extent,extent),(0,extent),(0,0),anchors[0]]
    for i,(x,y) in enumerate(anchors):
        walk.extend(((x,y+2),(x+2,y+2),(x+2,y),(x,y)))
        if i+1<len(anchors): walk.append(anchors[i+1])
    walk.extend((*reversed(anchors[:-1]),(0,0)))
    return {'outer':walk,'holes':[]}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pcb',type=Path)
    parser.add_argument('--grid',type=int,default=20)
    parser.add_argument('--limit',type=int,default=1)
    parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args()
    if min(args.grid,args.limit)<=0 or args.report.exists():
        parser.error('positive bounds and a fresh report path are required')
    if args.pcb:
        scene=scene_of(args.pcb)
        regions=sorted((a for a in scene.areas if a.kind=='fill'),key=lambda a:len(a.outer),reverse=True)[:args.limit]
        # Match the native verifier's fixed nanometre coordinate projection.
        fills=[(a.net,a.layer,{'outer':[(round(x*1000000),round(y*1000000)) for x,y in a.outer],
            'holes':[[(round(x*1000000),round(y*1000000)) for x,y in ring] for ring in a.holes]}) for a in regions]
    else:
        fills=[('synthetic','fixture',grid_fill(args.grid))]
    rows=[]
    for net,layer,fill in fills:
        started=perf_counter()
        shape=decoder.native_fill_polygon(fill)
        seconds=perf_counter()-started
        rows.append({'net':net,'layer':layer,'vertices':len(fill['outer']),'seconds':seconds,
                     'area':shape.area,'geometry_sha256':sha256(shape.normalize().wkb).hexdigest(),
                     'input_sha256':sha256(json.dumps(fill,sort_keys=True).encode()).hexdigest()})
    report={'scope':'native-fill decode profile only; no routing or electrical/CAM qualification',
            'qualified_release':False,'decoder_path':decoder.__file__,
            'decoder_sha256':sha256(Path(decoder.__file__).read_bytes()).hexdigest(),
            'pcb_sha256':sha256(args.pcb.read_bytes()).hexdigest() if args.pcb else None,'regions':rows}
    args.report.parent.mkdir(parents=True,exist_ok=True)
    args.report.write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
    print(json.dumps(report))


if __name__=='__main__':
    main()
