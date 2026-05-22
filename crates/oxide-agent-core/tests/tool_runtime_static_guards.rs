use std::fs;
use std::path::Path;

#[test]
fn legacy_tool_bridge_module_is_removed() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));

    assert!(
        !manifest_dir.join("src/agent/tool_bridge.rs").exists(),
        "legacy tool_bridge.rs must stay removed; use agent::tool_runtime instead"
    );

    let agent_mod =
        fs::read_to_string(manifest_dir.join("src/agent/mod.rs")).expect("read agent/mod.rs");
    assert!(
        !agent_mod.contains("tool_bridge"),
        "agent module must not export the legacy tool bridge"
    );
}
