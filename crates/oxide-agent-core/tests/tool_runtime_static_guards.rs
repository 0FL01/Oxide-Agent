use std::fs;
use std::path::{Path, PathBuf};

fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }
}

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

#[test]
fn sandbox_manager_usage_stays_inside_sandbox_facades() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let agent_dir = manifest_dir.join("src/agent");
    let allowed_agent_facades = [manifest_dir.join("src/agent/providers/sandbox.rs")];
    let mut files = Vec::new();
    collect_rust_files(&agent_dir, &mut files);

    let offenders = files
        .into_iter()
        .filter(|path| !allowed_agent_facades.iter().any(|allowed| allowed == path))
        .filter(|path| {
            fs::read_to_string(path)
                .expect("read agent source file")
                .contains("SandboxManager")
        })
        .map(|path| {
            path.strip_prefix(manifest_dir)
                .expect("source path under manifest dir")
                .display()
                .to_string()
        })
        .collect::<Vec<_>>();

    assert!(
        offenders.is_empty(),
        "SandboxManager usage must stay behind sandbox module/facade boundaries; offenders: {offenders:?}"
    );
}
