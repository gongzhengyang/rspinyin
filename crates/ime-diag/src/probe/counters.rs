//! The counter registry: the diagnostic counters the boundary contract names.
//!
//! Responsibility: name every counter the design fixes and give each one a fixed
//! index, so that a bump is one `fetch_add` on an array slot rather than a lookup by
//! name on the hot path. The names are contractual -- diagnostics and tests match on
//! them, exactly as they match on the `domain/action/reason` error codes -- so they
//! are stated once, here, and never reworded.
//!
//! Boundaries: this module owns names and indices and nothing else. Which code path
//! bumps which counter is the wiring's decision: the layers that produce the
//! conditions (the UI channels, the decoder's error path, the dictionary lookup, the
//! path layer's read-only degradation) report them to their own callers, and the
//! probe is fed from there. That is what keeps the pure engine free of diagnostics.
//!
//! Every counter is a count of a *condition*, never of user input: how many frames
//! were coalesced, how many clicks timed out. The design's diagnostic substitute for
//! content -- candidate counts and source distributions -- is measured elsewhere and
//! is deliberately not a counter here, because a probe that could carry a character
//! a user typed would be a probe that could leak one.
//!
//! # Where each counter is raised
//!
//! The condition belongs to the layer that produces it; the count belongs here. The
//! table says which layer that is, so that a counter cannot be added to the contract
//! and then never raised -- or raised twice, from two layers that both think the
//! other one owns it.
//!
//! | Counter | Raised by |
//! |---|---|
//! | `ui.frame.coalesced` | the UI command channel, when a frame replaces one the UI thread has not read |
//! | `ui.control.dropped` | the UI command channel, when the ordered `Show` / `Hide` queue is still full after the sender's budget |
//! | `ui.select.timeout` | the UI thread, when it gives up waiting for room in the select queue |
//! | `ui.click.debounced` | the UI interaction layer, when the double-click debounce suppresses a click |
//! | `ui.stale-select` | the host thread, when a selection names a revision the session has moved past |
//! | `ui.buffer.starvation` | the UI thread, when a frame is due and none has arrived |
//! | `ui.not-ready` | the UI thread, when a command arrives before the window is warm |
//! | `ui.thread.dead` | the command sender, when the UI thread has exited and the command is discarded |
//! | `probe.lost` | the host thread, when a render receipt never comes back for a stamped key |
//! | `decode.too-long` | the caller of a decode, on the decoder's too-long failure |
//! | `decode.no-path` | the caller of a decode, on the decoder's no-path failure |
//! | `dict.lookup.miss` | the caller of a dictionary lookup that found nothing |
//! | `userdb.commit.slow` | the caller of a user-frequency commit that took too long |
//! | `data.readonly-mode` | the path layer, when the data directory is unusable and the plugin degrades |
//! | `config.invalid` | the configuration loader, on a document it rejected or repaired |
//! | `ui.theme.blur-unavailable` | the theme layer, when the compositor negotiates no blur |
//! | `platform.cursor.unresolved` | the platform layer, when no cursor geometry can be obtained |
//! | `platform.x11.no-compositor` | the X11 backend probe, when there is no compositing manager |
//! | `platform.x11.no-argb-visual` | the X11 backend probe, when there is no 32-bit visual |

/// The number of counters a probe carries.
///
/// A constant rather than `Counter::ALL.len()` so that it can be an array length;
/// a test keeps the two in step.
pub const COUNTER_COUNT: usize = 19;

/// One diagnostic counter.
///
/// The variant names are the contract's own short forms; [`Counter::name`] carries
/// the dotted diagnostic name a report prints and a log line matches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Counter {
    /// `ui.frame.coalesced`: frames replaced in the latest-wins slot before the UI
    /// thread read them.
    FrameCoalesced,
    /// `ui.control.dropped`: ordered `Show` / `Hide` commands collapsed because the
    /// queue was full past the host's wait budget.
    ControlDropped,
    /// `ui.select.timeout`: clicks abandoned because the host was not draining the
    /// select queue within the UI thread's budget.
    SelectTimeout,
    /// `ui.click.debounced`: clicks suppressed by the double-click debounce.
    ClickDebounced,
    /// `ui.stale-select`: selections naming a revision the session has moved past,
    /// the counter behind the `ui/stale-select` code.
    StaleSelect,
    /// `ui.buffer.starvation`: frames the UI thread had nothing to draw for because
    /// no frame had arrived yet.
    BufferStarvation,
    /// `ui.not-ready`: commands that arrived before the window finished warming up.
    NotReady,
    /// `ui.thread.dead`: the UI thread has exited, so commands are discarded rather
    /// than queued.
    ThreadDead,
    /// `probe.lost`: latency samples whose render receipt never came back, so the
    /// measurement was dropped.
    ProbeLost,
    /// `decode.too-long`: decodes refused for exceeding the input length limit, the
    /// counter behind the `decode/too-long` code.
    DecodeTooLong,
    /// `decode.no-path`: decodes that found no path through the lattice, the counter
    /// behind the `decode/no-path` code.
    DecodeNoPath,
    /// `dict.lookup.miss`: lookups that found no entry for a syllable string.
    DictLookupMiss,
    /// `userdb.commit.slow`: user-frequency commits that took longer than the write
    /// budget allows.
    UserDbCommitSlow,
    /// `data.readonly-mode`: the data directory was unusable and the plugin degraded
    /// to read-only, the counter behind the `data/readonly-mode` code.
    DataReadonlyMode,
    /// `config.invalid`: configuration documents rejected or repaired, the counter
    /// behind the `config/invalid` code.
    ConfigInvalid,
    /// `ui.theme.blur-unavailable`: the compositor negotiated no blur, so the theme
    /// fell back to an opaque background.
    ThemeBlurUnavailable,
    /// `platform.cursor.unresolved`: no cursor geometry could be obtained, so the
    /// window fell back to a screen corner.
    CursorUnresolved,
    /// `platform.x11.no-compositor`: an X11 session with no compositing manager.
    X11NoCompositor,
    /// `platform.x11.no-argb-visual`: an X11 session with no 32-bit visual, so the
    /// window cannot be translucent.
    X11NoArgbVisual,
}

impl Counter {
    /// Every counter, in the order the boundary contract lists them.
    pub const ALL: [Self; 19] = [
        Self::FrameCoalesced,
        Self::ControlDropped,
        Self::SelectTimeout,
        Self::ClickDebounced,
        Self::StaleSelect,
        Self::BufferStarvation,
        Self::NotReady,
        Self::ThreadDead,
        Self::ProbeLost,
        Self::DecodeTooLong,
        Self::DecodeNoPath,
        Self::DictLookupMiss,
        Self::UserDbCommitSlow,
        Self::DataReadonlyMode,
        Self::ConfigInvalid,
        Self::ThemeBlurUnavailable,
        Self::CursorUnresolved,
        Self::X11NoCompositor,
        Self::X11NoArgbVisual,
    ];

    /// The stable diagnostic name a report prints.
    pub const fn name(self) -> &'static str {
        match self {
            Self::FrameCoalesced => "ui.frame.coalesced",
            Self::ControlDropped => "ui.control.dropped",
            Self::SelectTimeout => "ui.select.timeout",
            Self::ClickDebounced => "ui.click.debounced",
            Self::StaleSelect => "ui.stale-select",
            Self::BufferStarvation => "ui.buffer.starvation",
            Self::NotReady => "ui.not-ready",
            Self::ThreadDead => "ui.thread.dead",
            Self::ProbeLost => "probe.lost",
            Self::DecodeTooLong => "decode.too-long",
            Self::DecodeNoPath => "decode.no-path",
            Self::DictLookupMiss => "dict.lookup.miss",
            Self::UserDbCommitSlow => "userdb.commit.slow",
            Self::DataReadonlyMode => "data.readonly-mode",
            Self::ConfigInvalid => "config.invalid",
            Self::ThemeBlurUnavailable => "ui.theme.blur-unavailable",
            Self::CursorUnresolved => "platform.cursor.unresolved",
            Self::X11NoCompositor => "platform.x11.no-compositor",
            Self::X11NoArgbVisual => "platform.x11.no-argb-visual",
        }
    }

    /// The index this counter occupies in a counter array.
    ///
    /// A plain discriminant cast: the variants carry no data, so the index is stable
    /// and the array in [`Probes`](super::Probes) is exactly as long as
    /// [`Counter::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The counter a diagnostic name refers to.
    ///
    /// A name that is not one of [`Counter::ALL`] is `None` rather than a new
    /// counter, so that a snapshot file naming something unknown is refused instead
    /// of silently contributing a zero.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|counter| counter.name() == name)
    }
}
