//! Reads the local project: Anchor.toml, Cargo.lock, target/deploy artifacts,
//! and (via shell-outs) the installed toolchain.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

pub struct Project {
    pub root: PathBuf,
    pub anchor: AnchorConfig,
    pub locked: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct AnchorConfig {
    #[serde(default)]
    pub toolchain: Toolchain,
    #[serde(default)]
    pub provider: Provider,
    /// programs.<cluster> -> { program-name -> program-id }
    #[serde(default)]
    pub programs: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    pub scripts: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Toolchain {
    pub anchor_version: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Provider {
    #[serde(default)]
    pub cluster: String,
    #[serde(default)]
    pub wallet: String,
}

pub struct Artifact {
    /// Program name as it appears in Anchor.toml (kebab-case).
    pub name: String,
    /// On-chain program id from Anchor.toml for the active cluster.
    pub program_id: Option<String>,
    pub so_path: PathBuf,
    pub so_len: u64,
    /// SBPF arch flag: ELF e_flags at byte offset 48.
    pub sbpf_flag: u32,
}

impl Project {
    pub fn load(root: &Path) -> Result<Self> {
        let anchor_path = root.join("Anchor.toml");
        let anchor_raw = fs::read_to_string(&anchor_path).with_context(|| {
            format!(
                "cannot read {} — is this an Anchor workspace? (point --path at the directory containing Anchor.toml)",
                anchor_path.display()
            )
        })?;
        let anchor: AnchorConfig =
            toml::from_str(&anchor_raw).context("Anchor.toml did not parse")?;

        let locked = read_lockfile(&root.join("Cargo.lock")).unwrap_or_default();

        Ok(Self {
            root: root.to_owned(),
            anchor,
            locked,
        })
    }

    /// The `[programs.<cluster>]` table matching the provider cluster, falling
    /// back to any cluster table so doctor still works on unusual configs.
    pub fn program_ids(&self) -> BTreeMap<String, String> {
        let cluster = normalize_cluster_key(&self.anchor.provider.cluster);
        if let Some(programs) = self.anchor.programs.get(&cluster) {
            return programs.clone();
        }
        self.anchor
            .programs
            .values()
            .next()
            .cloned()
            .unwrap_or_default()
    }

    /// Scan `target/deploy/*.so` and pair with Anchor.toml program ids.
    pub fn artifacts(&self) -> Vec<Artifact> {
        let ids = self.program_ids();
        let mut artifacts = Vec::new();
        for (name, program_id) in &ids {
            let artifact_name = name.replace('-', "_");
            let so_path = self
                .root
                .join("target/deploy")
                .join(format!("{artifact_name}.so"));
            if let Ok(bytes) = fs::read(&so_path) {
                artifacts.push(Artifact {
                    name: name.clone(),
                    program_id: Some(program_id.clone()),
                    so_len: bytes.len() as u64,
                    sbpf_flag: sbpf_flag(&bytes),
                    so_path,
                });
            }
        }
        artifacts
    }

    /// `*-upgrade-buffer.json` keypairs left behind by interrupted deploys.
    pub fn stranded_buffer_keypairs(&self) -> Vec<PathBuf> {
        let deploy_dir = self.root.join("target/deploy");
        let Ok(entries) = fs::read_dir(&deploy_dir) else {
            return Vec::new();
        };
        entries
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.ends_with("upgrade-buffer.json"))
            })
            .collect()
    }

    pub fn rpc_url(&self, override_url: Option<&str>) -> String {
        if let Some(url) = override_url {
            return url.to_owned();
        }
        if let Ok(url) = std::env::var("SONDIR_RPC") {
            return url;
        }
        cluster_to_url(&self.anchor.provider.cluster)
    }
}

fn read_lockfile(path: &Path) -> Result<BTreeMap<String, String>> {
    parse_lockfile(&fs::read_to_string(path)?)
}

fn parse_lockfile(raw: &str) -> Result<BTreeMap<String, String>> {
    #[derive(Deserialize)]
    struct Lock {
        #[serde(default)]
        package: Vec<LockPackage>,
    }
    #[derive(Deserialize)]
    struct LockPackage {
        name: String,
        version: String,
    }
    let lock: Lock = toml::from_str(raw)?;
    Ok(lock
        .package
        .into_iter()
        .map(|p| (p.name, p.version))
        .collect())
}

/// ELF `e_flags` lives at byte offset 48 (little-endian u32). Solana encodes
/// the SBPF arch version there.
pub fn sbpf_flag(elf: &[u8]) -> u32 {
    elf.get(48..52)
        .and_then(|b| b.try_into().ok())
        .map(u32::from_le_bytes)
        .unwrap_or(u32::MAX)
}

fn normalize_cluster_key(cluster: &str) -> String {
    match cluster {
        c if c.contains("devnet") => "devnet".into(),
        c if c.contains("testnet") => "testnet".into(),
        c if c.contains("mainnet") => "mainnet".into(),
        "localnet" | "localhost" => "localnet".into(),
        other => other.into(),
    }
}

fn cluster_to_url(cluster: &str) -> String {
    match cluster {
        "devnet" => "https://api.devnet.solana.com".into(),
        "testnet" => "https://api.testnet.solana.com".into(),
        "mainnet" | "mainnet-beta" => "https://api.mainnet-beta.solana.com".into(),
        "localnet" | "localhost" | "" => "http://127.0.0.1:8899".into(),
        url => url.into(),
    }
}

/// `solana-keygen pubkey <path>` — avoids pulling ed25519 into our deps.
pub fn keypair_pubkey(path: &Path) -> Result<String> {
    let output = Command::new("solana-keygen")
        .arg("pubkey")
        .arg(path)
        .output()
        .context("solana-keygen not found on PATH")?;
    if !output.status.success() {
        return Err(anyhow!(
            "solana-keygen pubkey {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

pub fn tool_version(tool: &str) -> Option<String> {
    let output = Command::new(tool).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbpf_flag_reads_e_flags_at_offset_48() {
        let mut elf = vec![0u8; 64];
        elf[48] = 3;
        assert_eq!(sbpf_flag(&elf), 3);
    }

    #[test]
    fn sbpf_flag_on_truncated_file_is_sentinel() {
        assert_eq!(sbpf_flag(&[0u8; 10]), u32::MAX);
    }

    #[test]
    fn cluster_key_normalizes_urls() {
        assert_eq!(
            normalize_cluster_key("https://foo.solana-devnet.quiknode.pro/abc"),
            "devnet"
        );
        assert_eq!(normalize_cluster_key("localnet"), "localnet");
    }

    #[test]
    fn cluster_to_url_maps_known_names_and_passes_urls_through() {
        assert_eq!(cluster_to_url("devnet"), "https://api.devnet.solana.com");
        assert_eq!(cluster_to_url("http://my-rpc:8899"), "http://my-rpc:8899");
        assert_eq!(cluster_to_url(""), "http://127.0.0.1:8899");
    }

    #[test]
    fn anchor_config_parses_the_shapes_doctor_relies_on() {
        let config: AnchorConfig = toml::from_str(
            r#"
            [toolchain]
            anchor_version = "1.1.2"
            [programs.devnet]
            my-program = "7uXfkM1LGqy8wQkBV6Dg7mvQwFTTBBNjBnwM6FxSkVob"
            [provider]
            cluster = "devnet"
            wallet = "wallet.json"
            [scripts]
            test = "cargo test"
            "#,
        )
        .expect("valid Anchor.toml");
        assert_eq!(config.toolchain.anchor_version.as_deref(), Some("1.1.2"));
        assert_eq!(config.provider.cluster, "devnet");
        assert_eq!(config.programs["devnet"]["my-program"].len(), 44);
        assert_eq!(config.scripts["test"], "cargo test");
    }

    #[test]
    fn lockfile_parse_extracts_name_version_pairs() {
        let locked = parse_lockfile(
            r#"
            version = 4
            [[package]]
            name = "litesvm"
            version = "0.12.0"
            [[package]]
            name = "anchor-lang"
            version = "1.1.2"
            "#,
        )
        .expect("valid lockfile");
        assert_eq!(locked["litesvm"], "0.12.0");
        assert_eq!(locked["anchor-lang"], "1.1.2");
    }
}
