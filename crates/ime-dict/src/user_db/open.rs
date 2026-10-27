//! Opening a user store: the constructor family and every way one is configured.
//!
//! Split out of `user_db.rs` to keep that file inside the line limit. The constructors
//! are a ladder of defaults -- the plain [`UserDb::open`] a caller nearly always wants,
//! then the seams tests and decay tuning need -- and each rung is the one below it plus
//! one replaced default, so a reader can stop at the first rung that fits.

use super::*;

impl UserDb {
    /// Opens the store at `path`, creating it when it does not exist.
    ///
    /// # Errors
    /// Returns [`ImeError::DictUnavailable`] when the directory cannot be prepared, when
    /// the file cannot be created with mode [`FILE_MODE`], when the store refuses the file
    /// (a second handle on the same path, a corrupt image), or when the file carries a
    /// schema version this build does not know.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ImeError> {
        Self::open_with(path, Box::new(SystemClock::new()))
    }

    /// Opens the store at `path` with an injected clock.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with(path: impl AsRef<Path>, clock: Box<dyn Clock>) -> Result<Self, ImeError> {
        Self::open_with_hydrate_cap(path, clock, HYDRATE_CAP)
    }

    /// Opens the store at `path` with a clock and decay policy supplied by the caller.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_decay(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        decay: DecayParams,
    ) -> Result<Self, ImeError> {
        Self::open_with_hydrate_cap_and_decay(path, clock, HYDRATE_CAP, decay)
    }

    /// Opens the store at `path` with an injected clock and hydration ceiling.
    ///
    /// The ceiling is a parameter for the same reason the clock is: the behaviour of a
    /// store too large to load is worth asserting, and no test may build a
    /// fifty-thousand-record store to reach it. A running plugin opens through
    /// [`UserDb::open`] and so always uses [`HYDRATE_CAP`]; a ceiling of zero loads no
    /// store that holds a record, which is how the fallback path is reached deliberately.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_hydrate_cap(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        hydrate_cap: u64,
    ) -> Result<Self, ImeError> {
        Self::open_with_hydrate_cap_and_decay(path, clock, hydrate_cap, DecayParams::default())
    }

    /// Opens the store with a hydration ceiling and an explicit decay policy.
    ///
    /// The policy is projected from configuration by the caller; it is captured before
    /// the store's threads start, so every later fold uses one coherent policy.
    ///
    /// # Errors
    /// As [`UserDb::open`].
    pub fn open_with_hydrate_cap_and_decay(
        path: impl AsRef<Path>,
        clock: Box<dyn Clock>,
        hydrate_cap: u64,
        decay: DecayParams,
    ) -> Result<Self, ImeError> {
        let path = path.as_ref().to_path_buf();
        prepare_path(&path)?;
        let db = Database::create(&path).map_err(|error| store_error(&path, &error))?;
        let schema = prepare_schema(&path, &db)?;
        let v1_readonly = schema == SchemaOutcome::V1Degraded;
        let loaded = load_committed(&db, &path, hydrate_cap, v1_readonly);
        let is_hydrated = loaded.is_some();
        // The pins ride with the load when the store fits, and are read on their own when
        // it does not: the pinned set is what both the ranking's cap and eviction's
        // protection consult, and neither may go blind because the record count passed
        // the ceiling -- a pin set is a handful of rows, never the reason a store is too
        // large to load.
        let (records, pins) = match loaded {
            Some(loaded) => (loaded.records, (loaded.pinned, loaded.key_pins)),
            None => (HashMap::new(), load_pins(&db, &path, v1_readonly)),
        };
        // The coined words ride their own small load: a read transaction at open, where a
        // read transaction is the caller's own call and not a keystroke's. The map is what
        // `forget` and `is_user_word` consult so that neither has to scan the file later.
        let coined_words = load_coined(&db, &path);
        // A v1 store served read-only carries no clock at all -- its migration, the
        // only thing that would have written one, failed. Zero is the clock a fresh
        // store starts with, and a read-only store never advances it.
        let tick = if v1_readonly {
            0
        } else {
            read_tick(&path, &db)?
        };
        let inner = Arc::new(Inner {
            db,
            is_hydrated,
            decay,
            committed: Mutex::new(records),
            pinned: Mutex::new(pins.0),
            key_pins: Mutex::new(pins.1),
            coined_words: Mutex::new(coined_words),
            tick: AtomicU64::new(tick),
            schema_migrated: matches!(schema, SchemaOutcome::Migrated | SchemaOutcome::V1Degraded),
            v1_readonly,
            pending: Mutex::new(PendingState::default()),
            cache: if is_hydrated {
                None
            } else {
                Some(Mutex::new(MissCache::new(CACHE_CAPACITY)))
            },
            readonly: AtomicBool::new(v1_readonly),
            batch: AtomicUsize::new(COMMIT_BATCH),
            interval_ms: AtomicU64::new(COMMIT_INTERVAL_MS),
            slow_streak: AtomicUsize::new(0),
            last_record_nanos: AtomicU64::new(clock.now_nanos()),
            clock,
            path,
            sweep: OnceLock::new(),
            writer: OnceLock::new(),
        });
        // Both threads reach the store through a weak handle, so neither can keep it alive.
        // Their handles are installed through the locks rather than through `Arc::get_mut`:
        // the weak handles the two threads hold are themselves enough to keep `get_mut`
        // answering `None` forever, which is how an install here used to silently drop both
        // threads and turn every record path into a caller-thread flush. `set` cannot
        // refuse -- the locks are fresh, and only this function, which has not returned yet,
        // can reach them -- so the results are dropped rather than matched.
        let _ = inner.sweep.set(start_sweep(&inner));
        let _ = inner.writer.set(FlushThread::start(&inner));
        Ok(Self { inner })
    }
}
