//! `sondir sweep` — proactive conflict discovery.
//!
//! The facts DB records conflicts people already crashed into. Sweep inverts
//! that: probe every PAIR of ecosystem crates through cargo's own resolver,
//! the way a fresh project hits them — **latest × latest** first (what two
//! `cargo add`s produce). When latest fails, a second `*` probe classifies it:
//! still failing = a HARD conflict (no version combination exists at all);
//! resolving = a LATEST-ONLY conflict (works only because cargo backtracks —
//! the litesvm-magicblock shape). Anything not in the facts DB is a NEW
//! CANDIDATE, surfaced before any user hits it. Run weekly from CI; exit 5
//! (distinct from watch's 3 and facts-verify's 4) when candidates appear.

use std::collections::BTreeSet;
use std::sync::Mutex;

use anyhow::Result;
use serde::Serialize;

use crate::facts;
use crate::resolve::{compile_probe, probe, CompileProbeResult, ProbeResult, ALIASES};
use crate::watch::crates_io_max_version;

#[derive(Serialize, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
pub enum HitKind {
    /// No version combination resolves at all.
    Hard,
    /// Latest × latest fails; cargo only escapes by backtracking one side.
    LatestOnly,
    /// Resolves cleanly at latest and then fails `cargo check`. Invisible to the
    /// resolver-only sweep — the class that let 66/66 read "clean" on 2026-07-27
    /// while a fresh litesvm project could not be built (canary c24).
    CompileOnly,
}

#[derive(Serialize)]
pub struct SweepHit {
    pub pair: [String; 2],
    pub latest: [String; 2],
    pub kind: HitKind,
    /// The facts-DB conflict id when this failure is already recorded.
    pub known: Option<String>,
    /// For latest-only hits: the versions the backtracking escape lands on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub escape: Option<Vec<String>>,
    pub excerpt: String,
}

#[derive(Serialize)]
pub struct SweepReport {
    pub probed: usize,
    pub clean: usize,
    pub hits: Vec<SweepHit>,
    /// What `clean` actually means for this run. Without `--compile` it is only
    /// "cargo could select versions" — on 2026-07-27 that read 66/66 while 21 of
    /// those pairs did not compile. A number this easy to misread has to carry
    /// its own qualifier, in the JSON as well as the printed report.
    pub depth: &'static str,
}

struct Subject {
    krate: &'static str,
    latest: String,
    features: Vec<String>,
}

pub fn run(json: bool, compile: bool) -> Result<i32> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(15)))
        .build()
        .into();

    eprintln!("resolving latest versions from crates.io…");
    let mut subjects = Vec::new();
    for alias in ALIASES {
        match crates_io_max_version(&agent, alias.krate) {
            Ok(latest) => subjects.push(Subject {
                krate: alias.krate,
                latest,
                features: alias.features.iter().map(|f| (*f).to_owned()).collect(),
            }),
            Err(err) => eprintln!("  skipping {}: {err:#}", alias.krate),
        }
    }

    let mut pairs = Vec::new();
    for i in 0..subjects.len() {
        for j in i + 1..subjects.len() {
            pairs.push((i, j));
        }
    }
    let total = pairs.len();
    eprintln!(
        "sweeping {total} pairs across {} ecosystem crates (latest × latest)…",
        subjects.len()
    );
    if compile {
        eprintln!(
            "  --compile: every pair that RESOLVES also gets `cargo check`. Expect this to \
             take tens of minutes on a cold cache; builds share {}",
            crate::resolve::compile_probe_target_dir().display()
        );
    }

    // Modest parallelism: cargo serializes on its package-cache locks, but
    // index/network wait still overlaps. Work is pulled from a shared queue.
    let queue = Mutex::new(pairs);
    let results: Mutex<Vec<SweepHit>> = Mutex::new(Vec::new());
    let clean = Mutex::new(0usize);
    let done = Mutex::new(0usize);

    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| loop {
                let Some((i, j)) = queue.lock().unwrap().pop() else {
                    return;
                };
                let (a, b) = (&subjects[i], &subjects[j]);
                if let Some(hit) = sweep_pair(a, b, i, j, compile) {
                    results.lock().unwrap().push(hit);
                } else {
                    *clean.lock().unwrap() += 1;
                }
                let mut done = done.lock().unwrap();
                *done += 1;
                eprintln!("  {}/{total} probed", *done);
            });
        }
    });

    let mut hits = results.into_inner().unwrap();
    hits.sort_by(|x, y| x.pair.cmp(&y.pair));
    let report = SweepReport {
        probed: total,
        clean: clean.into_inner().unwrap(),
        hits,
        depth: if compile {
            "resolve+compile"
        } else {
            "resolve-only"
        },
    };

    let new_candidates = report.hits.iter().filter(|h| h.known.is_none()).count();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report, new_candidates);
    }
    // Exit 5 on new candidates (3 = watch fired, 4 = fact stale).
    Ok(if new_candidates > 0 { 5 } else { 0 })
}

fn sweep_pair(a: &Subject, b: &Subject, i: usize, j: usize, compile: bool) -> Option<SweepHit> {
    let latest_deps = vec![
        (
            a.krate.to_owned(),
            format!("={}", a.latest),
            a.features.clone(),
        ),
        (
            b.krate.to_owned(),
            format!("={}", b.latest),
            b.features.clone(),
        ),
    ];
    let stderr = match probe(&latest_deps, &format!("sweep-latest-{i}-{j}")) {
        // Resolving is where the resolver-only sweep stops. With --compile we
        // keep going, because "cargo picked versions" and "rustc accepts them"
        // are different questions (canary c22/c24).
        Ok(ProbeResult::Resolves(_)) => {
            return compile.then(|| compile_hit(a, b, i, j)).flatten();
        }
        Ok(ProbeResult::Conflicts(stderr)) => stderr,
        Err(err) => format!("probe error: {err:#}"),
    };
    let excerpt = stderr
        .lines()
        .filter(|l| l.contains("failed to select") || l.contains("required by"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();

    // Classify: can cargo escape by backtracking either side?
    let star_deps = vec![
        (a.krate.to_owned(), "*".to_owned(), a.features.clone()),
        (b.krate.to_owned(), "*".to_owned(), b.features.clone()),
    ];
    let (kind, escape) = match probe(&star_deps, &format!("sweep-star-{i}-{j}")) {
        Ok(ProbeResult::Resolves(locked)) => {
            let escape = [a.krate, b.krate]
                .iter()
                .filter_map(|name| locked.get(*name).map(|v| format!("{name} {v}")))
                .collect();
            (HitKind::LatestOnly, Some(escape))
        }
        _ => (HitKind::Hard, None),
    };

    Some(SweepHit {
        pair: [a.krate.to_owned(), b.krate.to_owned()],
        latest: [a.latest.clone(), b.latest.clone()],
        kind,
        known: known_conflict_id(a.krate, b.krate),
        escape,
        excerpt,
    })
}

fn print_report(report: &SweepReport, new_candidates: usize) {
    let compiled = report.depth == "resolve+compile";
    println!(
        "\n{} pairs probed · {} clean ({}) · {} conflict",
        report.probed,
        report.clean,
        if compiled {
            "resolved AND compiled"
        } else {
            "resolved — NOT compile-checked"
        },
        report.hits.len()
    );
    if !compiled {
        // The number above is the one people quote. Left bare it says "the
        // ecosystem is fine" when it only says "cargo could pick versions".
        println!(
            "  note: resolver-only. A pair can resolve and still fail to build — \
             on 2026-07-27 this line read 66/66 clean while 21 of those pairs did not \
             compile. Re-run with --compile to check."
        );
    }
    for hit in &report.hits {
        let badge = match (&hit.known, hit.kind) {
            (Some(id), _) => format!("✓ known [{id}]"),
            (None, HitKind::Hard) => "🔥 NEW · HARD".into(),
            (None, HitKind::LatestOnly) => "🔥 NEW · latest-only".into(),
            (None, HitKind::CompileOnly) => "🔥 NEW · RESOLVES BUT DOES NOT COMPILE".into(),
        };
        println!(
            "\n{badge}  {} {} × {} {}",
            hit.pair[0], hit.latest[0], hit.pair[1], hit.latest[1]
        );
        if let Some(escape) = &hit.escape {
            println!("           escape: {}", escape.join(" + "));
        }
        for line in hit.excerpt.lines() {
            println!("           {}", line.trim());
        }
    }
    if new_candidates > 0 {
        println!(
            "\n{new_candidates} NEW conflict candidate(s) — reproduce, then record in facts/facts.toml with a probe"
        );
    }
}

/// The pair resolved — does it survive rustc? Only reached under `--compile`.
fn compile_hit(a: &Subject, b: &Subject, i: usize, j: usize) -> Option<SweepHit> {
    let deps = vec![
        (
            a.krate.to_owned(),
            format!("={}", a.latest),
            a.features.clone(),
        ),
        (
            b.krate.to_owned(),
            format!("={}", b.latest),
            b.features.clone(),
        ),
    ];
    let stderr = match compile_probe(&deps, &format!("sweep-check-{i}-{j}")) {
        Ok(CompileProbeResult::Compiles) => return None,
        Ok(CompileProbeResult::FailsToCompile(stderr)) => stderr,
        // It resolved a moment ago; if it does not now, the index moved under
        // us. Not a compile finding — say nothing rather than mislabel it.
        Ok(CompileProbeResult::FailsToResolve(_)) => return None,
        Err(err) => format!("compile probe error: {err:#}"),
    };
    let excerpt = stderr
        .lines()
        .filter(|l| l.trim_start().starts_with("error"))
        .take(3)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    Some(SweepHit {
        pair: [a.krate.to_owned(), b.krate.to_owned()],
        latest: [a.latest.clone(), b.latest.clone()],
        kind: HitKind::CompileOnly,
        known: known_compile_conflict_id(a.krate, b.krate),
        escape: None,
        excerpt,
    })
}

/// Compile conflicts are frequently single-crate — `litesvm@0.15` alone drags
/// both wincode majors in — so EITHER side matching is the right rule here.
/// Requiring both, as the resolve-level matcher does, would report every pair
/// containing litesvm as a fresh discovery.
fn known_compile_conflict_id(a: &str, b: &str) -> Option<String> {
    facts::compile_conflicts()
        .iter()
        .find(|conflict| {
            let names: BTreeSet<String> = conflict
                .probe
                .iter()
                .filter_map(|spec| facts::parse_probe(spec))
                .map(|(name, _, _)| name)
                .collect();
            names.contains(a) || names.contains(b)
        })
        .map(|conflict| conflict.id.clone())
}

/// Match a failing pair against recorded conflicts by probe crate-name overlap.
fn known_conflict_id(a: &str, b: &str) -> Option<String> {
    facts::conflicts()
        .iter()
        .find(|conflict| {
            let names: BTreeSet<String> = conflict
                .probe
                .iter()
                .filter_map(|spec| facts::parse_probe(spec))
                .map(|(name, _, _)| name)
                .collect();
            names.contains(a) && names.contains(b)
        })
        .map(|conflict| conflict.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A compile conflict whose probe names one crate must still be recognised
    /// when that crate shows up paired with anything else — otherwise every pair
    /// containing litesvm reports as a brand-new discovery.
    #[test]
    fn compile_conflicts_match_on_either_side() {
        assert_eq!(
            known_compile_conflict_id("litesvm", "light-sdk").as_deref(),
            Some("wincode-schema-split")
        );
        assert_eq!(
            known_compile_conflict_id("anchor-lang", "litesvm").as_deref(),
            Some("wincode-schema-split")
        );
        assert!(known_compile_conflict_id("anchor-lang", "mpl-core").is_none());
        // The first --compile sweep reported 10 mollusk pairs as NEW because the
        // only compile entry then named litesvm. Same split, second entry point.
        assert_eq!(
            known_compile_conflict_id("anchor-lang", "mollusk-svm").as_deref(),
            Some("wincode-schema-split-mollusk")
        );
    }

    /// The resolve-level matcher keeps the stricter rule: a pair is only "known"
    /// when the entry names BOTH crates.
    #[test]
    fn resolve_conflicts_still_require_both_sides() {
        assert_eq!(
            known_conflict_id("litesvm", "light-sdk").as_deref(),
            Some("litesvm-light")
        );
        assert!(known_conflict_id("litesvm", "mpl-core").is_none());
    }
}
