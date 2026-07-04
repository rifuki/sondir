//! End-to-end CLI behavior — exercised against the real binary.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_sondir"))
}

fn fixture(name: &str, anchor_toml: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("sondir-test-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("programs/demo")).expect("mkdir fixture");
    fs::write(dir.join("Anchor.toml"), anchor_toml).expect("write Anchor.toml");
    fs::write(
        dir.join("programs/demo/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\n",
    )
    .expect("write program manifest");
    dir
}

#[test]
fn missing_anchor_toml_is_a_friendly_error_with_exit_2() {
    let dir = std::env::temp_dir().join(format!("sondir-test-empty-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("mkdir");
    let out = bin()
        .args(["doctor", "--path"])
        .arg(&dir)
        .output()
        .expect("run sondir");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Anchor workspace"), "stderr was: {stderr}");
}

#[test]
fn offline_doctor_on_minimal_fixture_emits_valid_json() {
    let dir = fixture(
        "minimal",
        "[provider]\ncluster = \"localnet\"\nwallet = \"~/.config/solana/id.json\"\n",
    );
    let out = bin()
        .args(["doctor", "--offline", "--json", "--path"])
        .arg(&dir)
        .output()
        .expect("run sondir");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(parsed["findings"].is_array());
    // no ANSI escapes in machine output
    assert!(!stdout.contains('\u{1b}'));
}

#[test]
fn garbage_so_is_flagged_not_an_elf() {
    let dir = fixture(
        "garbage",
        "[programs.localnet]\ndemo = \"7uXfkM1LGqy8wQkBV6Dg7mvQwFTTBBNjBnwM6FxSkVob\"\n[provider]\ncluster = \"localnet\"\n",
    );
    fs::create_dir_all(dir.join("target/deploy")).expect("mkdir deploy");
    fs::write(dir.join("target/deploy/demo.so"), b"definitely not an elf").expect("write so");
    let out = bin()
        .args(["doctor", "--offline", "--json", "--path"])
        .arg(&dir)
        .output()
        .expect("run sondir");
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("valid JSON");
    let has_elf_fail = parsed["findings"]
        .as_array()
        .expect("array")
        .iter()
        .any(|f| {
            f["code"] == "arch-cluster"
                && f["severity"] == "fail"
                && f["title"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not a valid ELF")
        });
    assert!(has_elf_fail, "findings: {parsed}");
    assert_eq!(out.status.code(), Some(1), "fail finding must exit 1");
}

#[test]
fn mcp_server_handshakes_and_lists_tools() {
    // Drive the stdio MCP server: initialize, a notification (no reply), then
    // tools/list. Offline — no RPC or cargo calls in this script.
    let mut child = bin()
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn sondir mcp");
    let script = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"resources/list"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"sondir://facts"}}"#,
        "\n",
    );
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(script.as_bytes())
        .expect("write script");
    let out = child.wait_with_output().expect("wait mcp");
    let stdout = String::from_utf8_lossy(&out.stdout);

    // One JSON object per line; the notification produced no line, so exactly four.
    let lines: Vec<&str> = stdout.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 4, "expected 4 responses, got: {stdout}");

    let init: serde_json::Value = serde_json::from_str(lines[0]).expect("init json");
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["serverInfo"]["name"], "sondir");
    assert!(init["result"]["capabilities"]["resources"].is_object());

    let list: serde_json::Value = serde_json::from_str(lines[1]).expect("list json");
    assert_eq!(list["id"], 2);
    let tools = list["result"]["tools"].as_array().expect("tools array");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert!(names.contains(&"sondir_doctor"), "tools: {names:?}");
    assert!(names.contains(&"sondir_resolve"), "tools: {names:?}");
    assert!(names.contains(&"sondir_watch"), "tools: {names:?}");
    assert!(names.contains(&"sondir_facts_verify"), "tools: {names:?}");
    assert!(
        tools
            .iter()
            .all(|t| t["annotations"]["readOnlyHint"] == true),
        "every tool must be marked read-only"
    );

    let resources: serde_json::Value = serde_json::from_str(lines[2]).expect("resources json");
    let uris: Vec<&str> = resources["result"]["resources"]
        .as_array()
        .expect("resources array")
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(uris.contains(&"sondir://facts"), "resources: {uris:?}");

    let read: serde_json::Value = serde_json::from_str(lines[3]).expect("read json");
    let text = read["result"]["contents"][0]["text"]
        .as_str()
        .expect("facts text");
    assert!(
        text.contains("[[conflicts]]") && text.contains("litesvm-magicblock"),
        "facts resource must serve the TOML"
    );
}
