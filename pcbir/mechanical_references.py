"""Locked, planar DXF guides. Never board material, copper or manufacturing intent.

Only local regular files beneath the declaring source directory are authorized.
DXF input is bounded, hashed before parsing and strict about unsupported geometry.
"""
from __future__ import annotations

from dataclasses import dataclass
from decimal import Decimal, localcontext, ROUND_HALF_EVEN
from hashlib import sha256
from math import cos, sin, radians
from pathlib import Path, PurePosixPath
import re
import stat
import os

from .physical import Point

MAX_BYTES = 1_048_576
MAX_ENTITIES = 2048
MAX_VERTICES = 8192
UNIT_FACTORS = {"mm": Decimal(1_000_000), "inch": Decimal(25_400_000)}


@dataclass(frozen=True, slots=True)
class ReferenceEntity:
    kind: str
    points: tuple[Point, ...]
    radius_nm: int = 0
    start_degrees: Decimal = Decimal(0)
    sweep_degrees: Decimal = Decimal(0)
    closed: bool = False

    def __post_init__(self):
        object.__setattr__(self, "points", tuple(self.points))
        if self.kind not in {"line", "circle", "arc", "polyline"}:
            raise ValueError("unknown reference entity")
        if not self.points or any(not isinstance(p, Point) for p in self.points):
            raise ValueError("reference entity requires typed points")
        if any(abs(v) > 10**12 for p in self.points for v in (p.x_nm,p.y_nm)):
            raise ValueError("reference coordinates exceed +/-1km")
        if self.kind == "line" and (len(self.points)!=2 or self.points[0]==self.points[1]):
            raise ValueError("reference line requires distinct endpoints")
        if self.kind in {"circle","arc"}:
            if len(self.points)!=1 or type(self.radius_nm) is not int or not 0<self.radius_nm<=10**12:
                raise ValueError("reference circle/arc requires a centre and positive bounded radius")
        if self.kind == "arc" and (not self.start_degrees.is_finite() or not self.sweep_degrees.is_finite()
                                  or not 0<self.sweep_degrees<360):
            raise ValueError("reference arc requires finite angles and sweep between 0 and 360")
        if self.kind == "polyline":
            if len(self.points)<2 or any(a==b for a,b in zip(self.points,self.points[1:])):
                raise ValueError("reference polyline requires distinct adjacent vertices")
            if self.closed:
                if len(self.points)>512:
                    raise ValueError("closed DXF polylines are limited to 512 vertices for bounded topology checks")
                from .mechanical import validated_ring
                validated_ring(self.points)


@dataclass(frozen=True, slots=True)
class MechanicalReference:
    id: str
    file: str
    sha256: str
    units: str
    frame: str
    position: Point
    rotation_degrees: Decimal
    mirror_x: bool
    side: str
    purpose: str
    entities: tuple[ReferenceEntity, ...]
    asset_path: str = ""  # Compiler authority only; never sent as an HTTP file URL.

    def __post_init__(self):
        object.__setattr__(self,"entities",tuple(self.entities))
        if not self.id or not re.fullmatch(r"[0-9a-f]{64}",self.sha256):
            raise ValueError("reference requires an identity and lowercase SHA-256")
        if self.units not in UNIT_FACTORS or self.frame not in {"cartesian","board"}:
            raise ValueError("reference requires explicit mm/inch units and cartesian/board frame")
        if self.side not in {"front","back","both"} or type(self.mirror_x) is not bool:
            raise ValueError("reference side/mirror is invalid")
        if not isinstance(self.file,str) or not isinstance(self.purpose,str) or len(self.purpose)>4096:
            raise ValueError("reference file/purpose must be bounded strings")
        if not isinstance(self.position,Point) or not isinstance(self.rotation_degrees,Decimal) or not self.rotation_degrees.is_finite():
            raise ValueError("reference requires a typed position and finite rotation")
        if any(abs(v)>10**12 for v in (self.position.x_nm,self.position.y_nm)):
            raise ValueError("reference position exceeds +/-1km")
        if not 1<=len(self.entities)<=MAX_ENTITIES or any(not isinstance(e,ReferenceEntity) for e in self.entities):
            raise ValueError("reference requires 1–2048 typed entities")


def _asset_path(filename, relative):
    if not isinstance(relative,str) or not relative or "\\" in relative or ":" in relative:
        raise ValueError("reference path must be a relative forward-slash DXF path")
    raw_parts=relative.split("/")
    if any(re.fullmatch(r"(?:CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\..*)?",p,re.I) for p in raw_parts):
        raise ValueError("reference paths cannot name Windows device files")
    path=PurePosixPath(relative)
    if path.is_absolute() or any(p in {"", ".", ".."} or p.endswith((" ",".")) for p in raw_parts) or path.suffix.lower()!=".dxf":
        raise ValueError("reference path must stay beneath its declaring source directory")
    if filename.startswith("<"):
        raise ValueError("reference assets require a source filename")
    source=Path(filename).absolute()
    # Reject linked ancestors too, including Windows directory junctions.
    target=source.parent.joinpath(*path.parts)
    for candidate in (*target.parents,target):
        info=candidate.lstat()
        if stat.S_ISLNK(info.st_mode) or getattr(info,"st_file_attributes",0)&getattr(stat,"FILE_ATTRIBUTE_REPARSE_POINT",0x400):
            raise ValueError("reference assets do not follow symbolic links/junctions")
    if not target.resolve().is_relative_to(source.parent.resolve()):
        raise ValueError("reference asset escapes its source directory")
    return target


def read_locked_asset(filename, relative, digest):
    if not isinstance(digest,str) or not re.fullmatch(r"[0-9a-f]{64}",digest):
        raise ValueError("reference requires a lowercase SHA-256 checksum")
    target=_asset_path(filename,relative)
    with target.open("rb") as stream:
        info=os.fstat(stream.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_size>MAX_BYTES:
            raise ValueError("reference asset must be a regular DXF file of at most 1MiB")
        raw=stream.read(MAX_BYTES+1)
    current=_asset_path(filename,relative).stat()
    if (current.st_dev,current.st_ino,current.st_size,current.st_mtime_ns)!=(info.st_dev,info.st_ino,info.st_size,info.st_mtime_ns):
        raise ValueError("reference asset changed while reading")
    if len(raw)>MAX_BYTES or sha256(raw).hexdigest()!=digest:
        raise ValueError("reference asset checksum mismatch or oversized file")
    return target,raw


def parse_dxf(raw: bytes, units: str) -> tuple[ReferenceEntity,...]:
    """Strict ASCII tagged subset. No INSERT, XREF, spline, OCS transform or healing."""
    if units not in UNIT_FACTORS or len(raw)>MAX_BYTES:
        raise ValueError("DXF requires explicit mm/inch units and bounded input")
    try:lines=raw.decode("ascii").splitlines()
    except UnicodeError as exc:raise ValueError("only ASCII DXF reference files are supported") from exc
    if not lines or len(lines)%2:raise ValueError("malformed DXF group pairs")
    try:pairs=[(int(lines[i].strip()),lines[i+1].strip()) for i in range(0,len(lines),2)]
    except ValueError as exc:raise ValueError("invalid DXF group code") from exc
    pairs=[p for p in pairs if p[0]!=999]
    sections={};i=0
    while i<len(pairs):
        if pairs[i]==(0,"EOF"):
            if i!=len(pairs)-1:raise ValueError("DXF has data after EOF")
            break
        if pairs[i]!=(0,"SECTION") or i+1>=len(pairs) or pairs[i+1][0]!=2:
            raise ValueError("DXF requires SECTION records and terminal EOF")
        name=pairs[i+1][1];i+=2;body=[]
        while i<len(pairs) and pairs[i]!=(0,"ENDSEC"):
            if pairs[i]==(0,"SECTION"):raise ValueError("nested DXF section")
            body.append(pairs[i]);i+=1
        if i==len(pairs):raise ValueError("unterminated DXF section")
        if name in sections:raise ValueError("duplicate DXF section")
        sections[name]=body;i+=1
    else:raise ValueError("DXF missing EOF")
    if "ENTITIES" not in sections:raise ValueError("DXF missing ENTITIES")
    if set(sections)-{"HEADER","TABLES","ENTITIES","BLOCKS"} or sections.get("BLOCKS"):
        raise ValueError("DXF blocks/objects/unsupported sections are not permitted")
    header=sections.get("HEADER",[])
    declared=[]
    for i,(code,value) in enumerate(header):
        if code==9 and value=="$INSUNITS":
            if i+1==len(header) or header[i+1][0]!=70:raise ValueError("invalid DXF INSUNITS")
            declared.append(header[i+1][1])
    if len(declared)>1 or declared and declared[0] not in {"0", "4" if units=="mm" else "1"}:
        raise ValueError("DXF header units conflict with explicit source units")
    factor=UNIT_FACTORS[units]
    def number(text):
        if len(text)>64 or not re.fullmatch(r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[Ee][+-]?\d{1,3})?",text):
            raise ValueError("invalid DXF numeric value")
        value=Decimal(text)
        if not value.is_finite() or abs(value)>Decimal(10**12):raise ValueError("DXF number outside limits")
        return value
    def length(text):
        with localcontext() as ctx:
            ctx.prec=80;value=number(text)*factor
            if value!=value.to_integral_value() or abs(value)>10**12:raise ValueError("DXF lengths must be exact bounded nanometres")
            return int(value)
    records=[]
    for code,value in sections["ENTITIES"]:
        if code==0:records.append((value,[]))
        elif not records:raise ValueError("DXF entity data precedes entity type")
        else:records[-1][1].append((code,value))
    if not 1<=len(records)<=MAX_ENTITIES:raise ValueError("DXF requires 1–2048 entities")
    entities=[];vertices=0
    for kind,groups in records:
        if kind not in {"LINE","CIRCLE","ARC","LWPOLYLINE"}:raise ValueError(f"unsupported DXF entity {kind!r}")
        common={5,6,8,62,67,100,330,370,410,420,440,210,220,230,39}
        geometry={"LINE":{10,20,30,11,21,31},"CIRCLE":{10,20,30,40},"ARC":{10,20,30,40,50,51},
                  "LWPOLYLINE":{90,70,43,38,10,20,40,41,42,91}}[kind]
        if {code for code,_ in groups}-(common|geometry):raise ValueError("unsupported DXF entity group code")
        values={}
        for code,value in groups:values.setdefault(code,[]).append(value)
        def scalar(code,default=None):
            entries=values.get(code,[])
            if len(entries)>1 or not entries and default is None:raise ValueError(f"missing/duplicate DXF scalar {code}")
            return entries[0] if entries else default
        for code,default in ((210,"0"),(220,"0"),(230,"1"),(39,"0"),(30,"0"),(31,"0"),(38,"0")):
            if number(scalar(code,default))!=Decimal(default):raise ValueError("nonplanar DXF/extrusion/thickness is unsupported")
        if kind=="LWPOLYLINE":
            if any(number(v)!=0 for c in (40,41,42,43) for v in values.get(c,[])):
                raise ValueError("DXF polyline width/bulge is unsupported")
            flags=number(scalar(70,"0"));count=number(scalar(90))
            if flags not in {0,1,128,129} or count!=count.to_integral_value() or not 2<=count<=MAX_VERTICES:
                raise ValueError("invalid DXF polyline flags/count")
            points=[];x=None
            for code,value in groups:
                if code==10:
                    if x is not None:raise ValueError("DXF polyline vertex is missing Y")
                    x=length(value)
                elif code==20:
                    if x is None:raise ValueError("DXF polyline vertex is missing X")
                    points.append(Point(x,length(value)));x=None
            if x is not None or len(points)!=count:raise ValueError("DXF polyline vertex count mismatch")
            entity=ReferenceEntity("polyline",tuple(points),closed=bool(int(flags)&1))
        else:
            start=Point(length(scalar(10)),length(scalar(20)))
            if kind=="LINE":entity=ReferenceEntity("line",(start,Point(length(scalar(11)),length(scalar(21)))))
            elif kind=="CIRCLE":entity=ReferenceEntity("circle",(start,),length(scalar(40)))
            else:
                begin,end=number(scalar(50)),number(scalar(51))
                sweep=(end-begin)%360
                if sweep<0:sweep+=360
                entity=ReferenceEntity("arc",(start,),length(scalar(40)),begin,sweep)
        vertices+=len(entity.points)
        if vertices>MAX_VERTICES:raise ValueError("DXF reference exceeds 8192 vertices")
        entities.append(entity)
    return tuple(entities)


def lower_reference(item, point):
    p=item.parameters
    required={"file","sha256","units","frame"}
    allowed=required|{"position","rotation","mirror_x","side","purpose"}
    if item.shape!="dxf" or set(p)-allowed or required-set(p):
        raise ValueError("reference dxf requires file, sha256, units and frame with optional transform/side/purpose")
    angle=p.get("rotation",0)
    if type(angle) not in {int,float} or not Decimal(str(angle)).is_finite():raise ValueError("reference rotation must be finite numeric degrees")
    target,raw=read_locked_asset(item.location.filename,p["file"],p["sha256"])
    return MechanicalReference(item.name,p["file"],p["sha256"],p["units"],p["frame"],
        point(p["position"]) if "position" in p else Point(0,0),Decimal(str(angle))%360,
        p.get("mirror_x",False),p.get("side","both"),p.get("purpose","Reference guide only"),
        parse_dxf(raw,p["units"]),str(target))


def verify_reference_assets(references):
    """Check original authority/checksum before operations and immediately before save."""
    for ref in references:
        if not ref.asset_path:raise ValueError("reference asset has no compiler authority")
        asset=Path(ref.asset_path)
        # Recheck the full chain; resolving input paths alone would hide new symlinks.
        read_locked_asset(str(asset.parent/"authority.copper"),asset.name,ref.sha256)


def validate_reference_inventory(references):
    if any(not isinstance(r,MechanicalReference) for r in references):
        raise ValueError("reference inventory requires typed mechanical guides")
    if len(references)>32 or sum(len(r.entities) for r in references)>8192 or sum(len(e.points) for r in references for e in r.entities)>16384:
        raise ValueError("reference inventory exceeds 32 assets, 8192 entities or 16384 vertices")


def reference_scene(ref):
    """Board-frame read-only guides, nanometres; no asset URL is exposed."""
    angle=ref.rotation_degrees%360
    if angle<0:angle+=360
    cs,sn={Decimal(0):(1,0),Decimal(90):(0,1),Decimal(180):(-1,0),Decimal(270):(0,-1)}.get(angle,(cos(radians(float(angle))),sin(radians(float(angle)))))
    def transform(p):
        x=-p.x_nm if ref.mirror_x else p.x_nm
        y=-p.y_nm if ref.frame=="cartesian" else p.y_nm
        with localcontext() as ctx:
            ctx.prec=60
            c,s=Decimal(str(cs)),Decimal(str(sn))
            return [int((x*c+y*s).to_integral_value(rounding=ROUND_HALF_EVEN))+ref.position.x_nm,
                    int((-x*s+y*c).to_integral_value(rounding=ROUND_HALF_EVEN))+ref.position.y_nm]
    entities=[]
    for e in ref.entities:
        record={"kind":e.kind,"points":[transform(p) for p in e.points],"radius_nm":e.radius_nm,"closed":e.closed}
        if e.kind=="arc":
            center=e.points[0]
            def endpoint(degrees):
                a=radians(float(degrees))
                # Endpoint computation is display only, never legal board geometry.
                return transform(Point(round(center.x_nm+e.radius_nm*cos(a)),round(center.y_nm+e.radius_nm*sin(a))))
            record.update(start=endpoint(e.start_degrees),end=endpoint(e.start_degrees+e.sweep_degrees),
                large=e.sweep_degrees>180,sweep=not ((ref.frame=="cartesian")^ref.mirror_x))
        entities.append(record)
    return {"id":ref.id,"sha256":ref.sha256,"file":ref.file,"units":ref.units,"frame":ref.frame,
            "position":[ref.position.x_nm,ref.position.y_nm],"rotation":str(ref.rotation_degrees),
            "mirror_x":ref.mirror_x,"side":ref.side,"purpose":ref.purpose,"read_only":True,"entities":entities}
