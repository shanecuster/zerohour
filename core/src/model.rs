//! The data model: what a task, an event, a repeat rule, and a completion record look like.
//!
//! Everything else in the app (database, stats, UI) passes these types around. They are plain
//! data with a few helper methods; the date math lives in `repeat.rs` and storage in `db.rs`.
//!
//! Every `Item` is built to sync between devices later (phase 2): it has a random UUID instead
//! of an auto-increment number, an `updated_at` timestamp that changes on every edit (so the
//! newest edit can win a conflict), and a `deleted` flag instead of really deleting rows (so a
//! deletion can be sent to other devices).

use chrono::{DateTime, Datelike, NaiveDate, NaiveTime, TimeDelta, Utc, Weekday};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// A note on `#[derive(...)]`: it asks the compiler to write common code for us.
// - `Debug`      -> can be printed with `{:?}` (handy in tests and error messages).
// - `Clone`      -> can be duplicated with `.clone()`.
// - `Copy`       -> small types that are copied automatically instead of "moved" (see below).
// - `PartialEq`/`Eq` -> can be compared with `==`.
// - `Serialize`/`Deserialize` -> can be turned into JSON and back (serde), for sync/export.
//
// Ownership in one paragraph: in Rust every value has exactly one owner. Assigning a value to a
// new variable or passing it to a function *moves* it (the old name can't be used anymore),
// unless the type is `Copy` (then it is duplicated) or you pass a *borrow* (`&value`), which
// lends it out temporarily without giving it away.

/// Whether an item is something to *do* (Task) or a date to *count down to* (Event).
///
/// An `enum` is a type that is exactly one of a fixed list of variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    /// Something to do, optionally with a due date. Can be completed.
    Task,
    /// A date to count down to, like a birthday or holiday. Never "completed".
    Event,
}

impl Kind {
    /// The text we store in the database for this kind.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Event => "event",
        }
    }

    /// Reads a kind back from its database text. Returns `None` for unknown text.
    ///
    /// `Option<T>` is Rust's "maybe a value": either `Some(value)` or `None`. There is no null
    /// in Rust, so any "might be missing" value is spelled out as an `Option`.
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "task" => Some(Kind::Task),
            "event" => Some(Kind::Event),
            _ => None, // `_` matches anything not listed above.
        }
    }
}

/// How important a task is. Used for sorting and the priority marker.
///
/// The order of the variants matters: deriving `PartialOrd`/`Ord` makes `Low < Medium < High`,
/// which is what the sort in the UI relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Medium,
    High,
}

impl Priority {
    /// All priorities, lowest first. Handy for cycling through them in the add form.
    pub const ALL: [Priority; 3] = [Priority::Low, Priority::Medium, Priority::High];

    /// The text stored in the database.
    pub fn as_str(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
        }
    }

    /// The text shown to the user ("High", "Medium", "Low").
    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "Low",
            Priority::Medium => "Medium",
            Priority::High => "High",
        }
    }

    /// Reads a priority back from its database text.
    pub fn parse(s: &str) -> Option<Priority> {
        match s {
            "low" => Some(Priority::Low),
            "medium" => Some(Priority::Medium),
            "high" => Some(Priority::High),
            _ => None,
        }
    }
}

/// The unit for a custom "every N ..." repeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Unit {
    Days,
    Weeks,
    Months,
}

impl Unit {
    /// All units, for cycling in the add form.
    pub const ALL: [Unit; 3] = [Unit::Days, Unit::Weeks, Unit::Months];

    /// The text stored in the database (also used in labels).
    pub fn as_str(self) -> &'static str {
        match self {
            Unit::Days => "days",
            Unit::Weeks => "weeks",
            Unit::Months => "months",
        }
    }

    /// Reads a unit back from its database text.
    pub fn parse(s: &str) -> Option<Unit> {
        match s {
            "days" => Some(Unit::Days),
            "weeks" => Some(Unit::Weeks),
            "months" => Some(Unit::Months),
            _ => None,
        }
    }
}

/// A set of weekdays, e.g. Mon–Thu, for the "repeat on these days" rule.
///
/// Stored as 7 bits in one byte: bit 0 = Monday ... bit 6 = Sunday. That makes it `Copy`
/// (cheap to pass around by value) like the rest of `RepeatRule`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DaySet(u8);

/// Monday-first order, used for storage, labels and the bit numbering.
const WEEK: [Weekday; 7] = [
    Weekday::Mon,
    Weekday::Tue,
    Weekday::Wed,
    Weekday::Thu,
    Weekday::Fri,
    Weekday::Sat,
    Weekday::Sun,
];

/// Short lowercase names used in the stored text ("days:mon,tue").
const DB_NAMES: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
/// Names used in labels ("Mon–Thu", "Mon, Wed, Fri").
const LABEL_NAMES: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

impl DaySet {
    /// No days (not a valid rule on its own; the form refuses it).
    pub const EMPTY: DaySet = DaySet(0);
    /// Monday to Friday.
    pub const WEEKDAYS: DaySet = DaySet(0b001_1111);
    /// Monday to Thursday.
    pub const MON_THU: DaySet = DaySet(0b000_1111);
    /// Saturday and Sunday.
    pub const WEEKENDS: DaySet = DaySet(0b110_0000);
    /// All seven days.
    pub const ALL: DaySet = DaySet(0b111_1111);

    /// The bit for a weekday (Monday = bit 0).
    fn bit(day: Weekday) -> u8 {
        1 << day.num_days_from_monday()
    }

    /// True if `day` is in the set.
    pub fn contains(self, day: Weekday) -> bool {
        self.0 & DaySet::bit(day) != 0
    }

    /// The set with `day` switched on or off.
    pub fn toggled(self, day: Weekday) -> DaySet {
        // `^` (XOR) flips just that one bit.
        DaySet(self.0 ^ DaySet::bit(day))
    }

    /// True if no day is selected.
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The first date on or after `from` that falls on one of the days, or `None` if the set
    /// is empty. Never needs to look more than 7 days ahead.
    pub fn first_on_or_after(self, from: NaiveDate) -> Option<NaiveDate> {
        (0..7)
            .map(|offset| from + TimeDelta::days(offset))
            .find(|date| self.contains(date.weekday()))
    }

    /// Encodes as `"days:mon,tue,wed,thu"` (always Monday-first) for the database and sync.
    pub fn to_db_string(self) -> String {
        let names: Vec<&str> = WEEK
            .iter()
            .zip(DB_NAMES)
            .filter(|(day, _)| self.contains(**day))
            .map(|(_, name)| name)
            .collect();
        format!("days:{}", names.join(","))
    }

    /// Decodes the part after `"days:"`. `None` for unknown names or an empty list.
    fn parse_list(list: &str) -> Option<DaySet> {
        let mut set = DaySet::EMPTY;
        for name in list.split(',') {
            let index = DB_NAMES.iter().position(|n| *n == name)?;
            set = DaySet(set.0 | DaySet::bit(WEEK[index]));
        }
        (!set.is_empty()).then_some(set)
    }

    /// A short label: "every day", "weekdays", "weekends", a range like "Mon–Thu" (3 or more
    /// days in a row, Monday-first), or a list like "Mon, Wed, Fri".
    pub fn label(self) -> String {
        match self {
            DaySet::ALL => return "every day".to_string(),
            DaySet::WEEKDAYS => return "weekdays".to_string(),
            DaySet::WEEKENDS => return "weekends".to_string(),
            _ => {}
        }
        let indexes: Vec<usize> = (0..7).filter(|&i| self.contains(WEEK[i])).collect();
        // A run with no gaps: last index - first index + 1 == number of days.
        if let (Some(&first), Some(&last)) = (indexes.first(), indexes.last())
            && indexes.len() >= 3
            && last - first + 1 == indexes.len()
        {
            return format!("{}–{}", LABEL_NAMES[first], LABEL_NAMES[last]);
        }
        indexes
            .iter()
            .map(|&i| LABEL_NAMES[i])
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// How (and whether) an item repeats.
///
/// Variants can carry data: `EveryN { n, unit }` holds a number and a unit, e.g.
/// "every 2 weeks" is `EveryN { n: 2, unit: Unit::Weeks }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatRule {
    None,
    Daily,
    Weekly,
    Monthly,
    Yearly,
    EveryN {
        n: u32,
        unit: Unit,
    },
    /// On chosen days of the week, e.g. Mon–Thu. Never empty.
    Weekdays(DaySet),
}

impl RepeatRule {
    /// True for every rule except `None`.
    pub fn is_repeating(self) -> bool {
        self != RepeatRule::None
    }

    /// Encodes the rule as one short string for the database, e.g. `"yearly"` or `"every:3:days"`.
    ///
    /// A single text column keeps the schema simple; `parse` reverses it.
    pub fn to_db_string(self) -> String {
        match self {
            RepeatRule::None => "none".to_string(),
            RepeatRule::Daily => "daily".to_string(),
            RepeatRule::Weekly => "weekly".to_string(),
            RepeatRule::Monthly => "monthly".to_string(),
            RepeatRule::Yearly => "yearly".to_string(),
            // `format!` builds a String, like printf. `{n}` inserts the variable `n`.
            RepeatRule::EveryN { n, unit } => format!("every:{n}:{}", unit.as_str()),
            RepeatRule::Weekdays(days) => days.to_db_string(),
        }
    }

    /// Decodes a rule from its database string. Returns `None` if the text is not recognized.
    pub fn parse(s: &str) -> Option<RepeatRule> {
        match s {
            "none" => Some(RepeatRule::None),
            "daily" => Some(RepeatRule::Daily),
            "weekly" => Some(RepeatRule::Weekly),
            "monthly" => Some(RepeatRule::Monthly),
            "yearly" => Some(RepeatRule::Yearly),
            // `strip_prefix` returns the rest of the text if it starts with "days:".
            other if other.starts_with("days:") => other
                .strip_prefix("days:")
                .and_then(DaySet::parse_list)
                .map(RepeatRule::Weekdays),
            other => {
                // Expect exactly three pieces: "every", a number, a unit.
                let mut parts = other.split(':');
                if parts.next()? != "every" {
                    return None;
                }
                // `?` on an Option means: "if this is None, stop and return None right now".
                let n: u32 = parts.next()?.parse().ok()?;
                let unit = Unit::parse(parts.next()?)?;
                if parts.next().is_some() {
                    return None; // Extra junk after the unit.
                }
                Some(RepeatRule::EveryN { n, unit })
            }
        }
    }

    /// A short human label for the UI: "yearly", "every 2 weeks", or "" for no repeat.
    pub fn label(self) -> String {
        match self {
            RepeatRule::None => String::new(),
            RepeatRule::Daily => "daily".to_string(),
            RepeatRule::Weekly => "weekly".to_string(),
            RepeatRule::Monthly => "monthly".to_string(),
            RepeatRule::Yearly => "yearly".to_string(),
            // "every 1 days" reads badly, so singular units get special wording.
            RepeatRule::EveryN { n: 1, unit } => match unit {
                Unit::Days => "daily".to_string(),
                Unit::Weeks => "weekly".to_string(),
                Unit::Months => "monthly".to_string(),
            },
            RepeatRule::EveryN { n, unit } => format!("every {n} {}", unit.as_str()),
            RepeatRule::Weekdays(days) => days.label(),
        }
    }
}

/// One row in the main list: either a task or an event (see [`Kind`]).
///
/// Tasks and events share one struct (and one database table) because they have almost the
/// same fields. A field that only makes sense for one kind is simply ignored for the other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    /// Random UUID string. Unique across all devices, which matters for sync.
    pub id: String,
    /// Task or Event.
    pub kind: Kind,
    /// What the user typed as the name.
    pub title: String,
    /// Optional longer notes.
    pub notes: Option<String>,
    /// Importance. Events store it but the UI ignores it.
    pub priority: Priority,
    /// Tasks: the due date (optional). Events: the target date (required).
    ///
    /// A `NaiveDate` is a calendar date with no time and no time zone — "2026-12-25" means
    /// Christmas everywhere. That's exactly what a due date is, and it can't drift by a day when
    /// clocks change for daylight saving time.
    pub date: Option<NaiveDate>,
    /// Tasks only: an optional due *time* on the due date, e.g. 14:00.
    ///
    /// Stored as local wall-clock time, not UTC: "2 PM" means 2 PM wherever you are, like an
    /// alarm clock. Two synced devices in different time zones each show 2 PM local. A time
    /// is only allowed together with a date.
    pub due_time: Option<NaiveTime>,
    /// How the item repeats (or `RepeatRule::None`).
    pub repeat: RepeatRule,
    /// Events only: keep showing a one-off event after its date has passed.
    /// (Repeating events always roll forward to the next occurrence, so this doesn't apply.)
    pub keep_after_pass: bool,
    /// Tasks only: when the task was completed, or `None` if it's still open.
    pub completed_at: Option<DateTime<Utc>>,
    /// When the item was created (UTC).
    pub created_at: DateTime<Utc>,
    /// When the item last changed (UTC). Updated on EVERY edit; sync uses it to pick a winner.
    pub updated_at: DateTime<Utc>,
    /// Soft delete flag: deleted items stay in the database (hidden) so deletions can sync.
    pub deleted: bool,
}

impl Item {
    /// Creates a brand-new item with a fresh UUID and sensible defaults.
    ///
    /// `now` is passed in instead of read from the clock so that tests can control time.
    /// `impl Into<String>` means "anything that can become a String" — so callers can pass
    /// either `"text"` (a `&str`) or an owned `String`.
    pub fn new(kind: Kind, title: impl Into<String>, now: DateTime<Utc>) -> Item {
        Item {
            id: new_id(),
            kind,
            title: title.into(),
            notes: None,
            priority: Priority::Medium,
            date: None,
            due_time: None,
            repeat: RepeatRule::None,
            keep_after_pass: false,
            completed_at: None,
            created_at: now,
            updated_at: now,
            deleted: false,
        }
    }

    /// True if this is a task that has been completed.
    pub fn is_completed(&self) -> bool {
        self.completed_at.is_some()
    }
}

/// One entry in the completion log: "this task was finished at this moment".
///
/// Statistics are computed only from this log. Because we snapshot the title and priority at
/// completion time, stats stay correct even if the task is later edited or deleted, and new
/// kinds of stats never need a database migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Completion {
    /// Random UUID for this log entry.
    pub id: String,
    /// The `Item::id` of the task that was completed.
    pub item_id: String,
    /// When it was completed (UTC).
    pub completed_at: DateTime<Utc>,
    /// The task's title at the time it was completed.
    pub title_snapshot: String,
    /// The task's priority at the time it was completed.
    pub priority_snapshot: Priority,
    /// Whether the task was past its due date when completed.
    pub was_overdue: bool,
}

/// Generates a new random UUID (version 4) as a string, e.g. `"9f1c...-..."`.
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// The fixed "namespace" for zerohour's derived IDs. Any constant UUID works; it just has to
/// never change, or derived IDs made by old and new versions would stop matching.
const ID_NAMESPACE: Uuid = Uuid::from_u128(0xf430a8ff_6bba_4fe2_9d28_f2da9fe70752);

/// What a [`derived_id`] is derived *for*, so the same source id gives different results for
/// different purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DerivedId {
    /// The next instance of a repeating task, derived from the instance that was completed.
    NextInstance,
    /// The completion log entry for a task, derived from the task's id.
    Completion,
}

/// A UUID (version 5) computed from another id instead of picked at random.
///
/// Version 5 UUIDs are a hash of (namespace, name): the same input always gives the same
/// UUID, on every device. That matters for sync: if you complete the same repeating task on
/// two devices while both are offline, both create the *same* next instance and the *same*
/// completion entry, which then merge into one instead of showing up twice.
///
/// It's safe locally because a task can only be completed once, so each source id is only
/// ever used once per purpose.
pub fn derived_id(purpose: DerivedId, source_id: &str) -> String {
    let name = match purpose {
        DerivedId::NextInstance => format!("next-instance:{source_id}"),
        DerivedId::Completion => format!("completion:{source_id}"),
    };
    Uuid::new_v5(&ID_NAMESPACE, name.as_bytes()).to_string()
}

/// The `updated_at` to stamp on an edit: `now`, unless the stored value is already at or past
/// `now`, in which case 1 millisecond after the stored value.
///
/// Why: sync picks the version with the newer `updated_at`. If this device's clock is behind
/// another device's, a version synced from there can carry a timestamp "in the future". Without
/// this rule, your edit made *after* seeing that version would get an older timestamp and lose.
/// With it, an edit always beats the version it was made on top of. On a single device with a
/// normal clock, `now` is always later, so local-only behavior is unchanged.
pub fn next_updated_at(previous: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
    now.max(previous + TimeDelta::milliseconds(1))
}

// `#[cfg(test)]` means "only compile this module when running `cargo test`".
#[cfg(test)]
mod tests {
    use super::*; // Bring everything from the parent module (this file) into scope.

    #[test]
    fn repeat_rule_round_trips_through_db_string() {
        let rules = [
            RepeatRule::None,
            RepeatRule::Daily,
            RepeatRule::Weekly,
            RepeatRule::Monthly,
            RepeatRule::Yearly,
            RepeatRule::EveryN {
                n: 3,
                unit: Unit::Days,
            },
            RepeatRule::EveryN {
                n: 2,
                unit: Unit::Weeks,
            },
            RepeatRule::EveryN {
                n: 6,
                unit: Unit::Months,
            },
        ];
        for rule in rules {
            assert_eq!(RepeatRule::parse(&rule.to_db_string()), Some(rule));
        }
    }

    #[test]
    fn weekday_rules_round_trip_through_db_strings() {
        let set = DaySet::MON_THU;
        assert_eq!(
            RepeatRule::Weekdays(set).to_db_string(),
            "days:mon,tue,wed,thu"
        );
        for set in [
            DaySet::WEEKDAYS,
            DaySet::WEEKENDS,
            DaySet::ALL,
            DaySet(0b101_0101),
        ] {
            let rule = RepeatRule::Weekdays(set);
            assert_eq!(RepeatRule::parse(&rule.to_db_string()), Some(rule));
        }
        assert_eq!(RepeatRule::parse("days:"), None, "empty set");
        assert_eq!(RepeatRule::parse("days:mon,funday"), None);
    }

    #[test]
    fn day_set_labels() {
        assert_eq!(DaySet::WEEKDAYS.label(), "weekdays");
        assert_eq!(DaySet::MON_THU.label(), "Mon–Thu");
        assert_eq!(DaySet::WEEKENDS.label(), "weekends");
        assert_eq!(DaySet::ALL.label(), "every day");
        let mwf = DaySet::EMPTY
            .toggled(Weekday::Mon)
            .toggled(Weekday::Wed)
            .toggled(Weekday::Fri);
        assert_eq!(mwf.label(), "Mon, Wed, Fri");
        assert_eq!(DaySet::EMPTY.toggled(Weekday::Tue).label(), "Tue");
        assert_eq!(
            DaySet::EMPTY
                .toggled(Weekday::Sat)
                .toggled(Weekday::Mon)
                .label(),
            "Mon, Sat"
        );
    }

    #[test]
    fn day_set_finds_the_next_scheduled_day() {
        let thu = NaiveDate::from_ymd_opt(2026, 10, 8).unwrap();
        let mon = NaiveDate::from_ymd_opt(2026, 10, 12).unwrap();
        assert_eq!(DaySet::MON_THU.first_on_or_after(thu), Some(thu));
        assert_eq!(
            DaySet::MON_THU.first_on_or_after(thu + TimeDelta::days(1)),
            Some(mon)
        );
        assert_eq!(DaySet::EMPTY.first_on_or_after(thu), None);
    }

    #[test]
    fn repeat_rule_rejects_garbage() {
        assert_eq!(RepeatRule::parse("fortnightly"), None);
        assert_eq!(RepeatRule::parse("every:x:days"), None);
        assert_eq!(RepeatRule::parse("every:3:years"), None);
        assert_eq!(RepeatRule::parse("every:3:days:extra"), None);
    }

    #[test]
    fn labels_read_naturally() {
        assert_eq!(RepeatRule::Yearly.label(), "yearly");
        assert_eq!(
            RepeatRule::EveryN {
                n: 2,
                unit: Unit::Weeks
            }
            .label(),
            "every 2 weeks"
        );
        assert_eq!(
            RepeatRule::EveryN {
                n: 1,
                unit: Unit::Days
            }
            .label(),
            "daily"
        );
        assert_eq!(RepeatRule::None.label(), "");
    }

    #[test]
    fn derived_ids_are_stable_valid_and_distinct() {
        let source = "9b2d6f0e-3c1a-4b8e-9f00-123456789abc";
        let next = derived_id(DerivedId::NextInstance, source);
        // Same input, same output (this is what lets two devices agree).
        assert_eq!(next, derived_id(DerivedId::NextInstance, source));
        // A real UUID, version 5.
        assert_eq!(Uuid::parse_str(&next).unwrap().get_version_num(), 5);
        // Different purposes and different sources give different ids.
        assert_ne!(next, derived_id(DerivedId::Completion, source));
        assert_ne!(next, derived_id(DerivedId::NextInstance, &new_id()));
        assert_ne!(next, source);
    }

    #[test]
    fn next_updated_at_is_now_unless_the_stored_time_is_ahead() {
        use chrono::TimeZone;
        let now = Utc.with_ymd_and_hms(2026, 10, 8, 12, 0, 0).unwrap();
        let earlier = now - TimeDelta::hours(1);
        let ahead = now + TimeDelta::minutes(5);
        assert_eq!(next_updated_at(earlier, now), now, "normal case: just now");
        assert_eq!(
            next_updated_at(ahead, now),
            ahead + TimeDelta::milliseconds(1)
        );
        assert_eq!(next_updated_at(now, now), now + TimeDelta::milliseconds(1));
    }

    #[test]
    fn priorities_sort_low_to_high() {
        assert!(Priority::Low < Priority::Medium);
        assert!(Priority::Medium < Priority::High);
    }
}
