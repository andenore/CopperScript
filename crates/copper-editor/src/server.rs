//! `pcbir.editor.server`: the loopback-only HTTP transport.

use std::hash::{BuildHasher, RandomState};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use copper_json::{Dumps, Object, Value};

use crate::project::Session;
use crate::{Error, Result, object};

/// Unpredictable bytes for capability tokens and temporary names.
///
/// `RandomState` is keyed from the operating system's random source and
/// SipHash-1-3 is a keyed pseudorandom function, so hashing a counter gives
/// bytes an observer cannot predict without the per-process key; the
/// `attempt` number keeps retries distinct within one call.
pub fn random_bytes(count: usize, attempt: u32) -> Vec<u8> {
    let state = RandomState::new();
    let mut bytes = Vec::with_capacity(count + 8);
    let mut counter: u64 = u64::from(attempt) << 32;
    while bytes.len() < count {
        bytes.extend_from_slice(&state.hash_one(counter).to_le_bytes());
        counter += 1;
    }
    bytes.truncate(count);
    bytes
}

/// `secrets.token_hex(count)` style text: lower-case hexadecimal.
pub fn random_token(count: usize, attempt: u32) -> String {
    random_bytes(count, attempt).iter().map(|byte| format!("{byte:02x}")).collect()
}

/// `secrets.token_urlsafe(count)`: URL-safe base64 without padding.
pub fn token_urlsafe(count: usize) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let bytes = random_bytes(count, 0);
    let mut text = String::new();
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for index in 0..=chunk.len() {
            text.push(ALPHABET[((value >> (18 - 6 * index)) & 63) as usize] as char);
        }
    }
    text
}

/// The bundled browser client (`assets/`).
const ASSETS: [(&str, &[u8], &str); 4] = [
    ("/", include_bytes!("../assets/index.html"), "text/html; charset=utf-8"),
    ("/editor.js", include_bytes!("../assets/editor.js"), "text/javascript; charset=utf-8"),
    ("/editor.css", include_bytes!("../assets/editor.css"), "text/css; charset=utf-8"),
    ("/copper-logo.svg", include_bytes!("../assets/copper-logo.svg"), "image/svg+xml"),
];

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self'; img-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'";

/// One parsed request.
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn all(&self, name: &str) -> Vec<&str> {
        self.headers.iter().filter(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str()).collect()
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.all(name).into_iter().next()
    }
}

/// What the transport answers, or that this implementation cannot.
enum Outcome {
    Respond(u16, Vec<u8>, &'static str),
    NotImplemented(String),
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        _ => "Error",
    }
}

fn json_body(value: &Value) -> Vec<u8> {
    Dumps::new().dumps(value).into_bytes()
}

fn error_body(status: u16, message: &str) -> Outcome {
    Outcome::Respond(status, json_body(&object(vec![("error", Value::from(message))])), "application/json")
}

/// The loopback editor: its listener, URL and capability.
pub struct EditorServer {
    listener: TcpListener,
    pub port: u16,
    pub token: String,
    session: Arc<Mutex<Session>>,
    /// The definition editors opened from this one, by source.
    definitions: Arc<Mutex<indexmap::IndexMap<std::path::PathBuf, String>>>,
}

impl EditorServer {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/#token={}", self.port, self.token)
    }
}

/// `create_server`.
pub fn create_server(session: Session, port: &num_bigint::BigInt) -> Result<EditorServer> {
    let Some(port) = u16::try_from(port).ok() else {
        return Err(Error::Value("editor port must be between 0 and 65535".to_owned()));
    };
    let token = token_urlsafe(32);
    let listener = TcpListener::bind(("127.0.0.1", port))
        .map_err(|error| Error::Os(copper_lang::paths::python_os_error(&error, None)))?;
    let port =
        listener.local_addr().map_err(|error| Error::Os(copper_lang::paths::python_os_error(&error, None)))?.port();
    Ok(EditorServer { listener, port, token, session: Arc::new(Mutex::new(session)), definitions: Arc::default() })
}

fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<(Request, Vec<u8>)>> {
    let mut data = Vec::new();
    let mut buffer = [0u8; 8192];
    let head_end = loop {
        if let Some(index) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            break index;
        }
        if data.len() > 65536 + 8192 {
            return Ok(None);
        }
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok(None);
        }
        data.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut words = lines.next().unwrap_or("").split(' ');
    let (method, path) = (words.next().unwrap_or("").to_owned(), words.next().unwrap_or("").to_owned());
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
        .collect();
    Ok(Some((Request { method, path, headers }, data[head_end + 4..].to_vec())))
}

fn read_body(stream: &mut TcpStream, mut body: Vec<u8>, length: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = [0u8; 8192];
    while body.len() < length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(length);
    Ok(body)
}

/// `allowed`: the exact loopback Host, a same-origin Origin and, for the API, the capability.
fn refused(request: &Request, port: u16, token: &str, api: bool) -> Option<Outcome> {
    let expected = format!("127.0.0.1:{port}");
    if request.all("Host") != [expected.as_str()] {
        return Some(error_body(403, "invalid Host"));
    }
    if let Some(origin) = request.get("Origin")
        && origin != format!("http://{expected}")
    {
        return Some(error_body(403, "cross-origin request rejected"));
    }
    if api && request.get("X-Copper-Token").unwrap_or("").as_bytes() != token.as_bytes() {
        return Some(error_body(403, "session capability required"));
    }
    None
}

fn outcome_of(result: Result<Object>) -> Outcome {
    match result {
        Ok(body) => Outcome::Respond(200, json_body(&Value::Object(body)), "application/json"),
        Err(Error::Stale(message)) => error_body(409, &message),
        Err(Error::NotImplemented(what)) => Outcome::NotImplemented(what),
        Err(Error::Uncaught(line)) => Outcome::NotImplemented(format!("an uncaught Python error: {line}")),
        Err(error) => error_body(400, &error.to_string()),
    }
}

/// `json.loads(body, object_pairs_hook=unique, parse_constant=reject)`.
fn parse_operation(body: &[u8]) -> Result<Object> {
    // `json.loads(bytes)` detects UTF-8 with a BOM and decodes it as `utf-8-sig`.
    let text = crate::overlay::decode_utf8(body)?;
    match copper_json::parse_strict(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(Error::Value("operation must be an object".to_owned())),
        Err(copper_json::StrictError::Rejected(copper_json::Rejection::DuplicateKey(_))) => {
            Err(Error::Value("duplicate JSON fields".to_owned()))
        }
        Err(copper_json::StrictError::Rejected(_)) => Err(Error::Value("nonfinite JSON value".to_owned())),
        Err(copper_json::StrictError::Syntax(error)) => Err(Error::Value(error.python_message(text))),
    }
}

/// `int(text)` for a `Content-Length` value.
fn python_int(text: &str) -> std::result::Result<i128, String> {
    let trimmed = text.trim_matches(copper_json::python_isspace);
    let digits = trimmed.strip_prefix(['+', '-']).unwrap_or(trimmed).replace('_', "");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) || trimmed.contains("__") {
        return Err(format!("invalid literal for int() with base 10: {}", copper_json::python_str_repr(text)));
    }
    let value: i128 = digits.parse().unwrap_or(i128::MAX);
    Ok(if trimmed.starts_with('-') { -value } else { value })
}

fn post(server: &Shared, request: &Request, stream: &mut TcpStream, initial: Vec<u8>) -> Outcome {
    let session = &server.session;
    let endpoint = request.path.split(['?', '#']).next().unwrap_or("");
    if endpoint != "/api/operation" && endpoint != "/api/open-definition" {
        return error_body(404, "not found");
    }
    let length = match python_int(request.get("Content-Length").unwrap_or("0")) {
        Ok(length) => length,
        Err(message) => return error_body(400, &message),
    };
    if !(0 < length && length <= 65536)
        || request.get("Content-Type") != Some("application/json")
        || request.get("Transfer-Encoding").is_some()
        || request.all("Content-Length").len() != 1
    {
        return error_body(400, "bounded application/json body required");
    }
    let body = match read_body(stream, initial, length as usize) {
        Ok(body) => body,
        Err(error) => return error_body(400, &copper_lang::paths::python_os_error(&error, None)),
    };
    let parsed = match parse_operation(&body) {
        Ok(parsed) => parsed,
        Err(error) => return outcome_of(Err(error)),
    };
    let mut session = session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if endpoint == "/api/open-definition" {
        let index = match (parsed.len(), parsed.get("feature_index")) {
            (1, Some(Value::Int(index))) => index.clone(),
            _ => return error_body(400, "definition selection requires a feature index"),
        };
        let features = match session.scene() {
            Ok(scene) => {
                scene.get("source_features").and_then(Value::as_array).map(<[Value]>::to_vec).unwrap_or_default()
            }
            Err(error) => return outcome_of(Err(error)),
        };
        let feature = usize::try_from(&index).ok().and_then(|i| features.get(i));
        let Some(path) =
            feature.and_then(|f| f.get("definition_path")).and_then(Value::as_str).filter(|p| !p.is_empty())
        else {
            return error_body(400, "selected feature has no source definition");
        };
        if !path.ends_with(".copper") {
            return error_body(400, "this asset has no editable CopperScript definition");
        }
        let source = copper_lang::paths::resolve(std::path::Path::new(path));
        let mut definitions = server.definitions.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if !definitions.contains_key(&source) {
            let opened = crate::dependencies::prepare_editor_dependencies(&source, true, true, None, &mut |_| {})
                .and_then(|()| {
                    crate::project::document_session(&source, &crate::project::BuildSettings::default(), None, None)
                })
                .and_then(|child| create_server(child, &0.into()));
            let child = match opened {
                Ok(child) => child,
                Err(error) => return outcome_of(Err(error)),
            };
            definitions.insert(source.clone(), child.url());
            let err_exit = server.err_exit;
            std::thread::spawn(move || serve(child, err_exit));
        }
        let url = definitions[&source].clone();
        return Outcome::Respond(200, json_body(&object(vec![("url", Value::from(url))])), "application/json");
    }
    outcome_of(session.operation(&parsed))
}

fn handle(server: &Shared, mut stream: TcpStream, port: u16, token: &str) {
    let (session, err_exit) = (&server.session, server.err_exit);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(Some((request, initial))) = read_request(&mut stream) else { return };
    let path = request.path.split(['?', '#']).next().unwrap_or("").to_owned();
    let outcome = match request.method.as_str() {
        "GET" => match refused(&request, port, token, path.starts_with("/api/")) {
            Some(refusal) => refusal,
            None if path == "/api/scene" => {
                let mut session = session.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                // A stale scene is a plain 400 here, not a revision conflict.
                outcome_of(session.scene().map_err(|error| match error {
                    Error::Stale(message) => Error::Value(message),
                    other => other,
                }))
            }
            None => match ASSETS.iter().find(|(name, _, _)| *name == path) {
                Some((_, bytes, mime)) => Outcome::Respond(200, bytes.to_vec(), mime),
                None => error_body(404, "not found"),
            },
        },
        "POST" => match refused(&request, port, token, true) {
            Some(refusal) => refusal,
            None => post(server, &request, &mut stream, initial),
        },
        other => Outcome::NotImplemented(format!("HTTP {other} requests to the editor")),
    };
    match outcome {
        Outcome::Respond(status, body, mime) => {
            let head = format!(
                "HTTP/1.0 {status} {}\r\nServer: BaseHTTP/0.6 Python/3\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: {CSP}\r\n\r\n",
                reason(status),
                body.len()
            );
            let _ = stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(&body));
        }
        Outcome::NotImplemented(what) => err_exit(&what),
    }
}

/// What every request handler of one server shares.
struct Shared {
    session: Arc<Mutex<Session>>,
    definitions: Arc<Mutex<indexmap::IndexMap<std::path::PathBuf, String>>>,
    err_exit: &'static (dyn Fn(&str) + Sync),
}

/// `serve`: answer requests until the process is stopped; `err_exit` ends
/// the process when a request reaches behaviour this implementation lacks.
pub fn serve(server: EditorServer, err_exit: &'static (dyn Fn(&str) + Sync)) {
    let shared = Shared { session: server.session.clone(), definitions: server.definitions.clone(), err_exit };
    std::thread::scope(|scope| {
        for stream in server.listener.incoming() {
            let Ok(stream) = stream else { continue };
            let (shared, token, port) = (&shared, server.token.as_str(), server.port);
            scope.spawn(move || handle(shared, stream, port, token));
        }
    });
}
