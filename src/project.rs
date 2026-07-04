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

    /// Declared (not resolved) dependency versions across `programs/*/Cargo.toml`.
    ///
    /// The lockfile lies when resolution itself failed: `cargo add` writes the
    /// manifest, fails to re-lock, and the stale lock never mentions the new
    /// dep. Conflict checks must therefore also see what is DECLARED.
    /// (Found by canary c05.)
    pub fn declared_deps(&self) -> BTreeMap<String, String> {
        let mut declared = BTreeMap::new();
        let Ok(entries) = fs::read_dir(self.root.join("programs")) else {
            return declared;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let manifest = entry.path().join("Cargo.toml");
            let Ok(raw) = fs::read_to_string(&manifest) else {
                continue;
            };
            for (name, version) in parse_declared(&raw) {
                declared.insert(name, version);
            }
        }
        declared
    }

    /// A dependency's version: resolved (lockfile) first, declared (manifest)
    /// as fallback — the lock can be stale or absent after a failed resolve.
    pub fn dep_version(&self, name: &str) -> Option<String> {
        self.locked
            .get(name)
            .cloned()
            .or_else(|| self.declared_deps().get(name).cloned())
    }

    /// The provider wallet as an absolute path (`~` expanded, relative paths
    /// anchored at the workspace root). `None` when unset.
    pub fn wallet_path(&self) -> Option<PathBuf> {
        let raw = self.anchor.provider.wallet.trim();
        if raw.is_empty() {
            return None;
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        Some(resolve_wallet_path(raw, home.as_deref(), &self.root))
    }
}

/// Anchor configs routinely use `~/.config/solana/id.json`; the shell never
/// expands that for us. Absolute paths pass through; everything else is
/// workspace-relative.
fn resolve_wallet_path(raw: &str, home: Option<&Path>, root: &Path) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = home {
            return home.join(rest);
        }
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    }
}

fn parse_declared(raw: &str) -> Vec<(String, String)> {
    // `str::parse::<Value>()` parses a single value; a manifest is a whole
    // document, so go through the deserializer (toml 1.x tightened this).
    let Ok(value) = toml::from_str::<toml::Value>(raw) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for table in ["dependencies", "dev-dependencies"] {
        if let Some(deps) = value.get(table).and_then(|v| v.as_table()) {
            for (name, spec) in deps {
                let version = match spec {
                    toml::Value::String(v) => v.clone(),
                    toml::Value::Table(t) => t
                        .get("version")
                        .and_then(|v| v.as_str())
                        .unwrap_or("*")
                        .to_owned(),
                    _ => "*".to_owned(),
                };
                out.push((name.clone(), version));
            }
        }
    }
    out
}

fn read_lockfile(path: &Path) -> Result<BTreeMap<String, String>> {
    parse_lockfile(&fs::read_to_string(path)?)
}

pub(crate) fn parse_lockfile(raw: &str) -> Result<BTreeMap<String, String>> {
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
    // A truncated or non-ELF file must not read as "arch v0" — guard the magic
    // before trusting e_flags (audit pass 1).
    if !elf.starts_with(&[0x7f, b'E', b'L', b'F']) {
        return u32::MAX;
    }
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

/// Read the pubkey out of a Solana keypair file without shelling out: the
/// file is a JSON array of 64 bytes laid out `[secret(32) || pubkey(32)]`,
/// so the address is literally the last 32 bytes — no signing crypto needed.
/// (Was `solana-keygen pubkey`; the shell-out made every keypair check depend
/// on the CLI being installed and made the test suite non-hermetic.)
pub fn keypair_pubkey(path: &Path) -> Result<String> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("cannot read keypair file {}", path.display()))?;
    let bytes: Vec<u8> = serde_json::from_str(&raw)
        .with_context(|| format!("{} is not a JSON byte-array keypair", path.display()))?;
    if bytes.len() != 64 {
        return Err(anyhow!(
            "{}: expected 64-byte keypair, got {} bytes",
            path.display(),
            bytes.len()
        ));
    }
    let pubkey = solana_pubkey::Pubkey::try_from(&bytes[32..])
        .map_err(|_| anyhow!("{}: malformed public key half", path.display()))?;
    Ok(pubkey.to_string())
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
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
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

#[cfg(test)]
mod declared_tests {
    use super::*;

    #[test]
    fn declared_deps_sees_plain_and_table_specs() {
        // Realistic manifest layout — no leading indentation, as cargo writes it.
        let deps = parse_declared(
            "[dependencies]\n\
             ephemeral-rollups-sdk = { version = \"0.15.5\", features = [\"anchor\", \"vrf\"] }\n\
             [dev-dependencies]\n\
             litesvm = \"0.13.1\"\n",
        );
        assert!(deps.contains(&("litesvm".into(), "0.13.1".into())));
        assert!(deps.contains(&("ephemeral-rollups-sdk".into(), "0.15.5".into())));
    }
}

#[cfg(test)]
mod audit_tests {
    use super::*;

    #[test]
    fn wallet_tilde_expands_to_home() {
        let p = resolve_wallet_path(
            "~/.config/solana/id.json",
            Some(Path::new("/home/u")),
            Path::new("/repo"),
        );
        assert_eq!(p, PathBuf::from("/home/u/.config/solana/id.json"));
    }

    #[test]
    fn wallet_absolute_passes_through() {
        let p = resolve_wallet_path(
            "/abs/wallet.json",
            Some(Path::new("/home/u")),
            Path::new("/repo"),
        );
        assert_eq!(p, PathBuf::from("/abs/wallet.json"));
    }

    #[test]
    fn wallet_relative_anchors_at_root() {
        let p = resolve_wallet_path(
            ".raflux/owner.json",
            Some(Path::new("/home/u")),
            Path::new("/repo"),
        );
        assert_eq!(p, PathBuf::from("/repo/.raflux/owner.json"));
    }

    #[test]
    fn non_elf_bytes_never_read_as_arch_v0() {
        let garbage = vec![0u8; 64];
        assert_eq!(sbpf_flag(&garbage), u32::MAX);
        let mut elf = vec![0u8; 64];
        elf[..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        elf[48] = 1;
        assert_eq!(sbpf_flag(&elf), 1);
    }
}
