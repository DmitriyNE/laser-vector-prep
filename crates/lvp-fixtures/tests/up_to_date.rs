//! The committed fixtures must be exactly what the generator produces.

use std::fs;

#[test]
fn committed_fixtures_match_generator() {
    let root = lvp_fixtures::fixtures_dir();
    let mut stale = Vec::new();
    for (rel, expected) in lvp_fixtures::render() {
        match fs::read(root.join(&rel)) {
            Ok(actual) if actual == expected => {}
            Ok(_) => stale.push(format!("{rel}: differs")),
            Err(e) => stale.push(format!("{rel}: {e}")),
        }
    }
    assert!(
        stale.is_empty(),
        "fixtures/ is stale, run `just fixtures`:\n{}",
        stale.join("\n")
    );
}

#[test]
fn manifest_is_consistent_with_documents() {
    let fixtures = lvp_fixtures::all();
    let manifest = lvp_fixtures::manifest(&fixtures);
    let entries: Vec<&serde_json::Value> = manifest["fixtures"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["format"] == "dxf")
        .collect();
    assert_eq!(entries.len(), fixtures.len());
    for (entry, f) in entries.iter().zip(&fixtures) {
        assert_eq!(entry["name"], f.name);
        // Every layer named in the manifest is declared in the document's layer table.
        for layer in entry["layers"].as_object().unwrap().keys() {
            assert!(
                f.document.layers.iter().any(|l| &l.name == layer),
                "{}: layer {layer} used but not declared",
                f.name
            );
        }
    }
}

#[test]
fn board_fixture_has_the_documented_copper_extent() {
    let f = lvp_fixtures::all()
        .into_iter()
        .find(|f| f.name == "board-islands")
        .unwrap();
    let b = f.document.layer_bounds(Some("TOP_LAYER")).unwrap();
    assert!((b.width() - 20.1).abs() < 1e-9);
    assert!((b.min.x - 2.4).abs() < 1e-9 && (b.min.y - 2.0).abs() < 1e-9);
    assert!((b.max.x - 22.5).abs() < 1e-9 && (b.max.y - 9.3).abs() < 1e-9);
    let o = f.document.layer_bounds(Some("ROUTE_TOOL_PATH")).unwrap();
    assert!((o.width() - 25.0).abs() < 1e-6 && (o.height() - 11.3).abs() < 1e-6);
}

#[test]
fn gerber_fixtures_declare_their_function_in_the_file() {
    for g in lvp_fixtures::gerber::all() {
        let line = format!("%TF.FileFunction,{}*%", g.function.unwrap());
        assert!(g.text.contains(&line), "{}: missing {line}", g.name);
        assert!(
            g.text.trim_end().ends_with("M02*"),
            "{}: no end of file",
            g.name
        );
        assert!(g.text.contains("%FSLAX46Y46*%") && g.text.contains("%MOMM*%"));
    }
}
