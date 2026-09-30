//! The budget document: the schema of `docs/dev/budgets.json` and the parser that
//! enforces it.
//!
//! Responsibility: turn the document's text into [`Budgets`] and refuse anything the
//! schema does not name. An unknown key is an error rather than an ignored value, so a
//! typo cannot silently disable a threshold, and a value that is not a finite number of
//! the expected shape is rejected where it is read.
//!
//! Boundaries: this layer reads the document and nothing else. It never opens the spec
//! and never compares the two; the numbers it produces are checked against the spec
//! table by [`super::spec`].

use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value};

use super::*;

/// Top-level keys that are metadata rather than thresholds.
const META_KEYS: &[&str] = &["version", "source", "notes"];

impl Budgets {
    /// Parse and schema-check a `budgets.json` document.
    ///
    /// # Errors
    /// Returns an error when the text is not valid JSON, when `version` is
    /// unsupported, when a required key is missing, when a value is not a finite
    /// number of the expected shape, or when an unknown key is present. Unknown
    /// keys are rejected rather than ignored so a typo cannot silently disable a
    /// threshold.
    pub fn from_json(text: &str) -> Result<Self> {
        let parsed: Value = serde_json::from_str(text).context("document is not valid JSON")?;
        let root = parsed
            .as_object()
            .context("top level must be a JSON object")?;

        let version = root
            .get("version")
            .context("top level: missing key `version`")?
            .as_u64()
            .context("top level: `version` must be a non-negative integer")?;
        ensure!(
            version == 1,
            "top level: unsupported `version` {version}, expected 1"
        );
        root.get("source")
            .context("top level: missing key `source`")?
            .as_str()
            .context("top level: `source` must name the authoritative table")?;
        if let Some(notes) = root.get("notes") {
            for note in notes
                .as_array()
                .context("top level: `notes` must be strings")?
            {
                note.as_str()
                    .context("top level: `notes` must be strings")?;
            }
        }

        let latency = section(root, "latency_ms")?;
        let memory = section(root, "memory_mb")?;
        let cpu = section(root, "cpu_pct")?;
        let size = section(root, "size_mb")?;
        let robust = section(root, "robustness")?;
        let bench = section(root, "bench")?;
        let alloc = section(root, "alloc_count")?;

        let budgets = Self {
            version,
            latency_ms: LatencyMs {
                key_to_present_p50: positive(latency, "latency_ms", "key_to_present_p50")?,
                key_to_present_p99: positive(latency, "latency_ms", "key_to_present_p99")?,
                key_to_present_p99_144hz: positive(
                    latency,
                    "latency_ms",
                    "key_to_present_p99_144hz",
                )?,
                decode_p99: positive(latency, "latency_ms", "decode_p99")?,
                decode_p999: positive(latency, "latency_ms", "decode_p999")?,
                raster_p99: positive(latency, "latency_ms", "raster_p99")?,
                first_key_to_visible_p99: positive(
                    latency,
                    "latency_ms",
                    "first_key_to_visible_p99",
                )?,
                addon_load: positive(latency, "latency_ms", "addon_load")?,
            },
            memory_mb: MemoryMb {
                ui_rss: positive(memory, "memory_mb", "ui_rss")?,
                plugin_rss: positive(memory, "memory_mb", "plugin_rss")?,
                dict_mmap_rss: positive(memory, "memory_mb", "dict_mmap_rss")?,
            },
            cpu_pct: CpuPct {
                idle: positive(cpu, "cpu_pct", "idle")?,
                typing_10cps: positive(cpu, "cpu_pct", "typing_10cps")?,
                idle_redraw_count: count(cpu, "cpu_pct", "idle_redraw_count")?,
                idle_poll_timer_count: count(cpu, "cpu_pct", "idle_poll_timer_count")?,
            },
            size_mb: SizeMb {
                so_stripped: positive(size, "size_mb", "so_stripped")?,
                base_dict: positive(size, "size_mb", "base_dict")?,
            },
            robustness: Robustness {
                soak_hours: positive(robust, "robustness", "soak_hours")?,
                rss_drift_mb: positive(robust, "robustness", "rss_drift_mb")?,
                pass_rate_pct: positive(robust, "robustness", "pass_rate_pct")?,
            },
            bench: Bench {
                passthrough_classify_ns: positive(bench, "bench", "passthrough_classify_ns")?,
                buffer_ops_us: positive(bench, "bench", "input_buffer_ops_us")?,
                decode_holdout_s: positive(bench, "bench", "decode_holdout_s")?,
                ui_wakeup_latency_us: positive(bench, "bench", "ui_wakeup_latency_us")?,
            },
            alloc_count: AllocCount {
                decode_steady: count(alloc, "alloc_count", "decode_steady")?,
            },
            net_sockets: count(root, "top level", "net_sockets")?,
        };
        budgets.reject_unknown_paths(&parsed)?;
        Ok(budgets)
    }

    /// Flatten the document into the numeric thresholds it carries.
    ///
    /// The keys are the dotted paths used by `BINDINGS`, which is what lets the
    /// spec comparison run without a second copy of the numbers anywhere.
    pub fn thresholds(&self) -> Vec<Threshold> {
        let (l, m, c, s, r, b, a) = (
            &self.latency_ms,
            &self.memory_mb,
            &self.cpu_pct,
            &self.size_mb,
            &self.robustness,
            &self.bench,
            &self.alloc_count,
        );
        [
            Threshold("latency_ms.key_to_present_p50", l.key_to_present_p50),
            Threshold("latency_ms.key_to_present_p99", l.key_to_present_p99),
            Threshold(
                "latency_ms.key_to_present_p99_144hz",
                l.key_to_present_p99_144hz,
            ),
            Threshold("latency_ms.decode_p99", l.decode_p99),
            Threshold("latency_ms.decode_p999", l.decode_p999),
            Threshold("latency_ms.raster_p99", l.raster_p99),
            Threshold(
                "latency_ms.first_key_to_visible_p99",
                l.first_key_to_visible_p99,
            ),
            Threshold("latency_ms.addon_load", l.addon_load),
            Threshold("memory_mb.ui_rss", m.ui_rss),
            Threshold("memory_mb.plugin_rss", m.plugin_rss),
            Threshold("memory_mb.dict_mmap_rss", m.dict_mmap_rss),
            Threshold("cpu_pct.idle", c.idle),
            Threshold("cpu_pct.typing_10cps", c.typing_10cps),
            Threshold("cpu_pct.idle_redraw_count", c.idle_redraw_count as f64),
            Threshold(
                "cpu_pct.idle_poll_timer_count",
                c.idle_poll_timer_count as f64,
            ),
            Threshold("size_mb.so_stripped", s.so_stripped),
            Threshold("size_mb.base_dict", s.base_dict),
            Threshold("robustness.soak_hours", r.soak_hours),
            Threshold("robustness.rss_drift_mb", r.rss_drift_mb),
            Threshold("robustness.pass_rate_pct", r.pass_rate_pct),
            Threshold("bench.passthrough_classify_ns", b.passthrough_classify_ns),
            Threshold("bench.input_buffer_ops_us", b.buffer_ops_us),
            Threshold("bench.decode_holdout_s", b.decode_holdout_s),
            Threshold("bench.ui_wakeup_latency_us", b.ui_wakeup_latency_us),
            Threshold("alloc_count.decode_steady", a.decode_steady as f64),
            Threshold("net_sockets", self.net_sockets as f64),
        ]
        .to_vec()
    }

    /// Reject every value that is neither metadata nor a known threshold.
    fn reject_unknown_paths(&self, parsed: &Value) -> Result<()> {
        let mut known: Vec<String> = META_KEYS.iter().map(|key| (*key).to_owned()).collect();
        known.extend(self.thresholds().iter().map(|t| t.0.to_owned()));
        walk(parsed, "", &known)
    }
}

/// Reject every leaf of `value` whose dotted path is not in `known`.
fn walk(value: &Value, path: &str, known: &[String]) -> Result<()> {
    let Some(map) = value.as_object() else {
        ensure!(
            known.iter().any(|k| k == path),
            "unknown key `{path}` in {BUDGETS_FILE} (known keys: {})",
            known.join(", ")
        );
        return Ok(());
    };
    for (key, child) in map {
        let child_path = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        walk(child, &child_path, known)?;
    }
    Ok(())
}

/// Fetch a nested object.
fn section<'a>(root: &'a Map<String, Value>, name: &str) -> Result<&'a Map<String, Value>> {
    root.get(name)
        .with_context(|| format!("top level: missing section `{name}`"))?
        .as_object()
        .with_context(|| format!("top level: `{name}` must be an object"))
}

/// Fetch a strictly positive finite number.
fn positive(object: &Map<String, Value>, where_: &str, key: &str) -> Result<f64> {
    let value = object
        .get(key)
        .with_context(|| format!("{where_}: missing key `{key}`"))?
        .as_f64()
        .with_context(|| format!("{where_}.{key}: must be a number"))?;
    ensure!(
        value.is_finite() && value > 0.0,
        "{where_}.{key}: must be a finite number greater than zero, found {value}"
    );
    Ok(value)
}

/// Fetch a non-negative integer count.
fn count(object: &Map<String, Value>, where_: &str, key: &str) -> Result<u64> {
    object
        .get(key)
        .with_context(|| format!("{where_}: missing key `{key}`"))?
        .as_u64()
        .with_context(|| format!("{where_}.{key}: must be a non-negative integer"))
}
