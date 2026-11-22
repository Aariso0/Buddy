use std::process::Command;

fn run(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_buddy-cli"))
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn cli_create_show_recover_round_trip() {
    let root = tempfile::tempdir().unwrap();
    let id = "11111111-1111-4111-8111-111111111111";
    let dir = root.path().to_str().unwrap();
    let created: serde_json::Value =
        serde_json::from_str(&run(&["session", "new", "--root", dir, "--id", id])).unwrap();
    assert_eq!(created["format_version"], 1);
    let shown: serde_json::Value =
        serde_json::from_str(&run(&["session", "show", "--root", dir, "--id", id])).unwrap();
    assert_eq!(shown, created);
    let recovered: serde_json::Value =
        serde_json::from_str(&run(&["session", "recover", "--root", dir, "--id", id])).unwrap();
    assert_eq!(recovered["needs_repair"], true);
    assert_eq!(recovered["manifest"], created);
}
