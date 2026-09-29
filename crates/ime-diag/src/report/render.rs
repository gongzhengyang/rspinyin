//! Rendering a report: the text a person reads and the document CI reads.
//!
//! Responsibility: two views of one [`ProbeReport`] -- a column-per-percentile table
//! with the verdict beside it, and a JSON document with the same numbers keyed for a
//! machine. Both are pure functions of the report, so the two cannot disagree about
//! what was measured.
//!
//! # Why the JSON is written here
//!
//! The diagnostics crate carries no serialization dependency, and this document is
//! the reason that is affordable: its keys are the contract's own identifiers, its
//! values are whole numbers, and nothing in it can hold a string a user typed. A
//! general-purpose serializer would be a dependency taken on to write one fixed
//! shape of numbers; the escaping below is the whole of what the format requires of
//! us, and a test asserts the result parses.
//!
//! # Units
//!
//! Latencies are microseconds in the document and are printed in the unit the metric
//! is stated in -- milliseconds for everything but the wakeup latency, whose budget
//! is 50µs and which would read as `0.05ms`. Both renderings are plain ASCII,
//! including the microsecond suffix, so a report can be pasted anywhere.

use std::time::Duration;

use crate::probe::{COUNTER_COUNT, Counter, HistSnapshot, Percentile};

use super::{BudgetCheck, ProbeReport, Row, Verdict};

/// The horizontal rule between the sections of the text report.
const RULE: &str = "---------------------------------------------------------------------------\n";

/// The column the wrapped counter list continues under.
const COUNTERS_INDENT: &str = "          ";

/// The column at which the counter list wraps.
const COUNTERS_WIDTH: usize = 96;

impl ProbeReport {
    /// Renders the report as text for a person to read.
    ///
    /// The table carries every percentile and the budget column carries every
    /// threshold declared for the row, so a reader can see both what was measured and
    /// what it was measured against without opening the budget document.
    pub fn render_text(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "rspinyin budget report (sampled {}, {} sessions, {} keys)\n",
            format_span(self.sampled),
            self.sessions,
            self.keys
        ));
        out.push_str(RULE);
        out.push_str(&format!(
            "{:<20}{:>10}{:>10}{:>10}{:>10}   {:<30}{}\n",
            "metric", "P50", "P90", "P99", "P999", "budget", "verdict"
        ));
        for row in &self.rows {
            out.push_str(&row_text(row));
        }
        out.push_str(RULE);
        out.push_str(&counters_text(&self.counters));
        for note in &self.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out.push_str(&verdict_text(self));
        out
    }

    /// Renders the report as a JSON document.
    ///
    /// The document is what a CI job compares between runs: the numbers are whole
    /// microseconds and the verdicts are strings, so a regression is a changed value
    /// rather than a changed sentence.
    pub fn render_json(&self) -> String {
        let (verdict, failed) = match self.verdict() {
            Verdict::Fail(percentile) => ("FAIL", json_string(percentile.label())),
            other => (other.label(), "null".to_owned()),
        };
        let mut out = String::from("{\n");
        out.push_str(&format!(
            "  \"sampled_secs\": {:.3},\n",
            self.sampled.as_secs_f64()
        ));
        out.push_str(&format!("  \"sessions\": {},\n", self.sessions));
        out.push_str(&format!("  \"keys\": {},\n", self.keys));
        out.push_str(&format!("  \"verdict\": {},\n", json_string(verdict)));
        out.push_str(&format!("  \"failed_percentile\": {failed},\n"));
        out.push_str(&format!(
            "  \"notes\": [{}],\n",
            json_list(self.notes.iter().map(|note| json_string(note.as_str())))
        ));
        out.push_str("  \"metrics\": [\n");
        let last = self.rows.len().saturating_sub(1);
        for (index, row) in self.rows.iter().enumerate() {
            out.push_str(&row_json(row, index == last));
        }
        out.push_str("  ],\n");
        out.push_str("  \"counters\": {\n");
        let last = Counter::ALL.len().saturating_sub(1);
        for (index, counter) in Counter::ALL.into_iter().enumerate() {
            let comma = if index == last { "" } else { "," };
            out.push_str(&format!(
                "    {}: {}{comma}\n",
                json_string(counter.name()),
                self.counter(counter)
            ));
        }
        out.push_str("  },\n");
        out.push_str(&format!(
            "  \"violations\": [{}]\n",
            json_list(
                self.violations()
                    .iter()
                    .map(|item| json_string(item.as_str()))
            )
        ));
        out.push_str("}\n");
        out
    }
}

/// One row of the text table.
fn row_text(row: &Row) -> String {
    let unit = row.metric.unit();
    let budgets = if row.checks.is_empty() {
        "-".to_owned()
    } else {
        row.checks
            .iter()
            .map(|check| {
                format!(
                    "{}({})",
                    unit.format(check.limit_us),
                    check.percentile.label()
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut out = format!("{:<20}", row.metric.name());
    for percentile in Percentile::ALL {
        let measured = unit.format(row.sample.percentile(percentile) as f64);
        out.push_str(&format!("{measured:>10}"));
    }
    out.push_str(&format!("   {budgets:<30}{}\n", row.verdict.detail()));
    out
}

/// The counter section of the text report, wrapped to a readable width.
fn counters_text(counters: &[u64; COUNTER_COUNT]) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::from("counters: ");
    for counter in Counter::ALL {
        let entry = format!("{}={}", counter.name(), counters[counter.index()]);
        if current.len() > COUNTERS_INDENT.len() && current.len() + entry.len() + 2 > COUNTERS_WIDTH
        {
            lines.push(current);
            current = String::from(COUNTERS_INDENT);
        }
        if current.len() > COUNTERS_INDENT.len() {
            current.push_str("  ");
        }
        current.push_str(&entry);
    }
    lines.push(current);
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// The closing line of the text report.
fn verdict_text(report: &ProbeReport) -> String {
    match report.verdict() {
        Verdict::Unbudgeted => "verdict: no threshold is declared for any metric\n".to_owned(),
        Verdict::Pass => {
            let judged = report
                .rows
                .iter()
                .filter(|row| row.verdict == Verdict::Pass)
                .count();
            format!("verdict: {judged} metrics within budget\n")
        }
        Verdict::Fail(_) => {
            let violations = report.violations();
            let judged: usize = report.rows.iter().map(|row| row.checks.len()).sum();
            format!(
                "verdict: {} of {judged} thresholds missed ({})\n",
                violations.len(),
                violations.join("; ")
            )
        }
    }
}

/// One metric of the JSON document.
fn row_json(row: &Row, last: bool) -> String {
    let mut out = String::from("    {\n");
    out.push_str(&format!(
        "      \"name\": {},\n",
        json_string(row.metric.name())
    ));
    out.push_str(&format!(
        "      \"unit\": {},\n",
        json_string(row.metric.unit().suffix())
    ));
    out.push_str(&format!("      \"count\": {},\n", row.sample.count));
    out.push_str(&format!("      \"sum_us\": {},\n", row.sample.sum_us));
    for percentile in Percentile::ALL {
        out.push_str(&format!(
            "      \"{}_us\": {},\n",
            percentile.label().to_lowercase(),
            row.sample.percentile(percentile)
        ));
    }
    out.push_str(&format!("      \"max_us\": {},\n", row.sample.max_us));
    out.push_str(&format!(
        "      \"mean_us\": {:.3},\n",
        row.sample.mean_us()
    ));
    out.push_str(&format!(
        "      \"verdict\": {},\n",
        json_string(row.verdict.label())
    ));
    out.push_str(&format!(
        "      \"budgets\": [{}]\n",
        json_list(
            row.checks
                .iter()
                .map(|check| check_json(check, &row.sample))
        )
    ));
    out.push_str(if last { "    }\n" } else { "    },\n" });
    out
}

/// One threshold of the JSON document.
fn check_json(check: &BudgetCheck, sample: &HistSnapshot) -> String {
    format!(
        "{{\"key\": {}, \"percentile\": {}, \"limit_us\": {:.3}, \"verdict\": {}}}",
        json_string(check.key),
        json_string(check.percentile.label()),
        check.limit_us,
        json_string(check.verdict(sample).label())
    )
}

/// Joins JSON fragments with the separator the grammar requires.
fn json_list(fragments: impl Iterator<Item = String>) -> String {
    fragments.collect::<Vec<_>>().join(", ")
}

/// Renders `value` as a JSON string literal.
///
/// Every string this module writes comes from the contract's own identifier tables,
/// so no escape is reachable today. It is written anyway: a serializer that is only
/// correct for the input it currently sees is a trap for the day a key gains a
/// character, and the escaping is a dozen lines.
pub(super) fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// A sampling window as `1h02m03s`, `5m12s` or `42s`.
fn format_span(span: Duration) -> String {
    let total = span.as_secs();
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours}h{minutes:02}m{seconds:02}s")
    } else if minutes > 0 {
        format!("{minutes}m{seconds:02}s")
    } else {
        format!("{seconds}s")
    }
}
