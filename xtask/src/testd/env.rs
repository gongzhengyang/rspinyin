//! Environment capability probing: what this machine can actually do.
//!
//! Responsibility: answer, in one structured report, the questions a case asks before it
//! runs -- which display server is present, which compositor, which of the four Wayland
//! positioning tiers that compositor can serve, whether the Fcitx5 development package is
//! installed, whether a session loaded both addons, how many CJK fonts the font stack has,
//! whether the plugin may write its data directory, and whether another build or test
//! process is running right now.
//!
//! Boundaries: the probe reads and never writes, and it decides nothing about any case.
//! The environment gate reads this report to decide which cases are executable and which
//! must be marked unverifiable; the purity guard reads `concurrent_agents` to decide
//! whether a performance number may be believed. Neither of them re-probes anything, so
//! this module is the single place a capability question is answered.
//!
//! # One record, two halves
//!
//! [`observation`] reads the machine into an [`Observation`] and interprets nothing:
//! environment variables, `/proc`, the X server, `pkg-config`, `fc-list` and the addon
//! descriptors. [`EnvCapabilities::from_observation`] turns that record into the report, as
//! a pure function of it. Everything the report can say is therefore testable with no
//! display server, no compositor, no Fcitx5 and no process table, which is how the tests
//! beside this module are written.
//!
//! # `false` and "unknown" are different answers
//!
//! An unknown read as `false` is how a capability gate passes a case for the wrong reason.
//! Every reading the probe could not take is therefore recorded in
//! [`EnvCapabilities::gaps`], with the stable code the gate records and the environment
//! that would settle it, while the field itself carries the value the evidence supports:
//! `false`, `0` or `None`. A consumer that must not read an unknown as a negative answer
//! asks [`EnvCapabilities::has_gap`] first.
//!
//! # What this probe cannot establish on the development machine
//!
//! `xtask` links no Wayland client, so the `wl_registry` listing that separates T1 from T2
//! is not read here: the tier comes from the compositor table in [`compositor`], which is
//! the per-compositor mapping `features.md` 0.5.5 registers, and a Wayland session
//! therefore always carries [`ProbeGap::WaylandRegistryUnread`]. On the development machine
//! the compositor is Weston, which `ASM-13` names as lying outside all four tiers, so the
//! tier is [`WaylandTier::NotApplicable`] -- and it is that because the compositor was
//! identified, not because a reading failed.
//!
//! [`compositor`]: self::compositor
//! [`observation`]: self::observation

// The report's consumers are the environment gate and the purity guard, which are separate
// tasks of this platform and do not exist yet, and the subcommand tree that would print it
// lives in `xtask/src/main.rs` and `xtask/src/testd/mod.rs` -- two files this module does not
// own. Until that wiring lands, every item here is reported as dead code in a non-test build,
// and the attribute goes away with those lines.
//
// `unused_imports` is covered by the same reasoning: the `pub use` lines below are this
// module's surface, and a `pub use` in a *binary* crate is reported as unused whenever
// nothing in the crate names it.
#![allow(dead_code, unused_imports)]

mod addons;
mod compositor;
mod observation;
mod process;

#[cfg(test)]
mod tests;

pub use self::addons::{AddonDescriptor, AddonSpec, AddonState, EXPECTED_ADDONS};
pub use self::compositor::{CompositorFamily, RegistryFacts};
pub use self::observation::{Observation, X11Facts};
pub use self::process::ProcessEntry;

use self::compositor::Compositor;

/// The display server the session runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayServer {
    /// A Wayland session: `$WAYLAND_DISPLAY` names a socket that exists.
    Wayland,
    /// An X11 session: a connection to `$DISPLAY` was opened.
    X11,
    /// Neither: no display server was reachable. An `xtask` test run with no display is this
    /// case, and it is not a failure -- every in-process engine case runs here.
    Headless,
}

impl DisplayServer {
    /// The label the report and a case's executability table match on.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::Wayland => "wayland",
            Self::X11 => "x11",
            Self::Headless => "headless",
        }
    }
}

/// Which of the four Wayland positioning tiers applies to this session.
///
/// The variants mirror `ime_ui::platform::wayland::Tier`, which is the code that acts on
/// them. `xtask` does not link the renderer, so the vocabulary is repeated here rather than
/// imported, exactly as the coordinate conversion is: a change to one is a change to both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaylandTier {
    /// T1: a `zwlr_layer_shell_v1` overlay, positioned by its margins.
    LayerShell,
    /// T2: an `xdg_popup` the compositor positions.
    Popup,
    /// T3: an `xdg_popup` this plugin positions, over a full-screen parent surface.
    CanvasPopup,
    /// T4: no tier answered, so the host's own candidate list draws.
    Fallback,
    /// No tier of the ladder applies to this session.
    ///
    /// Three situations produce it, and [`EnvCapabilities::gaps`] says which: the session is
    /// not Wayland at all; the compositor is outside the four registered tiers, which is
    /// `ASM-13`'s case for Weston and the other compositors it names; or the tier could not
    /// be established because the `wl_registry` listing was not read.
    NotApplicable,
}

impl WaylandTier {
    /// The label the diagnostics and the case table use: `T1` through `T4`, or
    /// `not-applicable`.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn label(self) -> &'static str {
        match self {
            Self::LayerShell => "T1",
            Self::Popup => "T2",
            Self::CanvasPopup => "T3",
            Self::Fallback => "T4",
            Self::NotApplicable => "not-applicable",
        }
    }
}

/// A reading the probe could not take, and what would take it.
///
/// This list is the report's honesty mechanism. A consumer that reads `false` as "no" would
/// turn "nobody looked" into a negative answer, and the environment gate must mark such a
/// case unverifiable instead of passing it. Each variant carries the stable
/// `domain/action/reason` code the gate records and the environment a full answer needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeGap {
    /// `$DISPLAY` names a server the probe could not open a connection to, so every X11
    /// reading is unknown.
    X11Unreachable,
    /// A Wayland session was detected but no compositor process of that kind is running, so
    /// neither the compositor nor the tier is known.
    CompositorUnknown,
    /// The `wl_registry` listing was not read, so the tier and the layer-shell answer come
    /// from the compositor table rather than from the compositor itself.
    WaylandRegistryUnread,
    /// `fc-list` could not be run, so the CJK font count is unknown.
    CjkFontCountUnknown,
    /// `/proc` could not be read, so the number of build or test processes is unknown.
    AgentCountUnknown,
    /// Not every addon descriptor this product installs could be read, so an addon carries
    /// the category its registration declares rather than the one its descriptor does.
    AddonDescriptorsUnread,
    /// No Fcitx5 session log was supplied, so no addon load verdict exists at all.
    SessionLogAbsent,
    /// The plugin's data directory could not be resolved, so its writability is unknown.
    DataDirUnknown,
}

impl ProbeGap {
    /// The stable `domain/action/reason` code, which diagnostics and tests match on.
    ///
    /// These strings are part of the diagnostic contract: never reword one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn code(self) -> &'static str {
        match self {
            Self::X11Unreachable => "env/x11/unreachable",
            Self::CompositorUnknown => "env/compositor/unknown",
            Self::WaylandRegistryUnread => "env/wayland/registry-unread",
            Self::CjkFontCountUnknown => "env/fonts/unknown",
            Self::AgentCountUnknown => "env/proc/unreadable",
            Self::AddonDescriptorsUnread => "env/addons/descriptors-unread",
            Self::SessionLogAbsent => "env/session/log-absent",
            Self::DataDirUnknown => "env/data/dir-unknown",
        }
    }

    /// What would turn this gap into an answer, in one line.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn needs(self) -> &'static str {
        match self {
            Self::X11Unreachable => "an X server reachable at $DISPLAY",
            Self::CompositorUnknown => "a Wayland compositor process the table recognises",
            Self::WaylandRegistryUnread => "a wl_registry listing (needs a Wayland client)",
            Self::CjkFontCountUnknown => "fontconfig's fc-list",
            Self::AgentCountUnknown => "a readable /proc",
            Self::AddonDescriptorsUnread => "the descriptors Fcitx5 installs for its addons",
            Self::SessionLogAbsent => "a Fcitx5 session log from a sandbox run",
            Self::DataDirUnknown => "an absolute $XDG_DATA_HOME or $HOME",
        }
    }
}

/// What this machine can do, as one report.
///
/// Every field is public because the report is consumed field by field by the environment
/// gate and the purity guard, and because a case's diagnostics quote it. The fields a
/// consumer must not read as a bare negative are the ones a gap can accompany.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvCapabilities {
    /// The version of the Fcitx5 development package `pkg-config` reports, e.g. `5.1.7`;
    /// `None` when the development package is not installed.
    pub fcitx5_version: Option<String>,
    /// Whether `pkg-config` finds the Fcitx5 development package.
    pub dev_packages: bool,
    /// The addons this product ships, each with the category its descriptor declares and
    /// whether a session reported it loaded.
    pub addons_loaded: Vec<AddonState>,
    /// The display server the session runs on.
    pub display_server: DisplayServer,
    /// The compositor, named as the table names it (`weston`, `kwin`, `sway`); `None` when
    /// no compositor process of the session's own kind is running.
    pub compositor: Option<String>,
    /// Which of the four Wayland positioning tiers applies.
    pub tier: WaylandTier,
    /// Whether `zwlr_layer_shell_v1` is available, which is what T1 needs.
    pub wlr_layer_shell: bool,
    /// Whether the X server offers a 32-bit ARGB visual, which the transparent candidate
    /// window is drawn into.
    pub argb_visual: bool,
    /// Whether a compositor owns `_NET_WM_CM_S<n>` on the X server. This is the X11
    /// question: on a Wayland session transparency is native, and the field says nothing
    /// about it.
    pub compositor_present: bool,
    /// The fonts `fc-list :lang=zh` reports, counted as fontconfig lists them.
    pub cjk_font_count: u32,
    /// Whether the plugin's data directory may be written to. Read from the directory's
    /// permission bits, because a probe must not create a file in the operator's real
    /// `$XDG_DATA_HOME` to find out.
    pub writable_data_dir: bool,
    /// Build and test processes running outside this probe's own ancestry.
    pub concurrent_agents: u32,
    /// `$DISPLAY` as the session has it, which is the name the injection channel uses.
    pub display: Option<String>,
    /// `$WAYLAND_DISPLAY` as the session has it.
    pub wayland_display: Option<String>,
    /// What could not be established, in the order the probe found it.
    pub gaps: Vec<ProbeGap>,
}

impl EnvCapabilities {
    /// Probes the machine as it is at this moment.
    ///
    /// # Panics
    ///
    /// Never: every reading is optional, and a reading that cannot be taken becomes a gap.
    pub fn probe() -> Self {
        Self::from_observation(&Observation::read())
    }

    /// Probes the machine and takes the addon load verdicts from a Fcitx5 session log.
    ///
    /// The log is the only source of a load verdict: Fcitx5 writes `Loaded addon ...` on its
    /// own output, and `fcitx5-diagnose` never prints a per-addon verdict at all, so a
    /// caller that wants to know whether both addons came up has to bring the log a session
    /// wrote. The sandbox archives one per run.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn probe_with_session_log(log: &str) -> Self {
        let mut observation = Observation::read();
        observation.session_log = Some(log.to_owned());
        Self::from_observation(&observation)
    }

    /// Interprets an [`Observation`] as the report.
    ///
    /// This is the whole of the interpretation: the readers in [`observation`] and
    /// [`process`] produce the record, and everything above this line is a pure function of
    /// it, which is why the report can be tested for a machine that is not this one.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn from_observation(observation: &Observation) -> Self {
        let display_server = display_server_of(observation);
        let compositor = compositor_of(observation, display_server);
        let (tier, wlr_layer_shell) =
            wayland_facts(display_server, compositor, observation.registry);
        Self {
            fcitx5_version: observation.fcitx5_version.clone(),
            dev_packages: observation.fcitx5_version.is_some(),
            addons_loaded: addons::states(
                &observation.addon_descriptors,
                observation.session_log.as_deref(),
            ),
            display_server,
            compositor: compositor.map(|known| known.name.to_owned()),
            tier,
            wlr_layer_shell,
            argb_visual: observation.x11.is_some_and(|facts| facts.argb_visual),
            compositor_present: observation
                .x11
                .is_some_and(|facts| facts.compositor_present),
            cjk_font_count: cjk_font_count(observation),
            writable_data_dir: observation.data_dir_writable,
            concurrent_agents: concurrent_agents(observation),
            display: observation.display.clone(),
            wayland_display: observation.wayland_display.clone(),
            gaps: gaps_of(observation, display_server, compositor),
        }
    }

    /// The expected addons a session log did not report as loaded.
    ///
    /// Empty when no session log was supplied: without one no load verdict exists, and the
    /// report carries [`ProbeGap::SessionLogAbsent`] rather than claiming an addon is
    /// missing. A log that mentions neither addon reports both, which is what a session
    /// that never reached the addon loader looks like.
    ///
    /// This is the ADR-0003 observability point: the plugin is two addons with two
    /// descriptors, and a run that loaded only one of them has to say which.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn missing_addons(&self) -> Vec<String> {
        if self.has_gap(ProbeGap::SessionLogAbsent) {
            return Vec::new();
        }
        self.addons_loaded
            .iter()
            .filter(|addon| !addon.loaded)
            .map(|addon| addon.name.clone())
            .collect()
    }

    /// Whether a reading was missed.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn has_gap(&self, gap: ProbeGap) -> bool {
        self.gaps.contains(&gap)
    }

    /// The report as lines of text, for `xtask` to print and for the resource layer to
    /// publish.
    ///
    /// The shape is stable: one `key: value` line per field in the order the fields are
    /// declared, then one line per addon, then one line per gap carrying the gap's code and
    /// what would settle it. Nothing here is user input -- a display name, a compositor name
    /// and a version are the whole of it.
    ///
    /// # Panics
    ///
    /// Never.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.push(format!("display_server: {}", self.display_server.label()));
        lines.push(format!(
            "display: {}",
            self.display.as_deref().unwrap_or("none")
        ));
        lines.push(format!(
            "wayland_display: {}",
            self.wayland_display.as_deref().unwrap_or("none")
        ));
        lines.push(format!(
            "compositor: {}",
            self.compositor.as_deref().unwrap_or("unknown")
        ));
        lines.push(format!("tier: {}", self.tier.label()));
        lines.push(format!("wlr_layer_shell: {}", self.wlr_layer_shell));
        lines.push(format!("argb_visual: {}", self.argb_visual));
        lines.push(format!("compositor_present: {}", self.compositor_present));
        lines.push(format!(
            "fcitx5_version: {}",
            self.fcitx5_version.as_deref().unwrap_or("unknown")
        ));
        lines.push(format!("dev_packages: {}", self.dev_packages));
        lines.push(format!("cjk_font_count: {}", self.cjk_font_count));
        lines.push(format!("writable_data_dir: {}", self.writable_data_dir));
        lines.push(format!("concurrent_agents: {}", self.concurrent_agents));
        for addon in &self.addons_loaded {
            lines.push(format!(
                "addon {}: category {}, loaded {}",
                addon.name, addon.category, addon.loaded
            ));
        }
        for gap in &self.gaps {
            lines.push(format!("gap {}: needs {}", gap.code(), gap.needs()));
        }
        lines
    }
}

/// The display server the session runs on.
///
/// Wayland wins when both are present, which is what the plugin does: a session with a
/// Wayland socket is a Wayland session, and the X server beside it is how that session
/// serves its X11 clients rather than a second session.
fn display_server_of(observation: &Observation) -> DisplayServer {
    if observation.wayland_display.is_some() && observation.wayland_socket {
        return DisplayServer::Wayland;
    }
    if observation.x11.is_some() {
        return DisplayServer::X11;
    }
    DisplayServer::Headless
}

/// The compositor of this session, when a process of the session's own kind is running.
///
/// The process table is what names a compositor: Wayland has no request that answers the
/// question, and an X11 compositor is an ordinary client. A session with no display server
/// has no compositor, and one whose compositor the table does not recognise reports `None`
/// rather than a guess.
fn compositor_of(observation: &Observation, server: DisplayServer) -> Option<Compositor> {
    if server == DisplayServer::Headless {
        return None;
    }
    let table = observation.processes.as_ref()?;
    let names: Vec<&str> = table.iter().map(|row| row.comm.as_str()).collect();
    compositor::identify(&names, server)
}

/// The tier and the layer-shell answer for this session.
fn wayland_facts(
    server: DisplayServer,
    compositor: Option<Compositor>,
    registry: RegistryFacts,
) -> (WaylandTier, bool) {
    let known = match compositor {
        Some(known) if server == DisplayServer::Wayland => known,
        _ => return (WaylandTier::NotApplicable, false),
    };
    let family = known.family;
    // Qualified through `self` because the parameter above shares the module's name.
    (
        self::compositor::tier_for(family, registry),
        self::compositor::layer_shell_for(family, registry),
    )
}

/// The readings an observation does not carry, in the order the probe looks for them.
///
/// A gap is recorded for a reading that could not be taken, and never for one that was taken
/// and came back negative: a machine with no compositor and a machine whose compositor nobody
/// could identify are different answers, and only the second is a gap.
fn gaps_of(
    observation: &Observation,
    display_server: DisplayServer,
    compositor: Option<Compositor>,
) -> Vec<ProbeGap> {
    let mut gaps = Vec::new();
    if observation.display.is_some() && observation.x11.is_none() {
        gaps.push(ProbeGap::X11Unreachable);
    }
    if compositor.is_none() && display_server == DisplayServer::Wayland {
        gaps.push(ProbeGap::CompositorUnknown);
    }
    if display_server == DisplayServer::Wayland && !observation.registry.was_read() {
        gaps.push(ProbeGap::WaylandRegistryUnread);
    }
    if observation.cjk_fonts.is_none() {
        gaps.push(ProbeGap::CjkFontCountUnknown);
    }
    if observation.processes.is_none() {
        gaps.push(ProbeGap::AgentCountUnknown);
    }
    if observation.addon_descriptors.len() < EXPECTED_ADDONS.len() {
        gaps.push(ProbeGap::AddonDescriptorsUnread);
    }
    if observation.session_log.is_none() {
        gaps.push(ProbeGap::SessionLogAbsent);
    }
    if observation.data_dir.is_none() {
        gaps.push(ProbeGap::DataDirUnknown);
    }
    gaps
}

/// The CJK font count, zero when the font stack could not be asked.
fn cjk_font_count(observation: &Observation) -> u32 {
    // Qualified through `self` because the parameter above shares the module's name.
    observation
        .cjk_fonts
        .as_deref()
        .map_or(0, self::observation::count_cjk_fonts)
}

/// The number of build or test processes outside this probe's own ancestry, zero when the
/// process table could not be read.
fn concurrent_agents(observation: &Observation) -> u32 {
    observation
        .processes
        .as_ref()
        .map_or(0, |table| process::count_agents(table, observation.own_pid))
}
