//! Pure recognition for the quick-input trigger table.
//!
//! This module decides whether a raw buffer is asking for a quick-input domain and
//! names that domain. It deliberately renders nothing: date formatting, number
//! conversion, expression evaluation and candidate selection are separate increments.
//! Keeping recognition here lets later work plug a translator beside the state
//! machine without making the decoder depend on configuration or time.

/// The quick-input domains the trigger layer can recognise.
///
/// The calculator is a separate trigger because its admission is configured
/// independently of the date-family prefixes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuickDomain {
    /// One of the configured date-family trigger spellings was matched.
    Date,
    /// The calculator trigger was matched and the calculator is enabled.
    Calc,
}

/// A trigger decision: which quick-input domain the buffer selects.
///
/// This is a scaffold for later translator integration; it carries no candidate text
/// and performs no allocation on a miss.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuickMatch {
    /// The domain the matched trigger selects.
    pub domain: QuickDomain,
    /// The number of bytes in the matched trigger spelling.
    pub trigger_len: usize,
}

/// Returns the quick-input domain selected by `buffer`, if any.
///
/// The lookup is whole-prefix equality, not incremental character matching: a trigger
/// is complete only when the entire spelling has been typed. This keeps the function
/// deterministic and allocation-free, and makes an unmatched buffer cost at most one
/// byte comparison per configured spelling.
///
/// # Arguments
///
/// * `enabled` - the quick-input master switch.
/// * `buffer` - the raw spelling currently in the input buffer.
/// * `date_prefixes` - the enabled date-family trigger spellings, in configuration
///   order.
/// * `calc_enabled` - whether the calculator trigger may be recognised.
///
/// # Returns
///
/// `Some` with the first matching trigger when quick input is enabled and the buffer
/// exactly names a trigger; `None` otherwise.
///
/// # Panics
///
/// Never.
pub fn recognize(
    enabled: bool,
    buffer: &str,
    date_prefixes: &[&str],
    calc_enabled: bool,
) -> Option<QuickMatch> {
    if !enabled {
        return None;
    }
    for prefix in date_prefixes {
        if buffer == *prefix {
            return Some(QuickMatch {
                domain: QuickDomain::Date,
                trigger_len: prefix.len(),
            });
        }
    }
    if calc_enabled && buffer == "cC" {
        return Some(QuickMatch {
            domain: QuickDomain::Calc,
            trigger_len: "cC".len(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{QuickDomain, recognize};

    #[test]
    fn test_recognize_disabled_returns_no_match() {
        assert_eq!(recognize(false, "rq", &["rq"], true), None);
    }

    #[test]
    fn test_recognize_date_prefix_requires_whole_trigger() {
        assert!(recognize(true, "rq", &["rq", "dt"], true).is_some());
        assert!(recognize(true, "rqx", &["rq", "dt"], true).is_none());
        assert!(recognize(true, "", &["rq", "dt"], true).is_none());
    }

    #[test]
    fn test_recognize_calc_only_when_enabled() {
        assert_eq!(
            recognize(true, "cC", &[], true).map(|found| found.domain),
            Some(QuickDomain::Calc)
        );
        assert_eq!(recognize(true, "cC", &[], false), None);
    }

    #[test]
    fn test_recognize_first_date_prefix_wins() {
        let matched = recognize(true, "dt", &["dt", "rq"], true);
        assert_eq!(matched.map(|found| found.trigger_len), Some(2));
    }
}
