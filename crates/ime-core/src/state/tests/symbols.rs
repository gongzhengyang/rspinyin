//! The symbol panel's extended views, driven as whole sessions: the `vhelp`
//! index page, the domain jump out of it, the emoji alias filter, and the
//! paging of a category larger than the first supply rung.
//!
//! Beside the symbol walkthroughs in `transitions.rs`, which pin the panel's
//! original three answers; this file pins what the domain extension added.
//! Driven entirely from the in-memory doubles next door. No dictionary file,
//! no clock, no environment, no display server.

use ime_types::{CandidateSource, KeyAction};

use crate::state::SessionConfig;
use crate::state::SessionEnv;
use crate::state::machine::{Effect, Session, SessionEvent, SessionState, step};
use crate::symbols::SymbolTable;

use super::{Fixture, committed, frame_of};

/// Types `raw` into a fresh session over `env`, one [`KeyAction::InputChar`] each.
fn typed(raw: &str, cfg: &SessionConfig, env: &SessionEnv<'_>) -> (Session, Vec<Effect>) {
    let mut session = Session::new();
    let mut effects = Vec::new();
    for ch in raw.chars() {
        effects.extend(session.handle_key(KeyAction::InputChar(ch), cfg, env));
    }
    (session, effects)
}

/// The index-page texts the session is holding, in display order.
fn texts_of(session: &Session) -> Vec<&str> {
    session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.text.as_str())
        .collect()
}

/// The panel table of the index-page walkthroughs: two document categories and
/// the emoji domain added after, which is the shipped table's shape plus the
/// extension.
fn index_fixture_table() -> SymbolTable {
    let mut table = SymbolTable::parse("1\t数字\t０１\nx\t箭头\t←\n").expect("rows are usable");
    let _ = table.add_emoji_domain("😀\txiao\n😊\tlian\n");
    table
}

#[test]
fn test_vhelp_index_lists_domains_and_trigger_keys_in_table_order() {
    // The index page is one candidate per domain, the label as the text and the
    // trigger key as the annotation, in table order -- the sequence the page
    // promises, pinned here.
    let fixture = Fixture::new();
    let table = index_fixture_table();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (session, _) = typed("vhelp", &cfg, &env);

    assert!(session.is_symbol_mode(), "the index page is the panel's");
    assert_eq!(session.preedit.text, "vhelp");
    assert_eq!(texts_of(&session), ["数字", "箭头", "表情"]);
    let keys: Vec<&str> = session
        .decoded()
        .candidates
        .iter()
        .map(|candidate| candidate.annotation.as_deref().expect("the trigger key"))
        .collect();
    assert_eq!(keys, ["v1", "vx", "ve"]);
    assert!(
        session
            .decoded()
            .candidates
            .iter()
            .all(|candidate| candidate.source == CandidateSource::Symbol)
    );
}

#[test]
fn test_vhelp_without_a_table_falls_through_to_the_decode() {
    // An environment with no symbol table never opens the page: `vhelp` is an
    // ordinary input, echoed exactly as a build before the panel existed. This
    // is the extension's zero-change promise for a table-less install.
    let fixture = Fixture::new();
    let env = fixture.env();
    let cfg = SessionConfig::default();
    let (session, _) = typed("vhelp", &cfg, &env);
    assert!(!session.is_symbol_mode());
    assert_eq!(
        session.decoded().candidates[0].text,
        "vhelp",
        "the input the panel does not serve is echoed, as before"
    );
}

#[test]
fn test_vhelp_space_select_enters_the_named_domain() {
    // Selecting the highlighted domain is not a commit: the panel's input
    // becomes the domain's trigger key, and the list that follows is that
    // domain's symbols through the ordinary panel path.
    let fixture = Fixture::new();
    let table = index_fixture_table();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (mut session, _) = typed("vhelp", &cfg, &env);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);

    assert_eq!(session.buf.raw(), "v1", "the input is the trigger key");
    assert!(session.is_symbol_mode(), "the panel stays open");
    assert_eq!(
        texts_of(&session),
        ["０", "１"],
        "the digits domain is served"
    );
    assert!(
        committed(&effects).is_none(),
        "a domain is a place the panel goes, not text the user typed"
    );
}

#[test]
fn test_vhelp_digit_selects_the_domain_candidate_it_names() {
    // On the index page the number keys are selection again -- the input takes
    // no code to build -- so digit 2 selects the second domain and lands in the
    // arrows.
    let fixture = Fixture::new();
    let table = index_fixture_table();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (mut session, _) = typed("vhelp", &cfg, &env);
    let _ = session.handle_key(KeyAction::SelectIndex(2), &cfg, &env);

    assert_eq!(session.buf.raw(), "vx", "the second domain's trigger key");
    assert_eq!(texts_of(&session), ["←"], "the arrows domain is served");
}

#[test]
fn test_vhelp_leaves_a_prefix_category_served_and_opens_on_the_exact_code() {
    // The shipped table's `h` is a prefix of `help`, so `vh` keeps serving the
    // sequence row; only the exact code opens the page, which is why the page
    // is reserved ahead of the prefix walk.
    let mut table = SymbolTable::parse("h\t序号\tⓐⓑ\n").expect("rows are usable");
    let _ = table.add_emoji_domain("😀\txiao\n");
    let fixture = Fixture::new();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();

    let (session, _) = typed("vh", &cfg, &env);
    assert!(session.is_symbol_mode());
    assert_eq!(session.decoded().candidates[0].text, "ⓐ");
    assert_eq!(
        session.decoded().candidates[0].annotation.as_deref(),
        Some("序号")
    );

    let (session, _) = typed("vhelp", &cfg, &env);
    assert_eq!(
        texts_of(&session),
        ["序号", "表情"],
        "the exact code opens the page"
    );
}

#[test]
fn test_emoji_alias_filter_serves_matching_entries_and_learns_nothing() {
    // `ve` serves the whole domain; the suffix narrows it to the entries whose
    // alias starts with what was typed, homophones side by side in file order.
    // A panel commit is chosen, not spelled, so nothing is recorded either way.
    let fixture = Fixture::new();
    let table = index_fixture_table();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();

    let (session, _) = typed("ve", &cfg, &env);
    assert_eq!(texts_of(&session), ["😀", "😊"]);
    assert_eq!(
        session.decoded().candidates[0].annotation.as_deref(),
        Some("表情")
    );

    let (session, _) = typed("vex", &cfg, &env);
    assert_eq!(
        texts_of(&session),
        ["😀"],
        "only the entry whose alias starts with x"
    );

    let (mut session, _) = typed("vex", &cfg, &env);
    let effects = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    assert_eq!(committed(&effects), Some("😀"));
    assert!(
        session.take_pending().is_none(),
        "a panel candidate is chosen, not spelled"
    );
}

#[test]
fn test_index_page_pages_through_its_domains() {
    // The page rides the candidate grid: with more domains than a page holds,
    // the page keys turn the page like any list's, and the selection that
    // follows still lands in the domain the highlight names.
    let mut document = String::new();
    for code in 'a'..='k' {
        document.push_str(code.encode_utf8(&mut [0u8; 4]));
        document.push_str("\t域\t※\n");
    }
    let table = SymbolTable::load(&document).0;
    let fixture = Fixture::new();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (mut session, _) = typed("vhelp", &cfg, &env);
    assert_eq!(session.decoded().candidates.len(), 11);

    let effects = session.handle_key(KeyAction::PageNext, &cfg, &env);
    let frame = frame_of(&effects).expect("a page turn re-sends the frame");
    assert!(!frame.candidates.is_empty(), "the next page holds domains");
    assert_eq!(frame.page.current, 2, "the turn landed on the second page");

    // The jump lands in the domain the highlight names, whatever candidate of
    // the second page the turn left it on; the annotation is that domain's
    // trigger key, and the input after the jump is exactly it.
    let page_key = session.decoded().candidates[usize::from(session.paging.highlight)]
        .annotation
        .clone()
        .expect("the trigger key");
    let _ = session.handle_key(KeyAction::CommitHighlighted, &cfg, &env);
    assert!(
        session.is_symbol_mode(),
        "a domain selected from the second page opens it"
    );
    assert_eq!(
        session.buf.raw(),
        page_key,
        "the jump landed in the highlighted domain"
    );
}

#[test]
fn test_large_category_pages_without_a_decode() {
    // A category past the first supply rung pages out of the list it was
    // written with: the panel materializes its whole list, and the wider-rung
    // decode the ordinary list would ask for must never replace the symbols on
    // screen with decoded words.
    let mut document = String::from("1\t数字\t");
    for _ in 0..30 {
        document.push('０');
    }
    document.push('\n');
    let table = SymbolTable::load(&document).0;
    let fixture = Fixture::new();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (mut session, _) = typed("v1", &cfg, &env);
    assert_eq!(session.decoded().candidates.len(), 30);

    let effects = session.handle_key(KeyAction::PageLast, &cfg, &env);
    let frame = frame_of(&effects).expect("a page jump re-sends the frame");
    assert_eq!(
        session.decoded().candidates.len(),
        30,
        "the list is the panel's own, not a decode's"
    );
    assert!(
        session
            .decoded()
            .candidates
            .iter()
            .all(|candidate| candidate.source == CandidateSource::Symbol),
        "no decoded candidate joined the list"
    );
    assert!(!frame.candidates.is_empty());
    assert_eq!(session.state, SessionState::Composing);
}

#[test]
fn test_backspace_off_the_index_code_falls_through_to_the_decode() {
    // The pages are one input apart: backing a domain off reaches the bare
    // prefix, and stepping onto the exact code opens the page -- while one
    // letter past it (`vhel`, which no category prefixes in this table) is the
    // decode's again, exactly as the panel treats every input it does not
    // serve.
    let fixture = Fixture::new();
    let table = index_fixture_table();
    let env = fixture.env_with_symbols(&table);
    let cfg = SessionConfig::default();
    let (mut session, _) = typed("vx", &cfg, &env);
    let _ = session.handle_key(KeyAction::Backspace, &cfg, &env);
    assert_eq!(session.buf.raw(), "v");
    assert!(
        session.decoded().candidates.is_empty(),
        "the bare v lists nothing"
    );
    for ch in "help".chars() {
        let _ = session.handle_key(KeyAction::InputChar(ch), &cfg, &env);
    }
    assert_eq!(session.buf.raw(), "vhelp");
    assert_eq!(
        texts_of(&session),
        ["数字", "箭头", "表情"],
        "the exact code opens the page"
    );
    let _ = session.handle_key(KeyAction::Backspace, &cfg, &env);
    assert_eq!(session.buf.raw(), "vhel");
    assert!(
        !session.is_symbol_mode(),
        "a code nothing serves hands the input back to the decode"
    );
    assert_eq!(
        session.decoded().candidates[0].text,
        "vhel",
        "the input the panel gave up is echoed, as before it existed"
    );
    let _ = step(&mut session, SessionEvent::FocusLost, &cfg, &env);
}
