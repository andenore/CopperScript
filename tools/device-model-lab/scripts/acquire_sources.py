"""Record source metadata; downloads are opt-in and never executed."""
import hashlib, json, sys
from pathlib import Path

def record(path: str, source_id: str):
    p=Path(path); digest=hashlib.sha256(p.read_bytes()).hexdigest()
    out=Path("cache")/"sha256.json"; out.parent.mkdir(exist_ok=True)
    old=json.loads(out.read_text()) if out.exists() else {}
    old[source_id]={"path":str(p),"sha256":digest}
    out.write_text(json.dumps(old, indent=2)+"\n")
    print(digest)

if __name__ == "__main__":
    if len(sys.argv) != 3: raise SystemExit("usage: python scripts/acquire_sources.py FILE SOURCE_ID")
    record(sys.argv[1], sys.argv[2])
