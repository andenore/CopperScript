//! Part 3D models named by an `https://` URL: validated in the language, read
//! only from the cache `copper models fetch` fills, and exported to a
//! local-only folder unless the part says the model may be redistributed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

const MODEL: &str = "ISO-10303-21;\nHEADER;\nENDSEC;\nDATA;\nENDSEC;\nEND-ISO-10303-21;\n";
const URL: &str = "https://models.example.com/parts/W.step?download=1";

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|byte| format!("{byte:02x}")).collect()
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent).expect("the workspace root").to_path_buf()
}

fn copper(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_copper")).args(arguments).current_dir(workspace()).output().expect("copper runs")
}

fn text(output: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

/// A one-part board whose part names its model with `fields`.
fn project(name: &str, fields: &str) -> PathBuf {
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("url-models").join(name);
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("parts")).unwrap();
    // Its own module, so the model cache is this project's (not the workspace's).
    std::fs::write(directory.join("copper.mod"), "module example.com/url-models\n").unwrap();
    std::fs::write(
        directory.join("W.kicad_mod"),
        "(footprint \"W\"\n  (version 20240108)\n  (generator \"CopperScript-test\")\n  (layer \"F.Cu\")\n  (attr smd)\n  (fp_rect (start -2 -1) (end 2 1) (stroke (width 0.05) (type default)) (fill none) (layer \"F.CrtYd\"))\n  (pad \"1\" smd rect (at -1 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n  (pad \"2\" smd rect (at 1 0) (size 1 1) (layers \"F.Cu\" \"F.Paste\" \"F.Mask\"))\n)\n",
    )
    .unwrap();
    std::fs::write(
        directory.join("parts/widget.copper"),
        format!(
            "part Widget {{\n{fields}\n    pin A {{ number = \"1\"; domains = \"analog\"; directions = \"passive\"; }}\n    pin B {{ number = \"2\"; domains = \"analog\"; directions = \"passive\"; }}\n}}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        directory.join("board.copper"),
        "board B {\n    import p \"./parts\";\n    component U1: p.Widget { footprint = \"W.kicad_mod\"; }\n    net N { U1.A; }\n    net M { U1.B; }\n    constraint fixed_placement(U1) { x = 20mm; y = 15mm; rotation = 0; }\n}\n",
    )
    .unwrap();
    directory
}

fn url_fields(extra: &str) -> String {
    format!(
        "    model = \"{URL}\";\n    model_sha256 = \"{}\";\n    model_license = \"Example vendor terms: no redistribution\";\n{extra}",
        sha256(MODEL)
    )
}

fn export(directory: &Path) -> (Output, PathBuf) {
    let out = directory.join("out");
    let _ = std::fs::remove_dir_all(&out);
    let board = directory.join("board.copper");
    let pcb = out.join("b.kicad_pcb");
    let output = copper(&[
        "export-kicad-pcb",
        board.to_str().unwrap(),
        "--width-mm",
        "50",
        "--height-mm",
        "30",
        "-o",
        pcb.to_str().unwrap(),
    ]);
    (output, out)
}

fn cache(directory: &Path) -> PathBuf {
    directory.join(".copper-cache/models")
}

#[test]
fn a_url_model_is_used_only_from_the_cache_and_exported_local_only() {
    let directory = project("local-only", &url_fields(""));
    let board = directory.join("board.copper");
    let board = board.to_str().unwrap();

    let listed = copper(&["models", "list", board]);
    assert!(listed.status.success(), "{}", text(&listed));
    assert!(
        text(&listed).contains(&format!("missing  {URL} [Example vendor terms: no redistribution; local only] for ")),
        "{}",
        text(&listed)
    );

    // Builds never download: an unfetched model falls back to the generated body.
    let (output, out) = export(&directory);
    assert!(output.status.success(), "{}", text(&output));
    assert!(text(&output).contains("is not fetched; run `copper models fetch`"), "{}", text(&output));
    assert!(!out.join("CopperScript-local.3dshapes").exists());

    // Offline, fetch reports what it could not do.
    let offline = copper(&["models", "fetch", "--offline", board]);
    assert_eq!(offline.status.code(), Some(1), "{}", text(&offline));
    assert!(text(&offline).contains("(offline: not fetched)"));

    // As if fetched: the declared bytes in the cache.
    std::fs::create_dir_all(cache(&directory).join(sha256(MODEL))).unwrap();
    std::fs::write(cache(&directory).join(sha256(MODEL)).join("W.step"), MODEL).unwrap();
    assert!(text(&copper(&["models", "list", board])).contains(&format!("cached   {URL}")));
    let (output, out) = export(&directory);
    assert!(output.status.success(), "{}", text(&output));
    let local = out.join("CopperScript-local.3dshapes");
    let copied: Vec<String> = std::fs::read_dir(&local)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with("_W.step"))
        .collect();
    assert_eq!(copied.len(), 1, "{copied:?}");
    assert_eq!(std::fs::read_to_string(local.join(&copied[0])).unwrap(), MODEL);
    assert!(std::fs::read_to_string(local.join("README.txt")).unwrap().contains("Do not commit"));
    let pcb = std::fs::read_to_string(out.join("b.kicad_pcb")).unwrap();
    assert!(pcb.contains(&format!("(model \"${{KIPRJMOD}}/CopperScript-local.3dshapes/{}\"", copied[0])), "{pcb}");
    // The shared folder holds only generated bodies.
    for entry in std::fs::read_dir(out.join("CopperScript.3dshapes")).unwrap() {
        assert!(entry.unwrap().file_name().to_string_lossy().ends_with(".wrl"));
    }

    // A changed cached file is refused, not used.
    std::fs::write(cache(&directory).join(sha256(MODEL)).join("W.step"), "tampered").unwrap();
    let (output, _) = export(&directory);
    assert!(!output.status.success());
    assert!(text(&output).contains("does not match model_sha256"), "{}", text(&output));
}

#[test]
fn a_redistributable_url_model_is_exported_with_the_project() {
    let directory = project("redistributable", &url_fields("    model_redistributable = true;\n"));
    std::fs::create_dir_all(cache(&directory).join(sha256(MODEL))).unwrap();
    std::fs::write(cache(&directory).join(sha256(MODEL)).join("W.step"), MODEL).unwrap();
    let (output, out) = export(&directory);
    assert!(output.status.success(), "{}", text(&output));
    assert!(!out.join("CopperScript-local.3dshapes").exists());
    let shared: Vec<String> = std::fs::read_dir(out.join("CopperScript.3dshapes"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with("_W.step"))
        .collect();
    assert_eq!(shared.len(), 1, "{shared:?}");
}

#[test]
fn url_model_fields_are_validated() {
    for (name, fields, expected) in [
        ("no-hash", format!("    model = \"{URL}\";"), "needs model_sha256"),
        ("short-hash", format!("    model = \"{URL}\";\n    model_sha256 = \"abc\";"), "needs model_sha256"),
        (
            "http",
            format!("    model = \"http://x.com/W.step\";\n    model_sha256 = \"{}\";", sha256(MODEL)),
            "must use https://",
        ),
        (
            "not-a-model",
            format!("    model = \"https://x.com/W.zip\";\n    model_sha256 = \"{}\";", sha256(MODEL)),
            "must name a .step",
        ),
        (
            "hash-without-url",
            format!("    model_sha256 = \"{}\";", sha256(MODEL)),
            "applies only to an https:// model URL",
        ),
        ("bad-flag", url_fields("    model_redistributable = \"yes\";\n"), "must be true or false"),
    ] {
        let directory = project(name, &fields);
        let output = copper(&["check", directory.join("board.copper").to_str().unwrap()]);
        assert!(!output.status.success(), "{name}: {}", text(&output));
        assert!(text(&output).contains("CMP210") && text(&output).contains(expected), "{name}: {}", text(&output));
    }
}
