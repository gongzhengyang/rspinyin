//! The decision table: what this gate answers for every requirement a case may declare.
//!
//! Responsibility: hold the table, answer a declared requirement by applying the machine's own
//! report to the row that declares it, and give the remedy that row carries. Boundaries: this
//! file knows the requirements, the verdicts, the remedies and the capabilities a report can
//! settle; it reads no document, opens no socket and looks at no source tree.
//!
//! # A row carries a remedy as well as a verdict
//!
//! A withheld case that says what is missing without saying what to do about it leaves a reader
//! to guess whether the machine needs a package or the case needs code, and a guess is how a
//! reader ends up installing a compositor to clear a row that no compositor can clear. Every row
//! therefore carries the one line that settles it, and the rows whose code does not exist yet
//! carry the one that says so.
//!
//! # The verdicts are rules, not constants
//!
//! The requirements are the platform's decision table's, and what each row answers is a rule
//! over the report: `features.md` 0.5.5's registered answers are what these rules produce for
//! the development machine, which is what [`super::cross_check_spec`] holds them to. A gate
//! that hardcoded "X11 is runnable" would clear an X11 case on a headless CI runner, and a
//! cleared case is a case whose pass is believed.
//!
//! # What no report can settle
//!
//! Three rows are withheld wherever they run rather than on a particular machine: the probe
//! reads no compositor blur capability, no monitor layout, and no eight-hour run, so there is
//! nothing in the report that could clear them. Withholding them is the fail-closed answer, and
//! it is not a claim about any one machine.

use super::{EnvRequirement, Executability};
use crate::testd::env::{DisplayServer, EnvCapabilities, ProbeGap, WaylandTier};

/// What an X11 case needs when no X server answered.
const X11_NEEDS: &str = "an X server reachable at $DISPLAY";

/// What a Fcitx5 case needs when the development package is absent.
const FCITX5_NEEDS: &str = "the Fcitx5 development package and a session that loads both addons";

/// What a Wayland-tier case needs, in `features.md` 0.5.5's own words.
const WAYLAND_TIER_NEEDS: &str = "真实 Sway/Hyprland/KWin/GNOME 会话";

/// What a compositor-blur case needs, in `features.md` 0.5.5's own words.
const BLUR_NEEDS: &str = "支持应用侧模糊的合成器";

/// What a multi-monitor case needs, in `features.md` 0.5.5's own words.
const MULTI_MONITOR_NEEDS: &str = "真实多显示器环境";

/// What an eight-hour run needs, in `features.md` 0.5.5's own words.
const LONG_RUN_NEEDS: &str = "裸机 Linux，8 小时独占";

/// What a case that declares no requirement is told it needs.
const UNDECLARED_NEEDS: &str = "<未声明>";

/// Why the report cannot settle a compositor-blur case.
const BLUR_REASON: &str = "the probe reads no compositor blur capability, so nothing settles it";

/// Why the report cannot settle a multi-monitor case.
const MONITORS_REASON: &str = "the probe reads no monitor layout, so nothing here settles it";

/// Why the report cannot settle an eight-hour run.
const LONG_RUN_REASON: &str = "an eight-hour exclusive run is not something a probe can establish";

/// Why a candidate-window case is blocked.
const WINDOW_REASON: &str = "the window's drawing does not exist yet, so nothing can exercise it";

/// Why a configuration-reload case is blocked.
const CONFIG_REASON: &str = "the configuration layer is an empty shell, so nothing can reload";

/// Why a log-redaction case is blocked.
const LOG_REASON: &str = "the diagnostics layer is an empty shell, so nothing can redact";

/// Why a case whose requirement the table does not declare is withheld.
const UNDECLARED_REASON: &str = "unknown requirement";

/// What a machine with no X server has to do before an X11 case can run.
const X11_REMEDY: &str = "start Xorg, Xvfb or Xephyr and point $DISPLAY at it";

/// What a machine without the Fcitx5 development package has to install.
const FCITX5_REMEDY: &str = "install `libfcitx5core-dev` (Debian) or `fcitx5-devel` (Fedora)";

/// Which session a machine outside the four tiers has to run the case on instead.
const WAYLAND_TIER_REMEDY: &str = "run the case on Sway/Hyprland/KWin/Mutter, or a nested one";

/// Which compositor a machine with no blur capability has to run the case under instead.
const BLUR_REMEDY: &str = "run the case under KWin or Hyprland, or beside picom";

/// What a machine with one monitor has to run the case on instead.
const MULTI_MONITOR_REMEDY: &str = "run the case on two monitors at different scales";

/// What an eight-hour run has to be held on.
const LONG_RUN_REMEDY: &str = "run the soak on bare metal, with the machine to itself";

/// Why no machine supplies what a candidate-window case waits on.
const WINDOW_REMEDY: &str = "no environment supplies it: it runs when the drawing lands";

/// Why no machine supplies what a configuration-reload case waits on.
const CONFIG_REMEDY: &str = "no environment supplies it: it runs when the layer can reload";

/// Why no machine supplies what a log-redaction case waits on.
const LOG_REMEDY: &str = "no environment supplies it: it runs when the layer can redact";

/// What a case that declares no requirement is told to do about it.
const UNDECLARED_REMEDY: &str = "declare the case's 环境需求 field in its attribute block";

/// The rows of the decision table that `features.md` 0.5.5 does not register as unverifiable.
const UNREGISTERED: &[&str] = &[];

/// The verdict the decision table reaches for `requirement` on `env`.
///
/// A requirement no row declares -- including one a case never declared at all -- is withheld,
/// which is the fail-closed answer rather than a `Runnable` the table cannot justify.
///
/// # Panics
///
/// Never.
pub(super) fn verdict_for(requirement: &EnvRequirement, env: &EnvCapabilities) -> Executability {
    if let EnvRequirement::Undeclared(_) = requirement {
        return undeclared();
    }
    DECISION_TABLE
        .iter()
        .find(|row| row.requirement == *requirement)
        .map_or_else(undeclared, |row| row.answer(env))
}

/// The remedy the decision table gives for `requirement`.
///
/// Every requirement has one, because a withheld case that says what is wrong without saying
/// what to do about it leaves a reader to guess: the two cases are the same to a reader until
/// they know whether to install a package or to wait for code.
///
/// A requirement no row declares -- including one a case never declared at all -- is told to
/// declare itself, which is the only thing a reader of that verdict can act on.
///
/// # Panics
///
/// Never.
pub(super) fn remedy_for(requirement: &EnvRequirement) -> &'static str {
    DECISION_TABLE
        .iter()
        .find(|row| row.requirement == *requirement)
        .map_or(UNDECLARED_REMEDY, |row| row.remedy)
}

/// The requirement a row of the decision table answers, and how the answer is reached.
pub(super) struct DecisionRow {
    /// The requirement the row answers.
    pub(super) requirement: EnvRequirement,
    /// How the answer is reached.
    pub(super) verdict: RowVerdict,
    /// The `features.md` 0.5.5 rows that register this requirement's item unverifiable, empty
    /// for a row 0.5.5 does not speak about.
    pub(super) register: &'static [&'static str],
    /// What would let a case that declares this requirement run, in one line.
    pub(super) remedy: &'static str,
}

impl DecisionRow {
    /// The verdict this row reaches on `env`.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn answer(&self, env: &EnvCapabilities) -> Executability {
        match self.verdict {
            RowVerdict::ByReport { capability, needs } => match capability.withholding(env) {
                Some(reason) => unverifiable(needs, &reason),
                None => Executability::Runnable,
            },
            RowVerdict::Withheld { needs, reason } => unverifiable(needs, reason),
            RowVerdict::Blocked { tasks, reason } => Executability::Blocked {
                task: tasks.join(", "),
                reason: String::from(reason),
            },
        }
    }

    /// The environment the row names, for a machine that cannot serve it.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn needs(&self) -> &'static str {
        match self.verdict {
            RowVerdict::ByReport { needs, .. } | RowVerdict::Withheld { needs, .. } => needs,
            RowVerdict::Blocked { .. } => "",
        }
    }

    /// Whether the row cites `task` as one `features.md` 0.5.5 registers for it.
    ///
    /// # Panics
    ///
    /// Never.
    pub(super) fn cites(&self, task: &str) -> bool {
        self.register.contains(&task)
    }
}

/// How one row of the decision table is answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowVerdict {
    /// The report settles it: the row is cleared when the machine shows the capability.
    ByReport {
        /// The capability the machine has to show.
        capability: Capability,
        /// What the environment would have to be when it does not.
        needs: &'static str,
    },
    /// The report cannot settle it at all, so the row is withheld wherever it runs.
    Withheld {
        /// What the environment would have to be.
        needs: &'static str,
        /// Why the report cannot settle it.
        reason: &'static str,
    },
    /// The code the case needs does not exist yet.
    Blocked {
        /// The tasks the case waits on.
        tasks: &'static [&'static str],
        /// What is missing, in one line.
        reason: &'static str,
    },
}

/// A capability the environment report can settle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Capability {
    /// An X server reachable at `$DISPLAY`.
    X11,
    /// The Fcitx5 development package, which both addons need to build and load.
    Fcitx5,
    /// A Wayland session whose compositor serves one of the four registered tiers.
    WaylandTier,
}

impl Capability {
    /// Why the report does not show this capability, or `None` when it does.
    ///
    /// # Panics
    ///
    /// Never.
    fn withholding(self, env: &EnvCapabilities) -> Option<String> {
        match self {
            Self::X11 => x11_withholding(env),
            Self::Fcitx5 => fcitx5_withholding(env),
            Self::WaylandTier => wayland_withholding(env),
        }
    }
}

/// The decision table: what this gate answers for every requirement a case may declare.
pub(super) const DECISION_TABLE: [DecisionRow; 9] = [
    DecisionRow {
        requirement: EnvRequirement::X11Session,
        verdict: RowVerdict::ByReport {
            capability: Capability::X11,
            needs: X11_NEEDS,
        },
        register: UNREGISTERED,
        remedy: X11_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::Fcitx5Session,
        verdict: RowVerdict::ByReport {
            capability: Capability::Fcitx5,
            needs: FCITX5_NEEDS,
        },
        register: UNREGISTERED,
        remedy: FCITX5_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::WaylandTier,
        verdict: RowVerdict::ByReport {
            capability: Capability::WaylandTier,
            needs: WAYLAND_TIER_NEEDS,
        },
        register: &["TASK-1.04.07", "TASK-1.05.07"],
        remedy: WAYLAND_TIER_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::CompositorBlur,
        verdict: RowVerdict::Withheld {
            needs: BLUR_NEEDS,
            reason: BLUR_REASON,
        },
        register: &["TASK-1.05.04"],
        remedy: BLUR_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::MultipleMonitors,
        verdict: RowVerdict::Withheld {
            needs: MULTI_MONITOR_NEEDS,
            reason: MONITORS_REASON,
        },
        register: &["TASK-1.04.05"],
        remedy: MULTI_MONITOR_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::LongRun,
        verdict: RowVerdict::Withheld {
            needs: LONG_RUN_NEEDS,
            reason: LONG_RUN_REASON,
        },
        register: &["TASK-1.02.07", "TASK-1.08.03"],
        remedy: LONG_RUN_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::CandidateWindowUi,
        verdict: RowVerdict::Blocked {
            tasks: &["TASK-1.05.01–1.05.08"],
            reason: WINDOW_REASON,
        },
        register: UNREGISTERED,
        remedy: WINDOW_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::ConfigReload,
        verdict: RowVerdict::Blocked {
            tasks: &["TASK-1.03.06"],
            reason: CONFIG_REASON,
        },
        register: UNREGISTERED,
        remedy: CONFIG_REMEDY,
    },
    DecisionRow {
        requirement: EnvRequirement::LogRedaction,
        verdict: RowVerdict::Blocked {
            tasks: &["TASK-1.08.01"],
            reason: LOG_REASON,
        },
        register: UNREGISTERED,
        remedy: LOG_REMEDY,
    },
];

/// A verdict that withholds a case for want of an environment.
///
/// # Panics
///
/// Never.
fn unverifiable(needs: &str, reason: &str) -> Executability {
    Executability::Unverifiable {
        needs: needs.to_owned(),
        reason: reason.to_owned(),
    }
}

/// The verdict for a case whose requirement the table does not declare.
///
/// # Panics
///
/// Never.
fn undeclared() -> Executability {
    unverifiable(UNDECLARED_NEEDS, UNDECLARED_REASON)
}

/// Why no X server answered, or `None` when one did.
///
/// The display name is what separates "nobody looked" from "there is no server": a session with
/// no `$DISPLAY` never tried, and one whose name could not be opened is a gap rather than a
/// negative answer.
///
/// # Panics
///
/// Never.
fn x11_withholding(env: &EnvCapabilities) -> Option<String> {
    match env.display.as_deref() {
        None => Some(String::from(
            "no $DISPLAY was set, so no X server was tried",
        )),
        Some(display) if env.has_gap(ProbeGap::X11Unreachable) => {
            Some(format!("the display {display} could not be opened"))
        }
        Some(_) => None,
    }
}

/// Why no Fcitx5 session can be driven, or `None` when one can.
///
/// The development package is the evidence: it is what both addons are compiled against and
/// what Fcitx5 refuses to load an addon without. Whether the addons loaded in a given session
/// is a result rather than a capability, so it is not asked here.
///
/// # Panics
///
/// Never.
fn fcitx5_withholding(env: &EnvCapabilities) -> Option<String> {
    (!env.dev_packages).then(|| {
        String::from("pkg-config does not know the Fcitx5Core module, so no addon can load")
    })
}

/// Why the session cannot exercise a compositor tier, or `None` when it can.
///
/// Three situations withhold a Wayland case, and they are three different answers: the session
/// is not Wayland at all; the compositor serves none of the four tiers `features.md` 0.5.2
/// registers, which is `ASM-13`'s case for Weston and its neighbours; or the tier could not be
/// established because the `wl_registry` listing was not read, in which case the tier is an
/// inference from the compositor's name and not a reading.
///
/// # Panics
///
/// Never.
fn wayland_withholding(env: &EnvCapabilities) -> Option<String> {
    if env.display_server != DisplayServer::Wayland {
        return Some(String::from("the session is not a Wayland session"));
    }
    if env.tier == WaylandTier::NotApplicable {
        return Some(compositor_withholding(env));
    }
    if env.has_gap(ProbeGap::WaylandRegistryUnread) {
        return Some(String::from(
            "the wl_registry listing was not read, so the tier is an inference, not a reading",
        ));
    }
    None
}

/// Why the compositor serves none of the four tiers.
///
/// # Panics
///
/// Never.
fn compositor_withholding(env: &EnvCapabilities) -> String {
    match env.compositor.as_deref() {
        Some(name) => {
            format!("the compositor {name} is outside the four tiers features.md 0.5.2 registers")
        }
        None => String::from("no compositor process of the session's kind could be identified"),
    }
}
