use super::*;

use ime_dict::format::reader::Reader;

/// A scratch directory unique to this test process and tag.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rspinyin-dictc-{tag}-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("creating the scratch directory");
    dir
}

/// The budget document a scratch repository root needs.
///
/// The compiler reads the section ceilings it enforces from `docs/dev/budgets.json`
/// relative to the root it was given, so a build rooted anywhere else needs a document
/// of its own. The schema refuses one with a missing key, so this is the whole shape;
/// the values are ones no assertion here depends on, and the only one that matters is
/// `size_mb.base_dict`, which the container ceiling is derived from.
const BUDGETS_FIXTURE: &str = r#"{
  "version": 1,
  "source": "fixture",
  "latency_ms": {"key_to_present_p50": 1.0, "key_to_present_p99": 1.0, "key_to_present_p99_144hz": 1.0, "decode_p99": 1.0, "decode_p999": 1.0, "raster_p99": 1.0, "first_key_to_visible_p99": 1.0, "addon_load": 1.0},
  "memory_mb": {"ui_rss": 1.0, "plugin_rss": 1.0, "dict_mmap_rss": 1.0},
  "cpu_pct": {"idle": 1.0, "typing_10cps": 1.0, "idle_redraw_count": 0, "idle_poll_timer_count": 0},
  "size_mb": {"so_stripped": 1.0, "base_dict": 20.0},
  "alloc_count": {"decode_steady": 1},
  "robustness": {"soak_hours": 1.0, "rss_drift_mb": 1.0, "pass_rate_pct": 1.0},
  "bench": {"passthrough_classify_ns": 1.0, "input_buffer_ops_us": 1.0, "decode_holdout_s": 1.0, "ui_wakeup_latency_us": 1.0, "post_ui_ns": 1.0},
  "net_sockets": 0
}"#;

/// The L1 character table of the scratch repository.
const FIXTURE_L1: &str = "中\tzhong\n国\tguo\n银\tyin\n行\txing,hang\n心\txin\n";

/// The word list of the scratch repository, which is also its frequency source: at
/// production scale the two are one file, and the rows' weights place the band.
///
/// Three rows a compiler can use and one it cannot: `行` states a reading that is not
/// a syllable sequence, which is the row the build has to count and skip rather than
/// fail on.
const FIXTURE_WORDS: &str = "中国\t\t5000\n银行\t\t4000\n中心\t\t3000\n行\tzzz\t10\n";

/// The correction table of the scratch repository: the reading `银行` actually has.
const FIXTURE_POLYPHONE: &str = "银行\tyin'hang\n";

/// Writes a scratch repository root holding the four files the compiler reads, and
/// returns it.
///
/// The root carries its own allowlist, raw sources and budget document, so a test can
/// drive the whole command -- allowlist gate, parse, expand, correct, build, account,
/// write -- and still depend on no committed data. The root is rebuilt from nothing on
/// every call, so a leftover from an interrupted run cannot be mistaken for something
/// this one wrote.
fn scratch_repository(tag: &str) -> PathBuf {
    let root = scratch(tag);
    // A missing directory is the ordinary case and not a failure; a stale one would be.
    let _ = fs::remove_dir_all(&root);
    let raw = root.join("data/raw");
    let docs = root.join("docs/dev");
    fs::create_dir_all(&raw).expect("creating data/raw");
    fs::create_dir_all(&docs).expect("creating docs/dev");
    fs::write(raw.join("pinyin-data.tsv"), FIXTURE_L1).expect("writing the L1 source");
    fs::write(raw.join("jieba-dict.tsv"), FIXTURE_WORDS)
        .expect("writing the word and frequency source");
    fs::write(raw.join("polyphone.tsv"), FIXTURE_POLYPHONE).expect("writing the correction table");
    fs::write(docs.join("budgets.json"), BUDGETS_FIXTURE).expect("writing the budgets");

    // The two upstream sources are hash-pinned exactly as the real allowlist pins the
    // fetched files; the derived one carries no pin, because the project generates it
    // and it changes whenever the generator does.
    let allowlist = format!(
        "[[source]]\nid = \"pinyin-data\"\nkind = \"upstream\"\nlayer = \"L1\"\n\
             url = \"https://example.invalid/pinyin-data\"\nlicense = \"MIT\"\nspdx = \"MIT\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"{}\"\npermissive = true\n\n\
             [[source]]\nid = \"jieba-dict\"\nkind = \"upstream\"\nlayer = \"L2\"\n\
             url = \"https://example.invalid/jieba-dict\"\nlicense = \"MIT\"\nspdx = \"MIT\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"{}\"\npermissive = true\n\n\
             [[source]]\nid = \"polyphone\"\nkind = \"derived\"\nlayer = \"L3c\"\n\
             url = \"https://example.invalid/polyphone\"\nlicense = \"Project-owned\"\n\
             spdx = \"MIT OR Apache-2.0\"\npermissive = true\n",
        hex(&Sha256::digest(FIXTURE_L1.as_bytes())),
        hex(&Sha256::digest(FIXTURE_WORDS.as_bytes())),
    );
    fs::write(root.join("data/sources.toml"), allowlist).expect("writing the allowlist");
    root
}

/// The arguments one scratch build uses, rooted at `root`.
///
/// The band is asked for two entries of the frequency source, which puts the words
/// weighted 4000 and above inside it. The growth ceiling is raised because the fixture
/// is three words and its expansion moves the key count far more than a real word
/// list's does; the ceiling a release build is held to is asserted by the compiler's
/// own growth test.
fn scratch_args(root: &Path) -> DictcArgs {
    DictcArgs {
        root: Some(root.to_path_buf()),
        input: PathBuf::from(DEFAULT_INPUT),
        output: PathBuf::from(DEFAULT_OUTPUT),
        polyphone: PathBuf::from(DEFAULT_POLYPHONE),
        l1: PathBuf::from(DEFAULT_L1),
        l4: PathBuf::from(DEFAULT_L4),
        sources: PathBuf::from(DEFAULT_SOURCES),
        expand_top: 2,
        expand_cap: 4,
        expand_min_weight: None,
        max_key_growth: 500.0,
        tail_flat_weight: 100,
        core_min_freq: 20,
        synth: None,
    }
}

#[test]
fn test_run_compiles_a_reproducible_container_the_reader_accepts() {
    let root = scratch_repository("run-compile");
    let output = root.join(DEFAULT_OUTPUT);

    run(scratch_args(&root)).expect("the first build");
    let first = fs::read(&output).expect("reading the first container");
    run(scratch_args(&root)).expect("the second build");
    let second = fs::read(&output).expect("reading the second container");

    // Two runs over one input have to produce one container: `base.dict` is a shipped
    // artifact, and its digest is what a release records and what the update path
    // compares.
    assert_eq!(first, second, "the container must be reproducible");
    assert_eq!(hex(&Sha256::digest(&first)), hex(&Sha256::digest(&second)));
    assert_eq!(&first[0..4], b"RSPD", "the image is a container");

    // And what the compiler wrote has to be what the read path accepts with every
    // checksum verified, which is the contract the container exists to keep. The row
    // whose reading is not a syllable was counted and skipped rather than compiled.
    let reader = Reader::open(&output).expect("opening the compiled container");
    assert_eq!(reader.entry_count(), 3);
    assert_eq!(reader.key_words("zhong'guo").expect("lookup"), vec!["中国"]);
    // The row states no reading, so the baseline comes from the L1 table (`yin'xing`)
    // and the correction table adds the reading the word actually has.
    assert_eq!(reader.key_words("yin'xing").expect("lookup"), vec!["银行"]);
    assert_eq!(reader.key_words("yin'hang").expect("lookup"), vec!["银行"]);
    assert!(
        reader.key_words("xing").expect("lookup").is_empty(),
        "the skipped row left no key behind"
    );

    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_run_refuses_a_source_the_allowlist_does_not_register() {
    let root = scratch_repository("run-allowlist");
    // A file the allowlist never named, handed to the compiler as its word list.
    fs::write(root.join("data/raw/rogue.tsv"), FIXTURE_WORDS)
        .expect("writing the unregistered source");
    let mut args = scratch_args(&root);
    args.input = PathBuf::from("data/raw/rogue.tsv");

    let refusal = run(args).expect_err("an unregistered source must stop the build");
    assert!(
        refusal.to_string().contains("rogue"),
        "the refusal names the source: {refusal}"
    );
    assert!(
        !root.join(DEFAULT_OUTPUT).exists(),
        "nothing may be written before the gate passes"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

#[test]
fn test_run_refuses_a_source_whose_pinned_hash_does_not_match() {
    let root = scratch_repository("run-pin");
    // Rewriting a pinned source after the allowlist was written is what the pin is
    // for: the bytes the build would consume are no longer the bytes that were
    // reviewed, and a dictionary cannot be un-mixed once it has been compiled.
    fs::write(root.join("data/raw/pinyin-data.tsv"), "中\tzhong\n")
        .expect("rewriting the pinned source");

    let refusal = run(scratch_args(&root)).expect_err("a changed source must stop the build");
    assert!(
        refusal.to_string().contains("sha256 mismatch"),
        "the refusal names the pin: {refusal}"
    );
    fs::remove_dir_all(&root).expect("cleaning up");
}

/// One allowlist entry.
fn source(id: &str, kind: &str, sha256: &str) -> Source {
    Source {
        id: id.to_owned(),
        kind: kind.to_owned(),
        spdx: "MIT".to_owned(),
        license: "MIT".to_owned(),
        url: "https://example.invalid".to_owned(),
        retrieved: "2026-09-29".to_owned(),
        sha256: sha256.to_owned(),
        permissive: true,
        layer: "L4".to_owned(),
    }
}

#[test]
fn test_check_source_refuses_unregistered_and_mismatched_sources() {
    let dir = scratch("gate");
    let registered = dir.join("base.tsv");
    fs::write(&registered, "中国\n").expect("writing the fixture");
    let unregistered = dir.join("sogou.tsv");
    fs::write(&unregistered, "中国\n").expect("writing the fixture");

    let allowlist = Allowlist {
        source: vec![
            source("base", "derived", ""),
            source("pinyin-data", "upstream", &"0".repeat(64)),
        ],
    };

    check_source(&allowlist, &registered).expect("a derived source needs no hash");
    let refusal = check_source(&allowlist, &unregistered).expect_err("unregistered source");
    assert!(
        refusal.to_string().contains("sogou"),
        "the refusal names the source: {refusal}"
    );
    let missing = check_source(&allowlist, &dir.join("pinyin-data.tsv"))
        .expect_err("a missing pinned file must fail");
    assert!(
        missing.to_string().contains("dict/source/missing"),
        "a source that was never fetched reports the fetch diagnostic: {missing}"
    );
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_check_source_refuses_a_hash_that_does_not_match() {
    let dir = scratch("hash");
    let path = dir.join("pinyin-data.tsv");
    fs::write(&path, "中\tzhong\n").expect("writing the fixture");
    let digest = hex(&Sha256::digest("中\tzhong\n".as_bytes()));
    let matching = Allowlist {
        source: vec![source("pinyin-data", "upstream", &digest)],
    };
    check_source(&matching, &path).expect("the pinned hash matches");

    let wrong = Allowlist {
        source: vec![source("pinyin-data", "upstream", &"a".repeat(64))],
    };
    let failure = check_source(&wrong, &path).expect_err("a wrong hash must be refused");
    assert!(failure.to_string().contains("sha256 mismatch"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_allowlist_refuses_a_copyleft_source() {
    let dir = scratch("allow");
    let path = dir.join("sources.toml");
    fs::write(
        &path,
        "[[source]]\nid = \"luna\"\nkind = \"upstream\"\nspdx = \"LGPL-3.0\"\n\
             license = \"LGPL-3.0\"\nurl = \"https://example.invalid\"\n\
             retrieved = \"2026-09-29\"\nsha256 = \"abc\"\npermissive = false\n\
             layer = \"L2\"\n",
    )
    .expect("writing the fixture");
    let failure = load_allowlist(&path).expect_err("a copyleft source must be refused");
    assert!(failure.to_string().contains("luna"), "{failure}");

    fs::write(
        &path,
        "[[source]]\nid = \"base\"\nkind = \"derived\"\nspdx = \"MIT OR Apache-2.0\"\n\
             license = \"Project-owned\"\nurl = \"https://example.invalid\"\npermissive = true\n\
             layer = \"L2\"\n",
    )
    .expect("writing the fixture");
    let allowlist = load_allowlist(&path).expect("a derived source needs no pin");
    assert_eq!(allowlist.source.len(), 1);
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_load_allowlist_requires_a_pin_once_the_source_is_fetched() {
    // The allowlist lives at `<root>/data/sources.toml`, so the fixture root needs
    // the `data/` level for the raw directory to resolve against.
    let dir = scratch("pin");
    let data = dir.join("data");
    let raw = data.join("raw");
    fs::create_dir_all(&raw).expect("creating data/raw");
    let path = data.join("sources.toml");

    // A source registered ahead of its fetch carries an empty pin by design: the
    // entry says "this is where the data will come from", and CI stays green until
    // the download lands.
    fs::write(
        &path,
        "[[source]]\nid = \"jieba-dict\"\nkind = \"upstream\"\nspdx = \"MIT\"\n\
             license = \"MIT\"\nurl = \"https://example.invalid\"\npermissive = true\n\
             layer = \"L4\"\n",
    )
    .expect("writing the fixture");
    let allowlist = load_allowlist(&path).expect("an unfetched upstream source needs no pin");
    assert_eq!(allowlist.source.len(), 1);

    // The moment the file exists the pin becomes mandatory: the bytes the build
    // would consume are no longer hypothetical, and an unpinned fetch cannot pass.
    fs::write(raw.join("jieba-dict.tsv"), "中国\t500\n").expect("writing the raw source");
    let failure = load_allowlist(&path).expect_err("a fetched source must be pinned");
    assert!(failure.to_string().contains("sha256"), "{failure}");
    fs::remove_dir_all(&dir).expect("cleaning up");
}

#[test]
fn test_growth_percent_and_the_guard_boundary() {
    assert_eq!(growth_percent(0, 0), 0.0);
    assert_eq!(growth_percent(100, 100), 0.0);
    assert_eq!(growth_percent(115, 100), 15.0);
    assert_eq!(growth_percent(130, 100), 30.0);
    assert!(growth_percent(131, 100) > 30.0);
}

#[test]
fn test_hex_and_human_size_render_for_the_log() {
    assert_eq!(hex(&[0x00, 0x0F, 0xFF]), "000fff");
    assert_eq!(human_size(512), "0.5KiB");
    assert_eq!(human_size(2 * 1024 * 1024), "2.00MiB");
}
