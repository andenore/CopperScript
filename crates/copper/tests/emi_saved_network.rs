//! Opt-in numerical regression of a hash-bound saved report/physical state.
//! Replays passive geometry only; does not validate a changed source checkout.
use copper_drc::emi::EmiFinding;
use copper_drc::{Finding, Severity};
use copper_fullwave::hotloop::HotLoopInput;
use copper_json::Value;
use sha2::{Digest, Sha256};

fn bound_text(name: &str) -> String {
    let path = std::env::var(name).expect("explicit saved artifact path");
    let bytes = std::fs::read(&path).unwrap();
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(digest, std::env::var(format!("{name}_SHA256")).expect("explicit artifact digest"));
    println!("{name}: {path}, sha256 {digest}");
    String::from_utf8(bytes).unwrap()
}

#[test]
#[ignore = "explicit hash-bound saved network; expensive numerical regression"]
fn saved_external_network() {
    let state = copper_state::StateDocument::read(&bound_text("COPPER_EMI_STATE")).unwrap();
    let board = copper_route::state_codec::physical_board(&state.value).unwrap();
    let report = copper_json::parse(&bound_text("COPPER_EMI_REPORT")).unwrap();
    assert_eq!(report.get("schema").and_then(Value::as_str), Some(copper_drc::emi::SCHEMA));
    let controller = std::env::var("COPPER_EMI_CONTROLLER").expect("explicit controller");
    let mut inputs: Vec<_> = report
        .get("findings")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .filter(|f| {
            f.get("code").and_then(Value::as_str) == Some("EMI-HOT-LOOP")
                && f.get("cell").and_then(Value::as_str) == Some(controller.as_str())
        })
        .map(|f| {
            let finding = EmiFinding {
                finding: Finding {
                    code: "EMI-HOT-LOOP".to_owned(),
                    severity: Severity::Warning,
                    message: "saved passive-network regression".to_owned(),
                    objects: vec![],
                    nets: vec![],
                    layers: vec![],
                    required_nm: None,
                    measured_nm: None,
                },
                cell: Some(controller.clone()),
                provenance: None,
                tags: vec![],
                estimate: None,
                margin_db: None,
                accepted: None,
                measures: f.get("measures").and_then(Value::as_object).unwrap().clone(),
                subjects: vec![],
                rank: 0.0,
                conducted_input: None,
            };
            HotLoopInput::from_finding(&finding, "buck_boost").unwrap()
        })
        .collect();
    assert_eq!(inputs.len(), 2);
    inputs.sort_by_key(|i| i.loop_kind != copper_drc::emi::loop_model::LoopKind::Input);
    let peer = inputs.pop().unwrap();
    let mut input = inputs.pop().unwrap();
    input.external_peer = peer.external_contour;
    input.polygon_mm.extend(peer.polygon_mm);
    let mut options = copper_extract::hotloop::HotLoopOptions::default();
    options.board.ideal_capacitors = true;
    options.board.margin_nm = 1_000_000;
    options.mesh.defect_cell_nm = 25_000;
    options.mesh.loop_cell_nm = 25_000;
    options.mesh.across = std::env::var("COPPER_EMI_WIDTH_CELLS").unwrap_or("4".to_owned()).parse().unwrap();
    options.mesh.max_filaments = std::env::var("COPPER_EMI_FILAMENTS").unwrap_or("20000".to_owned()).parse().unwrap();
    options.series = std::env::var("COPPER_EMI_SINGLE_MESH").is_err();
    if let Ok(frequencies) =
        std::env::var("COPPER_EMI_FREQUENCIES_HZ").or_else(|_| std::env::var("COPPER_EMI_FREQUENCY_HZ"))
    {
        options.frequencies_hz = frequencies.split(',').map(|f| f.trim().parse().unwrap()).collect();
    }
    println!(
        "native fill; budget {} filaments; series {}; frequencies {:?} Hz",
        options.mesh.max_filaments, options.series, options.frequencies_hz
    );
    let model = copper_extract::board::board_model(&board).unwrap();
    if let Ok(mode) = std::env::var("COPPER_EMI_FULLWAVE") {
        let options = copper_extract::fullwave::Options {
            margin_nm: 1_000_000,
            capacitor_height_nm: Some(
                std::env::var("COPPER_EMI_CAPACITOR_HEIGHT_NM").expect("explicit fixture height").parse().unwrap(),
            ),
            source_height_nm: std::env::var("COPPER_EMI_SOURCE_HEIGHT_NM").ok().map(|h| h.parse().unwrap()),
            budget_seconds: std::env::var("COPPER_EMI_SECONDS").unwrap_or("300".to_owned()).parse().unwrap(),
            threads: 2,
            ..Default::default()
        };
        let (region, patches) = copper_extract::fullwave::region(&model, &board, &input, &options, 1.0).unwrap();
        println!(
            "full-wave {} columns, {} cells, estimate {:?}, patches {:?}, flags {:?}",
            region.ports.len(),
            region.mesh.cells,
            region.estimate(),
            patches,
            region.flags
        );
        assert_eq!(
            region.component_posts.len(),
            4 + if options.source_height_nm.is_some() { 2 * region.ports.len() } else { 0 }
        );
        if mode == "budget" {
            let error = copper_extract::fullwave::extract(&model, &board, &input, &options).unwrap_err();
            println!("bounded full-wave refusal: {error}");
            assert!(matches!(error, copper_fullwave::Error::Budget(_)));
            assert!(error.to_string().contains("0 columns completed"));
        } else if mode != "geometry" {
            let result = copper_extract::fullwave::extract(&model, &board, &input, &options).unwrap();
            println!(
                "full-wave {} s, matrix difference {:?}, reasons {:?}",
                result.seconds, result.mesh_difference_pct, result.status_reasons
            );
            assert!(result.status_reasons.is_empty(), "{:?}", result.status_reasons);
        }
        return;
    }
    if std::env::var("COPPER_EMI_MESH_ONLY").is_ok() {
        let ports = copper_extract::board::loop_ports(&input).unwrap();
        let layout = copper_extract::board::build_layout(
            &model,
            &board,
            &input,
            &ports,
            &options.board,
            options.mesh.cell_nm,
            options.mesh.coarse_nm,
        )
        .unwrap();
        let mesh = copper_extract::mesh::mesh(&layout.layout, &options.mesh).unwrap();
        println!("mesh {:#?}", mesh.stats);
        assert!(mesh.stats.topology.mismatches.is_empty());
        assert_eq!(mesh.stats.unresolved_gap_count, 0);
        if options.series {
            for scale in [2.0, 4.0] {
                let spec = copper_extract::mesh::MeshSpec {
                    coarsen: false,
                    across: (options.mesh.across as f64 / if scale == 2.0 { 1.5 } else { 2.25 }).ceil() as usize,
                    scale: mesh.stats.scale * scale,
                    ..options.mesh.clone()
                };
                let coarse = copper_extract::mesh::mesh(&layout.layout, &spec).unwrap();
                println!("coarse x{scale} mesh {:#?}", coarse.stats);
                assert!(coarse.stats.topology.mismatches.is_empty());
                assert_eq!(coarse.stats.unresolved_gap_count, 0);
                if scale == 2.0 {
                    assert!(mesh.stats.filaments as f64 / coarse.stats.filaments as f64 >= 1.1);
                }
            }
        }
        return;
    }
    println!("filled; extracting {} ports", input.external_network().unwrap().unwrap().ports.len());
    let result =
        copper_extract::hotloop::extract_hot_loop_with_progress(&model, &board, &input, &options, |role, stage| {
            println!("{role}: {stage:?}")
        })
        .unwrap();
    println!("status {}: {:?}\nmeshes {:#?}", result.status, result.status_reasons, result.meshes);
    assert!(result.topology_mismatches.is_empty(), "{:?}", result.topology_mismatches);
    assert!(result.unresolved_gaps.is_empty(), "{:?}", result.unresolved_gaps);
    if options.series {
        assert_eq!(result.status, "ok", "{:?}", result.status_reasons);
    }
}
