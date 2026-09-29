//! The record of the environment a benchmark run was made in.
//!
//! Responsibility: write `meta.json` beside the criterion output, naming the CPU
//! model, the frequency-scaling governor and the compile flags the measured binaries
//! were built with. A latency number without them cannot be compared with the number
//! a later run produces: a governor that has switched to a lower frequency moves
//! every case by the same factor, and the difference reads as a regression.
//!
//! The record is written by the gate that reads the numbers rather than by each
//! benchmark binary, so that the resolution of the output directory and the shape of
//! the document live in one place instead of in every criterion target.
//!
//! The three values are read from `/proc`, `/sys` and the environment, and none of
//! them is required: a container has no cpufreq interface and a build without
//! `RUSTFLAGS` has no flags, so a missing value is recorded as `null` rather than
//! failing the gate.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::json;

/// The file the run's environment is recorded in, inside the criterion directory.
pub(super) const META_FILE: &str = "meta.json";

/// The file the CPU model is read from.
const CPUINFO: &str = "/proc/cpuinfo";

/// The file the frequency-scaling governor of the first processor is read from.
const GOVERNOR: &str = "/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor";

/// The environment a benchmark run was made in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    /// `model name` of the first processor, or `None` where the platform has none.
    pub cpu_model: Option<String>,
    /// Frequency-scaling governor of the first processor, or `None` where the
    /// platform has no cpufreq interface.
    pub scaling_governor: Option<String>,
    /// The flags the measured binaries were compiled with.
    pub rustflags: Option<String>,
}

impl RunMeta {
    /// Reads the environment this process runs in.
    pub(super) fn capture() -> Self {
        Self {
            cpu_model: cpu_model_from(&fs::read_to_string(CPUINFO).unwrap_or_default()),
            scaling_governor: governor_from(&fs::read_to_string(GOVERNOR).unwrap_or_default()),
            rustflags: rustflags(),
        }
    }

    /// Renders the record as the JSON document written to disk.
    pub(super) fn to_json(&self) -> String {
        let document = json!({
            "cpu_model": self.cpu_model,
            "scaling_governor": self.scaling_governor,
            "rustflags": self.rustflags,
        });
        format!("{document:#}\n")
    }
}

/// Writes the run's environment into `dir`, returning the path it wrote.
///
/// # Errors
/// Returns an error when the directory cannot be created or the file cannot be
/// written.
pub(super) fn write(dir: &Path) -> Result<PathBuf> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    let path = dir.join(META_FILE);
    fs::write(&path, RunMeta::capture().to_json())
        .with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
}

/// The CPU model named by the `model name` line of a `/proc/cpuinfo` dump.
pub(super) fn cpu_model_from(cpuinfo: &str) -> Option<String> {
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "model name" {
            let model = value.trim();
            if !model.is_empty() {
                return Some(model.to_owned());
            }
        }
    }
    None
}

/// The governor named by a `scaling_governor` file's contents.
pub(super) fn governor_from(text: &str) -> Option<String> {
    let governor = text.trim();
    (!governor.is_empty()).then(|| governor.to_owned())
}

/// The flags the measured binaries were compiled with.
///
/// Cargo does not pass `RUSTFLAGS` on to the programs it runs, so the value seen
/// here is the one the shell that started the gate exported -- which is also the one
/// cargo compiled with, because both commands come from the same shell. The
/// compile-time value is the fallback for a gate started some other way.
fn rustflags() -> Option<String> {
    std::env::var("RUSTFLAGS")
        .ok()
        .or_else(|| option_env!("RUSTFLAGS").map(str::to_owned))
        .filter(|flags| !flags.trim().is_empty())
}
