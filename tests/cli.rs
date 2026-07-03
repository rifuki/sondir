//! End-to-end CLI behavior — exercised against the real binary.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

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
