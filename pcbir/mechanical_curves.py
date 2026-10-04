"""Authoritative closed line/arc paths, with bounded INSCRIBED query geometry.

The first curved path implementation accepts convex paths with <=180-degree
arcs. Concave curved paths fail closed; polygon outlines remain unrestricted.
No sampled vertices are substituted for manufacturing primitives.
"""
from decimal import Decimal,localcontext
from fractions import Fraction
from functools import lru_cache
from math import atan2,acos,ceil,cos,sin,pi,sqrt

from .physical import BoundaryArc,BoundaryLine,BoardBoundaryPath,Point


def _cross(a,b,c):return (b.x_nm-a.x_nm)*(c.y_nm-b.y_nm)-(b.y_nm-a.y_nm)*(c.x_nm-b.x_nm)


def arc_circle(arc):
    """Exact rational circumcentre and squared radius of three integer points."""
    a,m,b=arc.start,arc.mid,arc.end
    determinant=2*(a.x_nm*(m.y_nm-b.y_nm)+m.x_nm*(b.y_nm-a.y_nm)+b.x_nm*(a.y_nm-m.y_nm))
    if not determinant:raise ValueError('boundary arc points cannot be collinear')
    qa=a.x_nm*a.x_nm+a.y_nm*a.y_nm;qm=m.x_nm*m.x_nm+m.y_nm*m.y_nm;qb=b.x_nm*b.x_nm+b.y_nm*b.y_nm
    cx=Fraction(qa*(m.y_nm-b.y_nm)+qm*(b.y_nm-a.y_nm)+qb*(a.y_nm-m.y_nm),determinant)
    cy=Fraction(qa*(b.x_nm-m.x_nm)+qm*(a.x_nm-b.x_nm)+qb*(m.x_nm-a.x_nm),determinant)
    return cx,cy,(Fraction(a.x_nm)-cx)**2+(Fraction(a.y_nm)-cy)**2


def arc_angles(arc):
    cx,cy,r2=arc_circle(arc)
    angles=[atan2(float(p.y_nm-cy),float(p.x_nm-cx)) for p in (arc.start,arc.mid,arc.end)]
    sweep=(angles[2]-angles[0])%(2*pi)
    if (angles[1]-angles[0])%(2*pi)>sweep:sweep-=2*pi
    if abs(sweep)>pi+1e-12:raise ValueError('curved boundary arcs must span at most 180 degrees')
    return cx,cy,r2,angles[0],sweep


def _distance_squared(cx,cy,a,b):
    dx,dy=b.x_nm-a.x_nm,b.y_nm-a.y_nm
    t=max(Fraction(0),min(Fraction(1),((cx-a.x_nm)*dx+(cy-a.y_nm)*dy)/(dx*dx+dy*dy)))
    return (cx-a.x_nm-t*dx)**2+(cy-a.y_nm-t*dy)**2


@lru_cache(maxsize=128)
def path_query_ring(path):
    from .mechanical import validated_ring
    vertices=[];turns=[]
    for segment in path.segments:
        if isinstance(segment,BoundaryLine):vertices.append(segment.start);continue
        cx,cy,r2,angle,sweep=arc_angles(segment)
        radius=sqrt(float(r2));error=path.maximum_chord_error_nm
        if not error<radius:raise ValueError('curve chord error must be smaller than its radius')
        step=2*acos(1-min(.5,(error-2)/(2*radius)))
        count=max(2,ceil(abs(sweep)/step)) if step else 4097
        if count+len(vertices)>4096:raise ValueError('curve query exceeds 4096 vertices; increase chord error')
        ring=[segment.start]
        for i in range(1,count):
            x=float(cx)+(radius-1)*cos(angle+sweep*i/count)
            y=float(cy)+(radius-1)*sin(angle+sweep*i/count)
            p=Point(round(x),round(y))
            if (p.x_nm-cx)**2+(p.y_nm-cy)**2>r2:
                raise ValueError('curve query quantization cannot prove inscribed geometry')
            ring.append(p)
        ring.append(segment.end)
        with localcontext() as context:
            context.prec=60
            r=(Decimal(r2.numerator)/Decimal(r2.denominator)).sqrt()
            limit=(r-error)**2
            for a,b in zip(ring,ring[1:]):
                distance=_distance_squared(cx,cy,a,b)
                if Decimal(distance.numerator)/Decimal(distance.denominator)<limit:
                    raise ValueError('curve query exceeds its declared chord-error bound')
        vertices.extend(ring[:-1]);turns.append(1 if sweep>0 else -1)
    vertices=validated_ring(vertices)
    winding=1 if sum(a.x_nm*b.y_nm-b.x_nm*a.y_nm for a,b in zip(vertices,(*vertices[1:],vertices[0])))>0 else -1
    if turns and (any(t!=winding for t in turns) or any(winding*_cross(vertices[i-1],p,vertices[(i+1)%len(vertices)])<0 for i,p in enumerate(vertices))):
        raise ValueError('curved paths currently require convex topology; concave curved queries are unsupported')
    return vertices


def rounded_rectangle_path(origin,width,height,radius,error):
    if not 0<2*radius<min(width,height):raise ValueError('rounded rectangle radius must be positive and below half its dimensions')
    x,y=origin.x_nm,origin.y_nm;right,bottom=x+width,y+height
    p=[Point(x+radius,y),Point(right-radius,y),Point(right,y+radius),Point(right,bottom-radius),
       Point(right-radius,bottom),Point(x+radius,bottom),Point(x,bottom-radius),Point(x,y+radius)]
    q=round(radius/sqrt(2))
    mids=[Point(right-radius+q,y+radius-q),Point(right-radius+q,bottom-radius+q),
          Point(x+radius-q,bottom-radius+q),Point(x+radius-q,y+radius-q)]
    names=['top','top_right','right','bottom_right','bottom','bottom_left','left','top_left']
    segments=tuple(BoundaryLine('outline/'+names[i],p[i],p[(i+1)%8]) if i%2==0 else
        BoundaryArc('outline/'+names[i],p[i],mids[i//2],p[(i+1)%8]) for i in range(8))
    return BoardBoundaryPath(segments,error)
