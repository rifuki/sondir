//! `sondir fix` — apply the facts-DB dependency-pin remedies to Cargo.toml.
//!
//! Safety is the whole point. Guarantees:
//!   * DRY-RUN by default — prints the plan and touches nothing. Only `--write`
//!     mutates, and even then it edits *only* the version string of the *exact*
//!     dependency named by a facts remedy — never another dep, never another key.
//!   * Format-preserving via `toml_edit`: features, comments, ordering, and every
//!     other line are byte-for-byte identical; only the version value changes.
//!   * Only applies remedies that live in the facts DB (`fix_pin`) — never a
//!     guess, and never a removal or an on-chain/file operation.
//!   * Verify-then-keep: after writing, it re-resolves the workspace; if the fix
//!     did NOT make it resolve, every file is ROLLED BACK to its original bytes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use toml_edit::{DocumentMut, Item, Value};

use crate::checks;
use crate::facts;
use crate::project::Project;

const SECTIONS: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

struct Edit {
    file: PathBuf,
    section: String,
    krate: String,
    from: String,
    to: String,
}

pub fn run(project: &Project, write: bool) -> Result<i32> {
    // 1. Which conflicts apply, and which carry a machine-applicable pin?
    let conflicts = checks::applicable_conflicts(project);
    if conflicts.is_empty() {
        println!("✓ no known conflicts to fix");
        return Ok(0);
    }

    // Dedupe the pins (one crate can be implicated by several conflicts).
    let mut pins: BTreeMap<String, String> = BTreeMap::new();
    let mut manual: Vec<&facts::KnownConflict> = Vec::new();
    for conflict in &conflicts {
        match conflict.fix_pin.as_deref().and_then(facts::parse_probe) {
            Some((krate, req, _)) => {
                pins.insert(krate, req);
            }
            None => manual.push(conflict),
        }
    }

    // 2. Find each pin's declaration in the workspace manifests → concrete edits.
    let manifests = manifest_paths(&project.root);
    let mut plan: Vec<Edit> = Vec::new();
    let mut unlocatable: Vec<String> = Vec::new();
    for (krate, req) in &pins {
        let mut found = false;
        for path in &manifests {
            for edit in locate(path, krate, req)? {
                found = true;
                plan.push(edit);
            }
        }
        if !found {
            unlocatable.push(krate.clone());
        }
    }

    // 3. Show the plan (this is all a dry-run does).
    print_plan(&plan, &manual, &unlocatable, write);

    if plan.is_empty() {
        return Ok(if manual.is_empty() { 0 } else { 1 });
    }
    if !write {
        println!("\ndry-run — nothing written. Re-run with `--write` to apply.");
        return Ok(1);
    }

    // 4. Apply, keeping every original in memory for rollback.
    let mut originals: BTreeMap<PathBuf, String> = BTreeMap::new();
    for edit in &plan {
        if !originals.contains_key(&edit.file) {
            originals.insert(
                edit.file.clone(),
                std::fs::read_to_string(&edit.file)
                    .with_context(|| format!("cannot read {}", edit.file.display()))?,
            );
        }
    }
    for (file, original) in &originals {
        let edited = apply_file(original, &pins)?;
        std::fs::write(file, edited).with_context(|| format!("cannot write {}", file.display()))?;
    }

    // 5. Verify the workspace now resolves; roll back if it doesn't.
    if resolves(&project.root) {
        println!("\n✓ applied {} edit(s) — workspace resolves.", plan.len());
        Ok(0)
    } else {
        for (file, original) in &originals {
            std::fs::write(file, original)?;
        }
        println!(
            "\n✗ the pins did NOT make the workspace resolve — ROLLED BACK all {} file(s). \
             This conflict needs manual attention (`sondir doctor` / `resolve` for the chain).",
            originals.len()
        );
        Ok(1)
    }
}

/// Root manifest + one level of the usual member dirs. We never chase globs we
/// can't see — an un-found crate is reported, never invented.
fn manifest_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = vec![root.join("Cargo.toml")];
    for members in ["programs", "crates"] {
        if let Ok(entries) = std::fs::read_dir(root.join(members)) {
            for entry in entries.filter_map(std::result::Result::ok) {
                let manifest = entry.path().join("Cargo.toml");
                if manifest.is_file() {
                    paths.push(manifest);
                }
            }
        }
    }
    paths.retain(|p| p.is_file());
    paths
}

/// Every declaration of `krate` in this manifest whose version differs from
/// `req` (a `[section]` or `[workspace.dependencies]` entry). Path/git deps and
/// already-satisfied entries yield nothing.
fn locate(path: &Path, krate: &str, req: &str) -> Result<Vec<Edit>> {
    let raw = std::fs::read_to_string(path)?;
    let doc: DocumentMut = raw
        .parse()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    let mut edits = Vec::new();
    let mut consider = |table: Option<&Item>, section: &str| {
        if let Some(item) = table.and_then(|t| t.get(krate)) {
            if let Some(from) = current_version(item) {
                if from != req {
                    edits.push(Edit {
                        file: path.to_owned(),
                        section: section.to_owned(),
                        krate: krate.to_owned(),
                        from,
                        to: req.to_owned(),
                    });
                }
            }
        }
    };
    for section in SECTIONS {
        consider(doc.get(section), section);
    }
    consider(
        doc.get("workspace").and_then(|w| w.get("dependencies")),
        "workspace.dependencies",
    );
    Ok(edits)
}

/// The version string of a dependency item, whether it's `dep = "x"` or
/// `dep = { version = "x", ... }`. `None` for path/git/versionless deps — which
/// `fix` must not touch.
fn current_version(item: &Item) -> Option<String> {
    if let Some(s) = item.as_str() {
        return Some(s.to_owned());
    }
    if let Some(t) = item.as_inline_table() {
        return t.get("version").and_then(|v| v.as_str()).map(str::to_owned);
    }
    if let Some(t) = item.as_table() {
        return t.get("version").and_then(|v| v.as_str()).map(str::to_owned);
    }
    None
}

/// Set the version of a dependency item in place, preserving features/other keys.
/// Returns false when the item has no version to set (path/git dep).
fn set_version(item: &mut Item, req: &str) -> bool {
    if item.is_str() {
        *item = toml_edit::value(req);
        return true;
    }
    if let Some(t) = item.as_inline_table_mut() {
        if t.contains_key("version") {
            t.insert("version", Value::from(req));
            return true;
        }
    }
    if let Some(t) = item.as_table_mut() {
        if t.contains_key("version") {
            t["version"] = toml_edit::value(req);
            return true;
        }
    }
    false
}

/// Apply every applicable pin to one manifest's text, returning the new text.
fn apply_file(raw: &str, pins: &BTreeMap<String, String>) -> Result<String> {
    let mut doc: DocumentMut = raw.parse()?;
    for (krate, req) in pins {
        for section in SECTIONS {
            if let Some(item) = doc.get_mut(section).and_then(|t| t.get_mut(krate)) {
                set_version(item, req);
            }
        }
        if let Some(item) = doc
            .get_mut("workspace")
            .and_then(|w| w.get_mut("dependencies"))
            .and_then(|d| d.get_mut(krate))
        {
            set_version(item, req);
        }
    }
    Ok(doc.to_string())
}

/// Does the workspace resolve now? (`cargo metadata` forces a full resolve.)
fn resolves(root: &Path) -> bool {
    Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn print_plan(
    plan: &[Edit],
    manual: &[&facts::KnownConflict],
    unlocatable: &[String],
    write: bool,
) {
    let head = if write { "applying" } else { "would apply" };

    if plan.is_empty() && manual.is_empty() {
        println!("✓ nothing to fix");
    }
    if !plan.is_empty() {
        println!("{head} {} dependency pin(s):", plan.len());
        for e in plan {
            println!(
                "  ~ [{}] {} = \"{}\" -> \"{}\"",
                e.section, e.krate, e.from, e.to
            );
            println!("      in {}", e.file.display());
        }
    }
    for krate in unlocatable {
        println!(
            "  ? {krate}: conflict applies but it's not a direct dependency here \
             (transitive) — add an explicit `{krate} = \"...\"` pin or a [patch]"
        );
    }
    for conflict in manual {
        println!(
            "  ⚠ [{}] no auto-fix (manual): {}",
            conflict.id, conflict.fix
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(krate: &str, req: &str) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        map.insert(krate.to_owned(), req.to_owned());
        map
    }

    #[test]
    fn edits_only_the_version_keeping_features_comments_and_neighbours() {
        let raw = "[dependencies]\n\
                   # keep this comment\n\
                   litesvm = { version = \"0.13.1\", features = [\"foo\"] }\n\
                   neighbour = \"9.9.9\"\n";
        let out = apply_file(raw, &pins("litesvm", "0.12")).unwrap();
        assert!(out.contains("version = \"0.12\""), "{out}");
        assert!(
            out.contains("features = [\"foo\"]"),
            "features preserved: {out}"
        );
        assert!(
            out.contains("# keep this comment"),
            "comment preserved: {out}"
        );
        assert!(
            out.contains("neighbour = \"9.9.9\""),
            "neighbour untouched: {out}"
        );
        assert!(!out.contains("0.13.1"), "old version gone: {out}");
    }

    #[test]
    fn edits_the_plain_string_form() {
        let out = apply_file(
            "[dev-dependencies]\nlitesvm = \"0.13.1\"\n",
            &pins("litesvm", "0.12"),
        )
        .unwrap();
        assert_eq!(out, "[dev-dependencies]\nlitesvm = \"0.12\"\n");
    }

    #[test]
    fn never_touches_a_path_or_git_dep() {
        let raw = "[dependencies]\nlitesvm = { path = \"../litesvm\" }\n";
        assert_eq!(apply_file(raw, &pins("litesvm", "0.12")).unwrap(), raw);
    }

    #[test]
    fn never_touches_an_unrelated_crate() {
        let raw = "[dependencies]\nkeepme = \"0.13.1\"\n";
        assert_eq!(apply_file(raw, &pins("litesvm", "0.12")).unwrap(), raw);
    }

    #[test]
    fn locate_skips_a_crate_already_at_the_target() {
        let dir = std::env::temp_dir().join(format!("sondir-fix-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let manifest = dir.join("Cargo.toml");
        std::fs::write(&manifest, "[dev-dependencies]\nlitesvm = \"0.12\"\n").unwrap();
        assert!(locate(&manifest, "litesvm", "0.12").unwrap().is_empty());
        // ...but a differing version IS located.
        std::fs::write(&manifest, "[dev-dependencies]\nlitesvm = \"0.13.1\"\n").unwrap();
        assert_eq!(locate(&manifest, "litesvm", "0.12").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
