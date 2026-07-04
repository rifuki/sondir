//! Torture suite — the fault-injection matrix behind the "battle-tested" claim.
//!
//! Every fault here reproduces a real incident (2026-07-02) or a canary
//! discovery, injected into a synthetic Anchor workspace. sondir must emit the
//! EXACT finding (code + severity) for each — and stay silent on the healthy
//! baseline, because a preflight tool that cries wolf gets uninstalled.
//! Everything runs offline: no RPC, no solana CLI, no network.

use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use solana_pubkey::Pubkey;

// ---------------------------------------------------------------- fixture --

struct Fixture {
    root: PathBuf,
}

impl Fixture {
    /// Healthy baseline: workspace manifest, one program, arch-v1 .so,
    /// litesvm 0.12 locked, localnet cluster. Every fault is one mutation.
    fn healthy(tag: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("sondir-torture-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("programs/demo/src")).unwrap();
        fs::create_dir_all(root.join("target/deploy")).unwrap();

        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nresolver = \"2\"\nmembers = [\"programs/demo\"]\n",
        )
        .unwrap();
        fs::write(root.join("programs/demo/src/lib.rs"), "").unwrap();

        let fixture = Self { root };
        fixture.program_manifest(&[]);
        fixture.anchor_toml("localnet", &program_id_a().to_string());
        fixture.so(1); // arch v1 — matches litesvm 0.12
        fixture.lock(&[("litesvm", "0.12.0")]);
        fixture
    }

    fn anchor_toml(&self, cluster: &str, program_id: &str) {
        fs::write(
            self.root.join("Anchor.toml"),
            format!(
                "[programs.{cluster}]\ndemo = \"{program_id}\"\n\n[provider]\ncluster = \"{cluster}\"\nwallet = \"id.json\"\n"
            ),
        )
        .unwrap();
    }

    fn program_manifest(&self, deps: &[&str]) {
        fs::write(
            self.root.join("programs/demo/Cargo.toml"),
            format!(
                "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{}\n",
                deps.join("\n")
            ),
        )
        .unwrap();
    }

    /// Minimal ELF: magic + e_flags word at byte offset 48 (the SBPF arch).
    fn so(&self, arch_flag: u32) {
        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[48..52].copy_from_slice(&arch_flag.to_le_bytes());
        fs::write(self.root.join("target/deploy/demo.so"), elf).unwrap();
    }

    fn raw_so(&self, bytes: &[u8]) {
        fs::write(self.root.join("target/deploy/demo.so"), bytes).unwrap();
    }

    fn lock(&self, packages: &[(&str, &str)]) {
        let mut lock = String::from("version = 3\n");
        for (name, version) in packages {
            lock.push_str(&format!(
                "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\n"
            ));
        }
        fs::write(self.root.join("Cargo.lock"), lock).unwrap();
    }

    /// `[secret(32) || pubkey(32)]` byte-array keypair file, like solana-keygen writes.
    fn deploy_keypair(&self, pubkey_bytes: [u8; 32]) {
        let mut bytes = vec![7u8; 32];
        bytes.extend_from_slice(&pubkey_bytes);
        fs::write(
            self.root.join("target/deploy/demo-keypair.json"),
            serde_json::to_string(&bytes).unwrap(),
        )
        .unwrap();
    }

    fn doctor(&self) -> (serde_json::Value, i32) {
        let out = Command::new(env!("CARGO_BIN_EXE_sondir"))
            .args(["doctor", "--offline", "--json", "--path"])
            .arg(&self.root)
            .output()
            .expect("run sondir doctor");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let parsed = serde_json::from_str(&stdout)
            .unwrap_or_else(|_| panic!("doctor did not emit JSON. stdout: {stdout}"));
        (parsed, out.status.code().unwrap_or(-1))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn program_id_a() -> Pubkey {
    Pubkey::new_from_array([1; 32])
}

fn findings_of<'a>(report: &'a serde_json::Value, code: &str) -> Vec<&'a serde_json::Value> {
    report["findings"]
        .as_array()
        .expect("findings array")
        .iter()
        .filter(|f| f["code"] == code)
        .collect()
}

fn has(report: &serde_json::Value, code: &str, severity: &str, title_contains: &str) -> bool {
    findings_of(report, code).iter().any(|f| {
        f["severity"] == severity
            && f["title"]
                .as_str()
                .unwrap_or_default()
                .contains(title_contains)
    })
}

// ----------------------------------------------------------------- matrix --

#[test]
fn healthy_baseline_raises_no_alarms() {
    let fixture = Fixture::healthy("baseline");
    let (report, exit) = fixture.doctor();
    let noisy: Vec<String> = report["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| {
            (f["severity"] == "fail" || f["severity"] == "warn")
                // Machines without the anchor CLI legitimately warn here.
                && f["code"] != "toolchain-anchor"
        })
        .map(|f| f.to_string())
        .collect();
    assert!(noisy.is_empty(), "healthy fixture raised: {noisy:?}");
    assert_eq!(exit, 0);
}

#[test]
fn keypair_drift_is_a_fail() {
    let fixture = Fixture::healthy("drift");
    fixture.deploy_keypair([2; 32]); // != Anchor.toml's [1;32] id
    let (report, exit) = fixture.doctor();
    assert!(
        has(
            &report,
            "keypair-drift",
            "fail",
            "deploy keypair != Anchor.toml"
        ),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn matching_keypair_is_ok_not_noise() {
    let fixture = Fixture::healthy("match");
    fixture.deploy_keypair([1; 32]); // == Anchor.toml id
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "keypair-drift", "ok", "matches Anchor.toml"),
        "report: {report}"
    );
    assert_eq!(exit, 0);
}

#[test]
fn malformed_keypair_skips_without_crashing() {
    let fixture = Fixture::healthy("badkey");
    fs::write(
        fixture.root.join("target/deploy/demo-keypair.json"),
        "definitely not a keypair",
    )
    .unwrap();
    let (report, exit) = fixture.doctor();
    assert!(
        findings_of(&report, "keypair-drift").is_empty(),
        "report: {report}"
    );
    assert_ne!(exit, 2, "malformed keypair must not be an execution error");
}

#[test]
fn truncated_elf_is_a_fail() {
    let fixture = Fixture::healthy("truncated");
    fixture.raw_so(&[0x7f, b'E', b'L', b'F', 0, 0, 0, 0, 0, 0]); // magic, then nothing
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "arch-cluster", "fail", "not a valid ELF"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn arch_v3_artifact_breaks_litesvm_012_tests() {
    let fixture = Fixture::healthy("archv3");
    fixture.so(3); // deploy-arch binary against a 0.12 test runtime
    let (report, _) = fixture.doctor();
    assert!(
        has(
            &report,
            "arch-litesvm",
            "warn",
            "will NOT run under litesvm 0.12.0"
        ),
        "report: {report}"
    );
    // ...while the same artifact is fine for the cluster (SBPFv3 assumed active).
    assert!(
        has(&report, "arch-cluster", "ok", "deployable"),
        "report: {report}"
    );
}

#[test]
fn arch_v0_artifact_breaks_litesvm_012_tests() {
    let fixture = Fixture::healthy("archv0");
    fixture.so(0); // platform-tools >=1.54 default vs 0.12 runtime
    let (report, _) = fixture.doctor();
    assert!(
        has(
            &report,
            "arch-litesvm",
            "warn",
            "will NOT run under litesvm 0.12.0"
        ),
        "report: {report}"
    );
}

#[test]
fn litesvm_013_with_magicblock_is_a_declared_conflict() {
    let fixture = Fixture::healthy("magicblock");
    fixture.program_manifest(&[
        "litesvm = \"0.13.1\"",
        "ephemeral-rollups-sdk = { version = \"0.15.5\", features = [\"anchor\", \"vrf\"] }",
    ]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "litesvm"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn litesvm_013_with_sysvar_301_is_a_declared_conflict() {
    let fixture = Fixture::healthy("sysvar");
    fixture.program_manifest(&[
        "litesvm = \"0.13.1\"",
        "solana-instructions-sysvar = \"3.0.1\"",
    ]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "litesvm"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn sysvar_exact_300_escape_hatch_is_not_flagged() {
    let fixture = Fixture::healthy("sysvar-ok");
    fixture.program_manifest(&[
        "litesvm = \"0.13.1\"",
        "solana-instructions-sysvar = \"=3.0.0\"",
    ]);
    let (report, _) = fixture.doctor();
    assert!(
        findings_of(&report, "dep-conflict").is_empty(),
        "the =3.0.0 escape hatch must not be flagged: {report}"
    );
}

#[test]
fn legacy_solana_program_is_a_declared_conflict() {
    let fixture = Fixture::healthy("legacy");
    fixture.program_manifest(&["solana-program = \"1.18.26\""]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "solana-program 1.x"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn fix_dry_run_shows_the_plan_but_writes_nothing() {
    let fixture = Fixture::healthy("fix-dry");
    fixture.program_manifest(&["litesvm = \"0.13.1\"", "light-sdk = \"0.24.0\""]);
    let manifest = fixture.root.join("programs/demo/Cargo.toml");
    let before = fs::read_to_string(&manifest).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_sondir"))
        .args(["fix", "--path"])
        .arg(&fixture.root)
        .output()
        .expect("run sondir fix");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(stdout.contains("would apply"), "plan shown: {stdout}");
    assert!(stdout.contains("litesvm"), "names the pin: {stdout}");
    // The cardinal rule: a dry-run must not modify a single byte.
    assert_eq!(fs::read_to_string(&manifest).unwrap(), before);
    assert_eq!(
        out.status.code(),
        Some(1),
        "dry-run with pending fixes exits 1"
    );
}

#[test]
fn sweep_discovered_mollusk_conflict_reaches_doctor_with_no_new_code() {
    // The mollusk entries were added ONLY to facts.toml (sweep discovery
    // 2026-07-04) — data-driven matching must surface them in doctor too.
    let fixture = Fixture::healthy("mollusk");
    fixture.program_manifest(&[
        "mollusk-svm = \"0.13.4\"",
        "ephemeral-rollups-sdk = { version = \"0.15.5\", features = [\"anchor\", \"vrf\"] }",
    ]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "mollusk-svm"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn non_local_cluster_warns_about_anchor_test_deploys() {
    let fixture = Fixture::healthy("footgun");
    fixture.anchor_toml("devnet", &program_id_a().to_string());
    let (report, _) = fixture.doctor();
    assert!(
        has(&report, "anchor-test-footgun", "warn", "anchor test"),
        "report: {report}"
    );
}

#[test]
fn unresolvable_workspace_is_a_resolve_fail() {
    let fixture = Fixture::healthy("broken");
    fixture.program_manifest(&["missing = { path = \"../does-not-exist\" }"]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "resolve", "fail", "does NOT resolve"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

// -------------------------------------------------------- wave 2: nastier --

#[test]
fn unknown_future_arch_flag_is_a_fail_not_a_shrug() {
    let fixture = Fixture::healthy("archv9");
    fixture.so(9); // some future SBPF arch sondir has never heard of
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "arch-cluster", "fail", "NOT deployable"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn zero_byte_so_is_a_fail() {
    let fixture = Fixture::healthy("empty-so");
    fixture.raw_so(&[]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "arch-cluster", "fail", "not a valid ELF"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn drift_is_caught_even_before_the_first_build() {
    // `anchor deploy` builds first, THEN targets the stray keypair — so the
    // drift must be reported even when no .so exists yet.
    let fixture = Fixture::healthy("drift-nobuild");
    fs::remove_file(fixture.root.join("target/deploy/demo.so")).unwrap();
    fixture.deploy_keypair([2; 32]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(
            &report,
            "keypair-drift",
            "fail",
            "deploy keypair != Anchor.toml"
        ),
        "drift with no .so must still fail: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn conflict_is_caught_from_the_lockfile_alone() {
    // Manifest looks innocent; the lock tells the truth (canary c05 inverse).
    let fixture = Fixture::healthy("lock-conflict");
    fixture.lock(&[("litesvm", "0.13.1"), ("ephemeral-vrf-sdk", "0.3.0")]);
    let (report, exit) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "litesvm"),
        "report: {report}"
    );
    assert_eq!(exit, 1);
}

#[test]
fn caret_requirement_still_counts_as_013() {
    let fixture = Fixture::healthy("caret");
    fixture.program_manifest(&[
        "litesvm = \"^0.13\"",
        "ephemeral-rollups-sdk = { version = \"0.15.5\", features = [\"anchor\", \"vrf\"] }",
    ]);
    let (report, _) = fixture.doctor();
    assert!(
        has(&report, "dep-conflict", "fail", "litesvm"),
        "^0.13 must be recognized as the 0.13 line: {report}"
    );
}

#[test]
fn two_programs_one_drifted_yields_exactly_one_fail() {
    let fixture = Fixture::healthy("multi");
    let id_a = program_id_a();
    let id_b = Pubkey::new_from_array([2; 32]);
    fs::write(
        fixture.root.join("Anchor.toml"),
        format!(
            "[programs.localnet]\ndemo = \"{id_a}\"\ndemo2 = \"{id_b}\"\n\n[provider]\ncluster = \"localnet\"\nwallet = \"id.json\"\n"
        ),
    )
    .unwrap();
    fixture.deploy_keypair([1; 32]); // demo: matches
    let mut bytes = vec![7u8; 32];
    bytes.extend_from_slice(&[9; 32]); // demo2: drifted
    fs::write(
        fixture.root.join("target/deploy/demo2-keypair.json"),
        serde_json::to_string(&bytes).unwrap(),
    )
    .unwrap();

    let (report, exit) = fixture.doctor();
    let drift = findings_of(&report, "keypair-drift");
    let fails = drift.iter().filter(|f| f["severity"] == "fail").count();
    let oks = drift.iter().filter(|f| f["severity"] == "ok").count();
    assert_eq!((fails, oks), (1, 1), "report: {report}");
    assert_eq!(exit, 1);
}

#[test]
fn wrong_length_keypair_is_skipped_not_misread() {
    let fixture = Fixture::healthy("shortkey");
    fs::write(
        fixture.root.join("target/deploy/demo-keypair.json"),
        serde_json::to_string(&vec![7u8; 63]).unwrap(), // one byte short
    )
    .unwrap();
    let (report, exit) = fixture.doctor();
    assert!(
        findings_of(&report, "keypair-drift").is_empty(),
        "a 63-byte file must never be read as an address: {report}"
    );
    assert_ne!(exit, 2);
}

#[test]
fn raw_url_cluster_also_triggers_the_footgun_warning() {
    let fixture = Fixture::healthy("url-cluster");
    fixture.anchor_toml("devnet", &program_id_a().to_string());
    // provider.cluster as a raw URL, the way CI configs often write it.
    let anchor_toml = fs::read_to_string(fixture.root.join("Anchor.toml")).unwrap();
    fs::write(
        fixture.root.join("Anchor.toml"),
        anchor_toml.replace(
            "cluster = \"devnet\"",
            "cluster = \"https://api.devnet.solana.com\"",
        ),
    )
    .unwrap();
    let (report, _) = fixture.doctor();
    assert!(
        has(&report, "anchor-test-footgun", "warn", "anchor test"),
        "a URL cluster is remote too: {report}"
    );
}

// ------------------------------------------------------------- agent path --

#[test]
fn mcp_doctor_reports_the_same_drift() {
    let fixture = Fixture::healthy("mcp-drift");
    fixture.deploy_keypair([2; 32]);

    let mut child = Command::new(env!("CARGO_BIN_EXE_sondir"))
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn sondir mcp");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {
            "name": "sondir_doctor",
            "arguments": { "path": fixture.root.to_string_lossy(), "offline": true }
        }
    });
    writeln!(child.stdin.take().unwrap(), "{call}").unwrap();
    let out = child.wait_with_output().expect("wait mcp");

    let response: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&out.stdout).lines().next().unwrap())
            .expect("mcp response json");
    let report: serde_json::Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap())
            .expect("embedded report json");
    assert!(
        has(
            &report,
            "keypair-drift",
            "fail",
            "deploy keypair != Anchor.toml"
        ),
        "agent path must see the identical finding: {report}"
    );
}
