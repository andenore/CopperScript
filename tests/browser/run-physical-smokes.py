"""Disposable real-browser smoke for all physical-editor document kinds."""
import os
from pathlib import Path
from queue import Queue
import subprocess
import sys
from threading import Thread

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tests'))
from test_editor_transactions import SOURCE
from test_macro_definitions import definition_project


def run(kind):
    import tempfile
    destination = ROOT / 'build' / 'physical-editor-smokes'
    destination.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix=kind+'-', dir=destination))
    (directory / 'copper.mod').write_text('module example/physical-editor-smoke\n', encoding='utf-8')
    source = directory / 'board.copper'
    if kind == 'macro':
        _, source, _, _ = definition_project(directory)
    elif kind == 'board':
        text = SOURCE.replace('mechanical {', '''silkscreen { text LABEL { text = "Original"; position = (5 mm, 20 mm); } }
mechanical {
stackup {
 copper F.Cu { thickness = 0.035 mm; }
 dielectric CORE { thickness = 1.53 mm; er = 4.1; }
 copper B.Cu { thickness = 0.035 mm; }
}''')
        source.write_text(text, encoding='utf-8')
    else:
        source.write_text('board_profile Fragment { hole H1 { position = (5 mm, 5 mm); diameter = 2 mm; } }', encoding='utf-8')
    hidden = {'creationflags': subprocess.CREATE_NO_WINDOW} if os.name == 'nt' else {}
    server = subprocess.Popen([sys.executable,'-u','-m','copperscript','edit',str(source),
        '--no-browser','--offline','--allow-proxy-footprints'],cwd=ROOT,stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,text=True,**hidden)
    lines=Queue()
    def collect():
        for line in server.stdout: lines.put(line)
        lines.put(None)
    Thread(target=collect,daemon=True).start()
    try:
        while True:
            line=lines.get(timeout=60)
            if line is None: raise RuntimeError(server.stderr.read())
            if 'http://127.0.0.1:' in line:
                url=line[line.index('http://127.0.0.1:'):].strip();break
        result = subprocess.run(['node',str(ROOT/'tests/browser/physical-editor.cjs'),url,str(source),kind,
            str(destination/(kind+'.png'))],cwd=ROOT,capture_output=True,text=True,timeout=90,**hidden)
        print(result.stdout, flush=True)
        print(result.stderr, file=sys.stderr, flush=True)
        result.check_returncode()
    finally:
        server.terminate();server.wait(timeout=10)


if __name__ == '__main__':
    for kind in sys.argv[1:] or ['board','macro','profile']: run(kind)
