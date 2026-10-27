//! LSP 3.17 stdio server (`pcbir.lsp.server`). Protocol bytes alone go to stdout.
//!
//! Transport is bounded JSON-RPC with Content-Length framing. Debounced
//! semantic diagnostics run on one worker thread; only current workspace
//! snapshots may publish.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use copper_json::{Dumps, Object, Value};

use crate::analysis::{Analysis, Workspace, analyse_path};
use crate::pyerr::{PyError, PyResult, item};
use crate::text::{PathKey, int, object, path_from_uri};

const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const MAX_HEADER: usize = 8192;
const DEBOUNCE: Duration = Duration::from_millis(150);

/// `ProtocolError`: the transport is unusable; the server stops.
#[derive(Debug, PartialEq)]
pub struct ProtocolError(pub String);

fn protocol(message: &str) -> ProtocolError {
    ProtocolError(message.to_owned())
}

/// `stream.readline(limit)`: up to and including `\n`, at most `limit` bytes.
fn read_line(stream: &mut dyn BufRead, limit: usize) -> Vec<u8> {
    let mut line = Vec::new();
    while line.len() < limit {
        let available = match stream.fill_buf() {
            Ok(available) if !available.is_empty() => available,
            _ => break,
        };
        let wanted = (limit - line.len()).min(available.len());
        let (taken, done) = match available[..wanted].iter().position(|byte| *byte == b'\n') {
            Some(newline) => (newline + 1, true),
            None => (wanted, false),
        };
        line.extend_from_slice(&available[..taken]);
        stream.consume(taken);
        if done {
            break;
        }
    }
    line
}

/// `int(text)` for an already stripped ASCII header value.
fn python_int(text: &str) -> Option<i128> {
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    let valid = !digits.is_empty()
        && !digits.starts_with('_')
        && !digits.ends_with('_')
        && !digits.contains("__")
        && digits.chars().all(|c| c.is_ascii_digit() || c == '_');
    if !valid {
        return None;
    }
    let value: String = digits.chars().filter(|c| *c != '_').collect();
    let value = value.trim_start_matches('0');
    let magnitude: i128 = if value.is_empty() {
        0
    } else if value.len() > 30 {
        i128::MAX
    } else {
        value.parse().ok()?
    };
    Some(if text.starts_with('-') { -magnitude } else { magnitude })
}

/// `str.strip()` of an ASCII-decoded header line.
fn strip_ascii(text: &str) -> &str {
    text.trim_matches(|c: char| matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}' | '\u{1c}'..='\u{1f}'))
}

/// `read_message`: the next JSON payload, `None` at a clean end of input.
pub fn read_message(stream: &mut dyn BufRead) -> Result<Option<Value>, ProtocolError> {
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut total = 0;
    loop {
        let line = read_line(stream, MAX_HEADER + 1);
        if line.is_empty() && headers.is_empty() {
            return Ok(None);
        }
        total += line.len();
        if line.is_empty() || total > MAX_HEADER {
            return Err(protocol("truncated or oversized LSP header"));
        }
        if line == b"\r\n" {
            break;
        }
        let invalid = || protocol("invalid LSP header");
        let text = std::str::from_utf8(&line).ok().filter(|text| text.is_ascii()).ok_or_else(invalid)?;
        let (name, value) = strip_ascii(text).split_once(':').ok_or_else(invalid)?;
        let name = name.to_ascii_lowercase();
        if headers.iter().any(|(known, _)| *known == name) {
            return Err(protocol("duplicate LSP header"));
        }
        headers.push((name, strip_ascii(value).to_owned()));
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| python_int(value))
        .ok_or_else(|| protocol("Content-Length is required"))?;
    if !(0 < length && length <= MAX_MESSAGE as i128) {
        return Err(protocol("message exceeds bounds"));
    }
    let mut payload = vec![0; length as usize];
    let mut filled = 0;
    while filled < payload.len() {
        match stream.read(&mut payload[filled..]) {
            Ok(0) | Err(_) => return Err(protocol("truncated LSP payload")),
            Ok(count) => filled += count,
        }
    }
    let text = String::from_utf8(payload).map_err(|_| protocol("invalid UTF-8 JSON payload"))?;
    copper_json::parse(&text).map(Some).map_err(|_| protocol("invalid UTF-8 JSON payload"))
}

/// `Writer`: one framed message at a time.
pub struct Writer {
    stream: Mutex<Box<dyn Write + Send>>,
    /// Set when a write fails; the server then stops like Python's `BrokenPipeError`.
    broken: AtomicBool,
}

impl Writer {
    pub fn new(stream: Box<dyn Write + Send>) -> Writer {
        Writer { stream: Mutex::new(stream), broken: AtomicBool::new(false) }
    }

    pub fn send(&self, message: &Value) -> PyResult<()> {
        let body = Dumps::compact().ensure_ascii(false).dumps(message);
        if body.len() > MAX_MESSAGE {
            return Err(PyError::Value("response exceeds bounds".to_owned()));
        }
        let mut frame = format!("Content-Length: {}\r\n\r\n", body.len()).into_bytes();
        frame.extend_from_slice(body.as_bytes());
        let mut stream = self.stream.lock().expect("writer");
        if stream.write_all(&frame).and_then(|()| stream.flush()).is_err() {
            self.broken.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn notify(&self, method: &str, params: Value) -> PyResult<()> {
        self.send(&object([("jsonrpc", Value::from("2.0")), ("method", Value::from(method)), ("params", params)]))
    }
}

/// The debounce timer (`Timer`/`submit`): a pending deadline or a stop request.
#[derive(Default)]
struct Timer {
    deadline: Option<Instant>,
    stopped: bool,
}

/// State shared by the request loop and the diagnostics worker.
struct Shared {
    writer: Writer,
    workspace: Mutex<Workspace>,
    timer: Mutex<Timer>,
    wake: Condvar,
    shutting_down: AtomicBool,
    exited: AtomicBool,
}

impl Shared {
    fn closing(&self) -> bool {
        self.shutting_down.load(Ordering::SeqCst) || self.exited.load(Ordering::SeqCst)
    }

    fn schedule(&self) {
        let mut timer = self.timer.lock().expect("timer");
        if self.closing() {
            return;
        }
        timer.deadline = Some(Instant::now() + DEBOUNCE);
        self.wake.notify_all();
    }

    /// The worker: wait for a deadline that is not pushed back, then publish.
    fn work(&self) {
        loop {
            {
                let mut timer = self.timer.lock().expect("timer");
                loop {
                    if timer.stopped {
                        return;
                    }
                    match timer.deadline {
                        None => timer = self.wake.wait(timer).expect("timer"),
                        Some(deadline) => {
                            let now = Instant::now();
                            if now >= deadline {
                                timer.deadline = None;
                                break;
                            }
                            timer = self.wake.wait_timeout(timer, deadline - now).expect("timer").0;
                        }
                    }
                }
                if self.closing() {
                    continue;
                }
            }
            self.diagnostics();
        }
    }

    fn diagnostics(&self) {
        let (generation, documents, sources, uris) = {
            let workspace = self.workspace.lock().expect("workspace");
            let (sources, uris) = workspace.sources();
            (workspace.generation, workspace.documents.clone(), sources, uris)
        };
        for document in documents.values() {
            if self.workspace.lock().expect("workspace").generation != generation || self.closing() {
                return;
            }
            let findings = analyse_path(&document.path, sources.clone(), uris.clone())
                .map_err(|failure| failure.message().to_owned())
                .and_then(|mut analysis| analysis.diagnostics());
            let findings = match findings {
                Ok(findings) => Value::Array(findings),
                Err(message) => {
                    let zero = object([("line", int(0)), ("character", int(0))]);
                    Value::Array(vec![object([
                        ("range", object([("start", zero.clone()), ("end", zero)])),
                        ("severity", int(1)),
                        ("code", Value::from("LSP_ANALYSIS")),
                        ("source", Value::from("CopperScript")),
                        ("message", Value::Str(format!("Analysis could not complete: {message}"))),
                    ])])
                }
            };
            // Coordinate the generation check and publication with edits/close.
            let workspace = self.workspace.lock().expect("workspace");
            if workspace.generation == generation && !self.closing() {
                let params = object([
                    ("uri", Value::Str(workspace.uri_for(&document.path))),
                    ("version", Value::Int(document.version.clone())),
                    ("diagnostics", findings),
                ]);
                let _ = self.writer.notify("textDocument/publishDiagnostics", params);
            }
        }
    }
}

/// The request loop's own state and the worker it owns.
pub struct Server {
    shared: Arc<Shared>,
    initialized: bool,
    worker: Option<JoinHandle<()>>,
    /// The requests' analysis of one document at one generation (`Workspace.cache`).
    cache: Option<(u64, PathKey, Analysis)>,
}

fn capabilities() -> Value {
    object([
        (
            "capabilities",
            object([
                ("positionEncoding", Value::from("utf-16")),
                (
                    "textDocumentSync",
                    object([
                        ("openClose", Value::Bool(true)),
                        ("change", int(2)),
                        ("save", object([("includeText", Value::Bool(false))])),
                    ]),
                ),
                (
                    "completionProvider",
                    object([
                        ("triggerCharacters", Value::Array(vec![Value::from(".")])),
                        ("resolveProvider", Value::Bool(false)),
                    ]),
                ),
                ("hoverProvider", Value::Bool(true)),
                ("definitionProvider", Value::Bool(true)),
                ("documentSymbolProvider", Value::Bool(true)),
                ("codeActionProvider", object([("codeActionKinds", Value::Array(vec![Value::from("quickfix")]))])),
            ]),
        ),
        ("serverInfo", object([("name", Value::from("CopperScript")), ("version", Value::from("0.1.0"))])),
    ])
}

impl Server {
    pub fn new(writer: Writer) -> Server {
        let shared = Arc::new(Shared {
            writer,
            workspace: Mutex::new(Workspace::default()),
            timer: Mutex::new(Timer::default()),
            wake: Condvar::new(),
            shutting_down: AtomicBool::new(false),
            exited: AtomicBool::new(false),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            std::thread::Builder::new()
                .name("copper-lsp".to_owned())
                .spawn(move || shared.work())
                .expect("spawn the diagnostics worker")
        };
        Server { shared, initialized: false, worker: Some(worker), cache: None }
    }

    pub fn exited(&self) -> bool {
        self.shared.exited.load(Ordering::SeqCst)
    }

    pub fn shutting_down(&self) -> bool {
        self.shared.shutting_down.load(Ordering::SeqCst)
    }

    pub fn broken(&self) -> bool {
        self.shared.writer.broken.load(Ordering::SeqCst)
    }

    fn snapshot_analysis(&mut self, uri: &Value) -> PyResult<&Analysis> {
        let path = path_from_uri(uri)?;
        let key = PathKey::of(&path);
        let (generation, (sources, uris)) = {
            let workspace = self.shared.workspace.lock().expect("workspace");
            (workspace.generation, workspace.sources())
        };
        match &mut self.cache {
            // Client URI spellings may change without a new generation; replies use the current ones.
            Some((cached, known, analysis)) if *cached == generation && *known == key => analysis.uris = uris,
            cache => *cache = Some((generation, key, analyse_path(&path, sources, uris)?)),
        }
        Ok(&self.cache.as_ref().expect("cached above").2)
    }

    fn handle(&mut self, method: &str, params: &Value) -> PyResult<Value> {
        if method == "initialize" {
            if self.initialized {
                return Err(PyError::Value("already initialized".to_owned()));
            }
            self.initialized = true;
            return Ok(capabilities());
        }
        if method == "exit" {
            self.shared.exited.store(true, Ordering::SeqCst);
            return Ok(Value::Null);
        }
        if !self.initialized {
            return Err(PyError::Lookup("server is not initialized".to_owned()));
        }
        if method == "shutdown" {
            self.shared.shutting_down.store(true, Ordering::SeqCst);
            return Ok(Value::Null);
        }
        if self.shutting_down() {
            return Err(PyError::Value("server is shutting down".to_owned()));
        }
        match method {
            "initialized" | "$/cancelRequest" | "$/setTrace" | "workspace/didChangeConfiguration" => Ok(Value::Null),
            "textDocument/didOpen" => {
                let document = item(params, "textDocument")?;
                let (uri, version, text) =
                    (item(document, "uri")?, item(document, "version")?, item(document, "text")?);
                if self.shared.workspace.lock().expect("workspace").update(uri, version, text, None)? {
                    self.shared.schedule();
                }
                Ok(Value::Null)
            }
            "textDocument/didChange" => {
                let document = item(params, "textDocument")?;
                let (uri, version) = (item(document, "uri")?, item(document, "version")?);
                let changes = item(params, "contentChanges")?;
                // `changes=None` (JSON null) falls through to the text check.
                let changes = (!matches!(changes, Value::Null)).then_some(changes);
                if self.shared.workspace.lock().expect("workspace").update(uri, version, &Value::Null, changes)? {
                    self.shared.schedule();
                }
                Ok(Value::Null)
            }
            "textDocument/didClose" => {
                let uri = item(item(params, "textDocument")?, "uri")?;
                self.shared.workspace.lock().expect("workspace").close(uri)?;
                self.shared.writer.notify(
                    "textDocument/publishDiagnostics",
                    object([("uri", uri.clone()), ("diagnostics", Value::Array(Vec::new()))]),
                )?;
                self.shared.schedule();
                Ok(Value::Null)
            }
            "textDocument/didSave" | "workspace/didChangeWatchedFiles" => {
                self.shared.workspace.lock().expect("workspace").invalidate();
                self.shared.schedule();
                Ok(Value::Null)
            }
            "textDocument/codeAction" => {
                let analysis = self.snapshot_analysis(item(item(params, "textDocument")?, "uri")?)?;
                analysis.code_actions(params)
            }
            "textDocument/completion"
            | "textDocument/hover"
            | "textDocument/definition"
            | "textDocument/documentSymbol" => {
                let analysis = self.snapshot_analysis(item(item(params, "textDocument")?, "uri")?)?;
                match method {
                    "textDocument/documentSymbol" => Ok(analysis.document_symbols()),
                    "textDocument/completion" => analysis.completion(item(params, "position")?),
                    "textDocument/hover" => analysis.hover(item(params, "position")?),
                    _ => analysis.definition(item(params, "position")?),
                }
            }
            other => Err(PyError::NotImplemented(other.to_owned())),
        }
    }

    fn response(id: &Value, key: &str, value: Value) -> Value {
        let mut message = Object::new();
        message.insert("jsonrpc".to_owned(), Value::from("2.0"));
        message.insert("id".to_owned(), id.clone());
        message.insert(key.to_owned(), value);
        Value::Object(message)
    }

    pub fn dispatch(&mut self, message: &Value) {
        let method = match message {
            Value::Object(fields)
                if fields.get("jsonrpc").is_some_and(|value| *value == Value::from("2.0"))
                    && matches!(fields.get("method"), Some(Value::Str(_))) =>
            {
                fields["method"].as_str().expect("checked").to_owned()
            }
            _ => {
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                let error =
                    object([("code", Value::Int((-32600).into())), ("message", Value::from("Invalid Request"))]);
                let _ = self.shared.writer.send(&Self::response(&id, "error", error));
                return;
            }
        };
        let id = message.get("id");
        let outcome = (|| -> PyResult<()> {
            let params = match message.get("params") {
                None | Some(Value::Null) => Value::Object(Object::new()),
                Some(params @ Value::Object(_)) => params.clone(),
                Some(_) => return Err(PyError::Value("object parameters are required".to_owned())),
            };
            let result = self.handle(&method, &params)?;
            if let Some(id) = id {
                self.shared.writer.send(&Self::response(id, "result", result))?;
            }
            Ok(())
        })();
        let Err(failure) = outcome else { return };
        if let Some(id) = id {
            let error =
                object([("code", Value::Int(failure.code().into())), ("message", Value::from(failure.message()))]);
            let _ = self.shared.writer.send(&Self::response(id, "error", error));
        } else if !matches!(failure, PyError::NotImplemented(_)) {
            let params =
                object([("type", int(1)), ("message", Value::Str(format!("{method}: {}", failure.message())))]);
            let _ = self.shared.writer.notify("window/logMessage", params);
        }
        // Unknown notifications are ignored, as LSP requires.
    }

    pub fn close(&mut self) {
        {
            let mut timer = self.shared.timer.lock().expect("timer");
            self.shared.exited.store(true, Ordering::SeqCst);
            timer.stopped = true;
            timer.deadline = None;
            self.shared.wake.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// `main()`: serve until `exit` or end of input. Exit status 0 only after `shutdown`.
pub fn serve(input: &mut dyn BufRead, output: Box<dyn Write + Send>, errors: &mut dyn Write) -> u8 {
    let mut server = Server::new(Writer::new(output));
    let status = loop {
        if server.exited() {
            break if server.shutting_down() { 0 } else { 1 };
        }
        let message = match read_message(input) {
            Ok(Some(message)) => message,
            Ok(None) => break if server.shutting_down() { 0 } else { 1 },
            Err(ProtocolError(message)) => {
                let _ = writeln!(errors, "CopperScript LSP: {message}");
                break 1;
            }
        };
        server.dispatch(&message);
        if server.broken() {
            let _ = writeln!(errors, "CopperScript LSP: [Errno 32] Broken pipe");
            break 1;
        }
    };
    server.close();
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(raw: &[u8]) -> Result<Option<Value>, ProtocolError> {
        read_message(&mut std::io::BufReader::new(raw))
    }

    #[test]
    fn malformed_transport_is_rejected_like_python() {
        assert_eq!(read(b"Content-Length: -1\r\n\r\n"), Err(protocol("message exceeds bounds")));
        assert_eq!(read(b"Content-Length: 999999999\r\n\r\n"), Err(protocol("message exceeds bounds")));
        assert_eq!(read(b"Content-Length: 5\r\n\r\n{}"), Err(protocol("truncated LSP payload")));
        assert_eq!(read(b"Content-Length: 1\r\nContent-Length: 1\r\n\r\n0"), Err(protocol("duplicate LSP header")));
        assert_eq!(read(b"Other: yes\r\n\r\n"), Err(protocol("Content-Length is required")));
        assert_eq!(read(b"Content-Length: 1\r\n\r\nx"), Err(protocol("invalid UTF-8 JSON payload")));
        assert_eq!(read(b"no colon\r\n\r\n"), Err(protocol("invalid LSP header")));
        assert_eq!(read(b""), Ok(None));
        assert_eq!(
            read(b"Content-Length: 1_1\r\n\r\n[1,2,3,4,5]").unwrap(),
            Some(copper_json::parse("[1,2,3,4,5]").unwrap())
        );
    }
}
