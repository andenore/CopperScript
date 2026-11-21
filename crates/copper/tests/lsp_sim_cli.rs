//! `copper lsp` runs a bench from the editor: the "run this bench" code lens
//! of a bench file and its `workspace/executeCommand`, which runs
//! `copper sim BOARD --bench NAME` and reports the verdict as a message.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

fn uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') { format!("file://{text}") } else { format!("file:///{text}") }
}

fn json(text: &str) -> String {
    copper_json::encode_str(text, true)
}

/// The JSON bodies the server writes, one per message.
fn messages(stdout: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    return;
                }
                let line = line.trim();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0u8; length];
            if reader.read_exact(&mut body).is_err() {
                return;
            }
            if send.send(String::from_utf8_lossy(&body).into_owned()).is_err() {
                return;
            }
        }
    });
    receive
}

fn wait_for(receive: &mpsc::Receiver<String>, needle: &str) -> String {
    loop {
        let message = receive.recv_timeout(Duration::from_secs(120)).expect("a message from the server");
        if message.contains(needle) {
            return message;
        }
    }
}

#[test]
fn the_run_bench_lens_runs_copper_sim_and_reports_the_verdict() {
    // A copy of the RC example, so that its build/sim lands in a scratch directory.
    let directory = std::env::temp_dir().join(format!("copper-lsp-sim-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("benches")).unwrap();
    let examples = workspace().join("examples/simulation");
    std::fs::copy(examples.join("rc_filter.copper"), directory.join("rc_filter.copper")).unwrap();
    std::fs::copy(examples.join("benches/rc_filter_ac.copper"), directory.join("benches/rc_filter_ac.copper")).unwrap();
    let bench = directory.join("benches/rc_filter_ac.copper");

    let mut server = Command::new(env!("CARGO_BIN_EXE_copper"))
        .arg("lsp")
        .current_dir(&directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("copper lsp starts");
    let receive = messages(server.stdout.take().unwrap());
    let mut input = server.stdin.take().unwrap();
    let mut send = |body: String| input.write_all(&frame(&body)).and_then(|()| input.flush()).unwrap();

    send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#.to_owned());
    let initialized = wait_for(&receive, "\"id\":1");
    assert!(initialized.contains("codeLensProvider") && initialized.contains("copper.sim.runBench"), "{initialized}");
    send(r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#.to_owned());

    send(format!(
        r#"{{"jsonrpc":"2.0","id":2,"method":"textDocument/codeLens","params":{{"textDocument":{{"uri":{}}}}}}}"#,
        json(&uri(&bench))
    ));
    let lens = wait_for(&receive, "\"id\":2");
    assert!(lens.contains("Run bench RcCorner") && lens.contains("rc_filter.copper"), "{lens}");

    let board = directory.join("rc_filter.copper");
    send(format!(
        r#"{{"jsonrpc":"2.0","id":3,"method":"workspace/executeCommand","params":{{"command":"copper.sim.runBench","arguments":[{},"RcCorner"]}}}}"#,
        json(&board.to_string_lossy())
    ));
    let message = wait_for(&receive, "window/showMessage");
    assert!(message.contains("copper sim: pass") && message.contains("PASS: 2 of 2"), "{message}");
    assert!(directory.join("build/sim/RcCorner.html").is_file(), "the page beside the board");

    send(r#"{"jsonrpc":"2.0","id":4,"method":"shutdown","params":{}}"#.to_owned());
    wait_for(&receive, "\"id\":4");
    send(r#"{"jsonrpc":"2.0","method":"exit","params":{}}"#.to_owned());
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&directory);
}
