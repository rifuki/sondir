use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Ok,
    Info,
    Warn,
    Fail,
}

impl Severity {
    fn glyph(self) -> &'static str {
        match self {
            Severity::Ok => "\x1b[32m✓\x1b[0m",
            Severity::Info => "\x1b[36mi\x1b[0m",
            Severity::Warn => "\x1b[33m⚠\x1b[0m",
            Severity::Fail => "\x1b[31m✗\x1b[0m",
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Finding {
    pub severity: Severity,
    /// Stable machine-readable code, e.g. `simd0431-extend`.
    pub code: &'static str,
    pub title: String,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn add(
        &mut self,
        severity: Severity,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
        fix: Option<String>,
    ) {
        self.findings.push(Finding {
            severity,
            code,
            title: title.into(),
            detail: detail.into(),
            fix,
        });
    }

    pub fn ok(&mut self, code: &'static str, title: impl Into<String>, detail: impl Into<String>) {
        self.add(Severity::Ok, code, title, detail, None);
    }

    pub fn info(
        &mut self,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
    ) {
        self.add(Severity::Info, code, title, detail, None);
    }

    pub fn warn(
        &mut self,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
        fix: Option<String>,
    ) {
        self.add(Severity::Warn, code, title, detail, fix);
    }

    pub fn fail(
        &mut self,
        code: &'static str,
        title: impl Into<String>,
        detail: impl Into<String>,
        fix: Option<String>,
    ) {
        self.add(Severity::Fail, code, title, detail, fix);
    }

    /// Render to stdout; returns the process exit code (1 when any FAIL present).
    pub fn print(&self, json: bool) -> anyhow::Result<i32> {
        if json {
            println!("{}", serde_json::to_string_pretty(self)?);
        } else {
            for f in &self.findings {
                println!("{} [{}] {}", f.severity.glyph(), f.code, f.title);
                for line in f.detail.lines() {
                    println!("      {line}");
                }
                if let Some(fix) = &f.fix {
                    for line in fix.lines() {
                        println!("      \x1b[1mfix:\x1b[0m {line}");
                    }
                }
            }
            let fails = self.count(Severity::Fail);
            let warns = self.count(Severity::Warn);
            println!(
                "\n{} fail · {} warn · {} findings",
                fails,
                warns,
                self.findings.len()
            );
        }
        Ok(i32::from(self.count(Severity::Fail) > 0))
    }

    fn count(&self, severity: Severity) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_code_is_one_only_when_a_fail_exists() {
        let mut report = Report::default();
        report.warn("w", "warn", "detail", None);
        assert_eq!(report.count(Severity::Fail), 0);
        report.fail("f", "fail", "detail", None);
        assert_eq!(report.count(Severity::Fail), 1);
    }

    #[test]
    fn findings_keep_insertion_order() {
        let mut report = Report::default();
        report.ok("a", "first", "");
        report.fail("b", "second", "", None);
        assert_eq!(report.findings[0].code, "a");
        assert_eq!(report.findings[1].code, "b");
    }
}
