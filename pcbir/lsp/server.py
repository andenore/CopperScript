"""LSP 3.17 stdio server. Protocol bytes alone go to stdout.

Transport is bounded JSON-RPC with Content-Length framing. Debounced semantic
diagnostics run in one worker; only current workspace snapshots may publish.
"""
import json
import sys
from concurrent.futures import ThreadPoolExecutor
from threading import Lock, Timer

from .analysis import Workspace

MAX_MESSAGE = 16 * 1024 * 1024
MAX_HEADER = 8192


class ProtocolError(ValueError):
    pass


def read_message(stream):
    headers, total = {}, 0
    while True:
        line = stream.readline(MAX_HEADER + 1)
        if not line and not headers:
            return None
        total += len(line)
        if not line or total > MAX_HEADER:
            raise ProtocolError('truncated or oversized LSP header')
        if line == b'\r\n':
            break
        try:
            name, value = line.decode('ascii').strip().split(':', 1)
        except (ValueError, UnicodeError) as exc:
            raise ProtocolError('invalid LSP header') from exc
        name = name.lower()
        if name in headers:
            raise ProtocolError('duplicate LSP header')
        headers[name] = value.strip()
    try:
        length = int(headers['content-length'])
    except (KeyError, ValueError) as exc:
        raise ProtocolError('Content-Length is required') from exc
    if not 0 < length <= MAX_MESSAGE:
        raise ProtocolError('message exceeds bounds')
    chunks, remaining = [], length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise ProtocolError('truncated LSP payload')
        chunks.append(chunk)
        remaining -= len(chunk)
    try:
        return json.loads(b''.join(chunks).decode('utf-8'))
    except (UnicodeError, ValueError) as exc:
        raise ProtocolError('invalid UTF-8 JSON payload') from exc


class Writer:
    def __init__(self, stream):
        self.stream = stream
        self.lock = Lock()

    def send(self, message):
        body = json.dumps(message, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
        if len(body) > MAX_MESSAGE:
            raise ProtocolError('response exceeds bounds')
        with self.lock:
            self.stream.write(f'Content-Length: {len(body)}\r\n\r\n'.encode('ascii') + body)
            self.stream.flush()

    def notify(self, method, params):
        self.send({'jsonrpc': '2.0', 'method': method, 'params': params})


class Server:
    def __init__(self, writer, *, debounce=0.15):
        self.writer = writer
        self.workspace = Workspace()
        self.debounce = debounce
        self.initialized = False
        self.shutting_down = False
        self.exited = False
        self.timer = None
        self.pending = None
        self.worker = ThreadPoolExecutor(max_workers=1, thread_name_prefix='copper-lsp')
        self.schedule_lock = Lock()

    def schedule(self):
        with self.schedule_lock:
            if self.shutting_down or self.exited:
                return
            if self.timer:
                self.timer.cancel()
            if self.pending:
                self.pending.cancel()
            self.timer = Timer(self.debounce, self.submit)
            self.timer.daemon = True
            self.timer.start()

    def submit(self):
        with self.schedule_lock:
            if not self.shutting_down and not self.exited:
                self.pending = self.worker.submit(self.diagnostics, self.workspace.snapshot())

    def diagnostics(self, snapshot):
        generation, documents = snapshot
        for path, (version, _) in documents.items():
            with self.workspace.lock:
                if self.workspace.generation != generation or self.shutting_down or self.exited:
                    return
            try:
                findings = self.workspace.analysis(path.as_uri(), snapshot).diagnostics()
            except Exception as exc:
                findings = [{'range': {'start': {'line': 0, 'character': 0}, 'end': {'line': 0, 'character': 0}},
                             'severity': 1, 'code': 'LSP_ANALYSIS', 'source': 'CopperScript',
                             'message': f'Analysis could not complete: {exc}'}]
            # Coordinate the generation check and publication with edits/close.
            with self.workspace.lock:
                if self.workspace.generation == generation and not self.shutting_down and not self.exited:
                    self.writer.notify('textDocument/publishDiagnostics',
                                       {'uri': path.as_uri(), 'version': version, 'diagnostics': findings})

    def handle(self, method, params):
        if method == 'initialize':
            if self.initialized:
                raise ValueError('already initialized')
            self.initialized = True
            return {'capabilities': {
                'positionEncoding': 'utf-16',
                'textDocumentSync': {'openClose': True, 'change': 2, 'save': {'includeText': False}},
                'completionProvider': {'triggerCharacters': ['.'], 'resolveProvider': False},
                'hoverProvider': True, 'definitionProvider': True, 'documentSymbolProvider': True,
            }, 'serverInfo': {'name': 'CopperScript', 'version': '0.1.0'}}
        if method == 'exit':
            self.exited = True
            return
        if not self.initialized:
            raise LookupError('server is not initialized')
        if method == 'shutdown':
            self.shutting_down = True
            return None
        if self.shutting_down:
            raise ValueError('server is shutting down')
        if method in {'initialized', '$/cancelRequest', '$/setTrace'}:
            return
        if method == 'textDocument/didOpen':
            doc = params['textDocument']
            if self.workspace.update(doc['uri'], doc['version'], doc['text']):
                self.schedule()
            return
        if method == 'textDocument/didChange':
            doc = params['textDocument']
            if self.workspace.update(doc['uri'], doc['version'], changes=params['contentChanges']):
                self.schedule()
            return
        if method == 'textDocument/didClose':
            uri = params['textDocument']['uri']
            self.workspace.close(uri)
            self.writer.notify('textDocument/publishDiagnostics', {'uri': uri, 'diagnostics': []})
            self.schedule()
            return
        if method in {'textDocument/didSave', 'workspace/didChangeWatchedFiles'}:
            self.workspace.invalidate()
            self.schedule()
            return
        if method in {'textDocument/completion', 'textDocument/hover',
                      'textDocument/definition', 'textDocument/documentSymbol'}:
            analysis = self.workspace.analysis(params['textDocument']['uri'])
            if method == 'textDocument/documentSymbol':
                return [s.outline() for s in analysis.root.symbols[1:]]
            return getattr(analysis, method.split('/')[-1])(params['position'])
        raise NotImplementedError(method)

    def dispatch(self, message):
        if not isinstance(message, dict) or message.get('jsonrpc') != '2.0' or not isinstance(message.get('method'), str):
            self.writer.send({'jsonrpc': '2.0', 'id': message.get('id') if isinstance(message, dict) else None,
                              'error': {'code': -32600, 'message': 'Invalid Request'}})
            return
        request = 'id' in message
        try:
            params = message.get('params')
            if params is None:
                params = {}
            if not isinstance(params, dict):
                raise ValueError('object parameters are required')
            result = self.handle(message['method'], params)
            if request:
                self.writer.send({'jsonrpc': '2.0', 'id': message['id'], 'result': result})
        except Exception as exc:
            code = (-32601 if isinstance(exc, NotImplementedError) else
                    -32002 if isinstance(exc, LookupError) and not isinstance(exc, KeyError) else
                    -32602 if isinstance(exc, (ValueError, KeyError, TypeError, OSError)) else -32603)
            if request:
                self.writer.send({'jsonrpc': '2.0', 'id': message['id'], 'error': {'code': code, 'message': str(exc)}})
            else:
                self.writer.notify('window/logMessage', {'type': 1, 'message': f'{message["method"]}: {exc}'})

    def close(self):
        with self.schedule_lock:
            self.exited = True
            if self.timer:
                self.timer.cancel()
        self.worker.shutdown(wait=True, cancel_futures=True)


def main():
    # Windows pipe framing must preserve literal CRLF and byte counts.
    if sys.platform == 'win32':
        import msvcrt
        import os
        msvcrt.setmode(sys.stdin.fileno(), os.O_BINARY)
        msvcrt.setmode(sys.stdout.fileno(), os.O_BINARY)
    server = Server(Writer(sys.stdout.buffer))
    try:
        while not server.exited:
            message = read_message(sys.stdin.buffer)
            if message is None:
                break
            server.dispatch(message)
        return 0 if server.shutting_down else 1
    except (ProtocolError, BrokenPipeError) as exc:
        print(f'CopperScript LSP: {exc}', file=sys.stderr)
        return 1
    finally:
        server.close()


if __name__ == '__main__':
    raise SystemExit(main())
