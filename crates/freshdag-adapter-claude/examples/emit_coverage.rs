//! Regenerates `coverage.json` from the Rust constructor.
//!
//! `cargo run -p freshdag-adapter-claude --example emit_coverage`
//!
//! The `role` field is injected here because
//! `freshdag_core::ir::CoverageManifest` cannot express it yet (see the
//! PENDING PHASE A note in `src/coverage.rs`). Once it can, drop the
//! injection — the constructor will emit it.
fn main() {
    let manifest = freshdag_adapter_claude::coverage::coverage_manifest();
    let mut value = serde_json::to_value(&manifest).expect("manifest serializes");
    if let Some(obj) = value.as_object_mut() {
        obj.entry("role".to_string())
            .or_insert_with(|| serde_json::Value::String("adapter".to_string()));
    }
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("coverage.json");
    let mut text = serde_json::to_string_pretty(&value).expect("pretty-print");
    text.push('\n');
    std::fs::write(&path, text).expect("write coverage.json");
    eprintln!("wrote {}", path.display());
}
