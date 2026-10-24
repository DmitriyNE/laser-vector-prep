//! Writes the generated fixtures into `fixtures/`. Run via `just fixtures`.

use std::fs;

fn main() -> std::io::Result<()> {
    let root = lvp_fixtures::fixtures_dir();
    for (rel, contents) in lvp_fixtures::render() {
        let path = root.join(&rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, contents)?;
        println!("wrote {}", path.display());
    }
    Ok(())
}
