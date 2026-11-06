//! `edit-mechanical` answers request methods it does not serve instead of
//! stopping: any web page can send a `HEAD` or `OPTIONS` to the loopback port.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

struct EditorChild(std::process::Child);

impl Drop for EditorChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A board of built-in parts in a module of its own, so nothing is fetched.
fn project() -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("editor-methods");
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).expect("a scratch directory");
    std::fs::write(directory.join("copper.mod"), "module example.com/editor-methods\n").unwrap();
    std::fs::write(
        directory.join("board.copper"),
        "board B {\n    use library \"tiny\";\n    component R1: RESISTOR { footprint = \"0402\"; }\n    \
         component R2: RESISTOR { footprint = \"0402\"; }\n    net N { R1.1; R2.1; }\n}\n",
    )
    .unwrap();
    directory
}

fn status_line(port: u16, request: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("the editor listens");
    stream.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    String::from_utf8_lossy(&response).lines().next().unwrap_or_default().to_owned()
}

#[test]
fn unsupported_methods_get_501_and_the_editor_keeps_serving() {
    let mut child = EditorChild(
        Command::new(env!("CARGO_BIN_EXE_copper"))
            .args(["edit-mechanical", "board.copper", "--allow-proxy-footprints", "--offline", "--no-browser"])
            .current_dir(project())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the copper binary runs"),
    );
    let mut port = None;
    for line in BufReader::new(child.0.stdout.take().unwrap()).lines() {
        let line = line.expect("editor output");
        if let Some(url) = line.split("http://127.0.0.1:").nth(1) {
            port = url.split('/').next().and_then(|text| text.parse::<u16>().ok());
            break;
        }
    }
    let Some(port) = port else {
        panic!("the editor printed no URL");
    };
    let host = format!("Host: 127.0.0.1:{port}\r\n");
    let head = status_line(port, &format!("HEAD / HTTP/1.0\r\n{host}\r\n"));
    let options = status_line(port, &format!("OPTIONS /api/scene HTTP/1.0\r\n{host}\r\n"));
    let foreign = status_line(port, "OPTIONS / HTTP/1.0\r\nHost: example.com\r\n\r\n");
    let get = status_line(port, &format!("GET / HTTP/1.0\r\n{host}\r\n"));
    let exited = child.0.try_wait().expect("child status");
    assert_eq!(head, "HTTP/1.0 501 Not Implemented");
    assert_eq!(options, "HTTP/1.0 501 Not Implemented");
    assert_eq!(foreign, "HTTP/1.0 403 Forbidden");
    assert!(get.starts_with("HTTP/1.0 "), "the editor stopped answering: {get:?}");
    assert!(exited.is_none(), "the editor exited: {exited:?}");
}
