use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::num::NonZeroU32;

use time::{Date, PrimitiveDateTime};

use crate::names::{CatalogLabel, NameError, PortableComponent};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DatedCollectionRange {
    start: Date,
    end: Date,
}

impl DatedCollectionRange {
    pub fn new(start: Date, end: Date) -> Result<Self, CatalogError> {
        validate_canonical_year(start)?;
        validate_canonical_year(end)?;
        if end < start {
            return Err(CatalogError::RangeEndsBeforeStart);
        }
        Ok(Self { start, end })
    }

    pub fn start(&self) -> Date {
        self.start
    }

    pub fn end(&self) -> Date {
        self.end
    }

    pub fn contains(&self, date: Date) -> bool {
        self.start <= date && date <= self.end
    }

    pub fn folder_name(&self, label: &CatalogLabel) -> Result<PortableComponent, CatalogError> {
        let span_days = (self.end - self.start).whole_days();
        let value = if span_days == 0 {
            format!("{} {label}", self.start)
        } else {
            format!("{} +{span_days}d {label}", self.start)
        };
        Ok(PortableComponent::new(value)?)
    }
}

pub fn year_directory(date: Date) -> Result<PortableComponent, CatalogError> {
    validate_canonical_year(date)?;
    Ok(PortableComponent::new(format!("{:04}", date.year()))?)
}

pub fn ongoing_collection_directory(
    label: &CatalogLabel,
) -> Result<PortableComponent, CatalogError> {
    Ok(PortableComponent::new(format!("z {label}"))?)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalStem {
    datetime: PrimitiveDateTime,
    counter: NonZeroU32,
}

impl CanonicalStem {
    /// Builds a canonical stem after the caller validates collection membership.
    pub fn new(datetime: PrimitiveDateTime, counter: NonZeroU32) -> Result<Self, CatalogError> {
        validate_canonical_year(datetime.date())?;
        Ok(Self { datetime, counter })
    }

    pub fn datetime(&self) -> PrimitiveDateTime {
        self.datetime
    }

    pub fn counter(&self) -> NonZeroU32 {
        self.counter
    }
}

impl fmt::Display for CanonicalStem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let date = self.datetime.date();
        let time = self.datetime.time();
        write!(
            formatter,
            "{date} {:02}-{:02}-{:02} {:03}",
            time.hour(),
            time.minute(),
            time.second(),
            self.counter
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalFilename(PortableComponent);

impl CanonicalFilename {
    pub fn new(stem: &CanonicalStem, extension: &str) -> Result<Self, CatalogError> {
        validate_extension(extension)?;
        Ok(Self(PortableComponent::new(format!("{stem}.{extension}"))?))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Display for CanonicalFilename {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourcePathOrderKey(Vec<String>);

impl SourcePathOrderKey {
    pub fn new<I, S>(components: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(components.into_iter().map(Into::into).collect())
    }

    pub fn components(&self) -> &[String] {
        &self.0
    }
}

impl Ord for SourcePathOrderKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for SourcePathOrderKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewItemCounterRequest {
    pub item_id: String,
    pub source_path_order_key: SourcePathOrderKey,
    pub accepted_datetime: PrimitiveDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CounterAllocation {
    pub item_id: String,
    pub accepted_datetime: PrimitiveDateTime,
    pub counter: NonZeroU32,
}

/// Allocates counters for logical items in one collection directory.
///
/// Compound originals and their sidecars must be represented by one request.
/// `existing` must contain every occupied counter for each requested date.
/// Results use accepted datetime, source path, and item identifier order.
pub fn allocate_collection_counters(
    existing: &BTreeMap<Date, BTreeSet<NonZeroU32>>,
    requests: impl IntoIterator<Item = NewItemCounterRequest>,
) -> Result<Vec<CounterAllocation>, CatalogError> {
    let mut requests: Vec<_> = requests.into_iter().collect();
    let mut item_ids = BTreeSet::new();
    for request in &requests {
        validate_canonical_year(request.accepted_datetime.date())?;
        if !item_ids.insert(request.item_id.as_str()) {
            return Err(CatalogError::DuplicateItemId(request.item_id.clone()));
        }
    }

    requests.sort_by(|left, right| {
        left.accepted_datetime
            .cmp(&right.accepted_datetime)
            .then_with(|| left.source_path_order_key.cmp(&right.source_path_order_key))
            .then_with(|| left.item_id.as_bytes().cmp(right.item_id.as_bytes()))
    });

    let mut highest_by_date: BTreeMap<Date, u32> = existing
        .iter()
        .map(|(date, counters)| {
            (
                *date,
                counters
                    .iter()
                    .next_back()
                    .map_or(0, |counter| counter.get()),
            )
        })
        .collect();

    requests
        .into_iter()
        .map(|request| {
            let date = request.accepted_datetime.date();
            let highest = highest_by_date.entry(date).or_default();
            *highest = highest
                .checked_add(1)
                .ok_or(CatalogError::CounterOverflow { date })?;
            let counter =
                NonZeroU32::new(*highest).expect("a positive counter follows checked addition");
            Ok(CounterAllocation {
                item_id: request.item_id,
                accepted_datetime: request.accepted_datetime,
                counter,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DateChangeCounterRequest {
    pub item_id: String,
    pub source_path_order_key: SourcePathOrderKey,
    pub current_counter: NonZeroU32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DateChangeCounterAllocation {
    pub item_id: String,
    pub counter: NonZeroU32,
}

/// Allocates counters for a batch moving to one date in one collection directory.
///
/// `existing` must contain every occupied target-date counter except the moving items.
/// Clock-only changes do not use this function because they preserve their counters.
/// Results use source path and item identifier order.
pub fn allocate_date_change_counters(
    existing: &BTreeSet<NonZeroU32>,
    requests: impl IntoIterator<Item = DateChangeCounterRequest>,
    new_date: Date,
) -> Result<Vec<DateChangeCounterAllocation>, CatalogError> {
    validate_canonical_year(new_date)?;
    let mut requests: Vec<_> = requests.into_iter().collect();
    let mut item_ids = BTreeSet::new();
    for request in &requests {
        if !item_ids.insert(request.item_id.as_str()) {
            return Err(CatalogError::DuplicateItemId(request.item_id.clone()));
        }
    }

    requests.sort_by(|left, right| {
        left.source_path_order_key
            .cmp(&right.source_path_order_key)
            .then_with(|| left.item_id.as_bytes().cmp(right.item_id.as_bytes()))
    });

    let mut allocated = existing.clone();
    let mut counters_by_item = BTreeMap::new();
    for request in &requests {
        if allocated.insert(request.current_counter) {
            counters_by_item.insert(request.item_id.clone(), request.current_counter);
        }
    }

    let mut highest = allocated
        .iter()
        .next_back()
        .map_or(0, |counter| counter.get());
    for request in &requests {
        if counters_by_item.contains_key(request.item_id.as_str()) {
            continue;
        }

        highest = highest
            .checked_add(1)
            .ok_or(CatalogError::CounterOverflow { date: new_date })?;
        let counter =
            NonZeroU32::new(highest).expect("a positive counter follows checked addition");
        allocated.insert(counter);
        counters_by_item.insert(request.item_id.clone(), counter);
    }

    requests
        .into_iter()
        .map(|request| {
            Ok(DateChangeCounterAllocation {
                counter: counters_by_item
                    .remove(request.item_id.as_str())
                    .expect("every validated request receives a counter"),
                item_id: request.item_id,
            })
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogError {
    Name(NameError),
    RangeEndsBeforeStart,
    InvalidExtension,
    YearOutOfRange { year: i32 },
    DuplicateItemId(String),
    CounterOverflow { date: Date },
}

impl fmt::Display for CatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(error) => error.fmt(formatter),
            Self::RangeEndsBeforeStart => {
                formatter.write_str("dated collection range must not end before it starts")
            }
            Self::InvalidExtension => {
                formatter.write_str("extension must be a non-empty portable suffix")
            }
            Self::YearOutOfRange { year } => {
                write!(
                    formatter,
                    "canonical names require a four-digit year: {year}"
                )
            }
            Self::DuplicateItemId(item_id) => {
                write!(formatter, "logical item appears more than once: {item_id}")
            }
            Self::CounterOverflow { date } => {
                write!(formatter, "counter namespace is exhausted for {date}")
            }
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<NameError> for CatalogError {
    fn from(error: NameError) -> Self {
        Self::Name(error)
    }
}

fn validate_extension(extension: &str) -> Result<(), CatalogError> {
    if extension.is_empty()
        || extension.starts_with('.')
        || extension.chars().any(|character| {
            character.is_control()
                || character.is_whitespace()
                || matches!(
                    character,
                    '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                )
        })
    {
        return Err(CatalogError::InvalidExtension);
    }
    Ok(())
}

fn validate_canonical_year(date: Date) -> Result<(), CatalogError> {
    if (0..=9999).contains(&date.year()) {
        Ok(())
    } else {
        Err(CatalogError::YearOutOfRange { year: date.year() })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;
    use time::{Date, Month, Time};

    use super::*;

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn datetime(year: i32, month: Month, day: u8, hour: u8) -> PrimitiveDateTime {
        date(year, month, day).with_time(Time::from_hms(hour, 2, 3).unwrap())
    }

    fn path(value: &str) -> SourcePathOrderKey {
        SourcePathOrderKey::new(value.split('/').map(str::to_owned))
    }

    #[test]
    fn range_folder_name_uses_inclusive_additional_days() {
        let range = DatedCollectionRange::new(
            date(2025, Month::December, 31),
            date(2026, Month::January, 2),
        )
        .unwrap();
        let label = CatalogLabel::new("New Year").unwrap();

        assert_eq!(
            range.folder_name(&label).unwrap().as_str(),
            "2025-12-31 +2d New Year"
        );
        assert!(range.contains(date(2026, Month::January, 1)));
    }

    #[test]
    fn year_and_ongoing_collection_directories_are_canonical() {
        let item_date = date(2024, Month::July, 9);
        let label = CatalogLabel::new("Kids").unwrap();

        assert_eq!(year_directory(item_date).unwrap().as_str(), "2024");
        assert_eq!(
            ongoing_collection_directory(&label).unwrap().as_str(),
            "z Kids"
        );
    }

    #[test]
    fn canonical_filename_preserves_extension_spelling() {
        let stem = CanonicalStem::new(
            datetime(2024, Month::July, 9, 5),
            NonZeroU32::new(12).unwrap(),
        )
        .unwrap();

        assert_eq!(
            CanonicalFilename::new(&stem, "JpG").unwrap().as_str(),
            "2024-07-09 05-02-03 012.JpG"
        );
        assert_eq!(
            CanonicalFilename::new(&stem, "e\u{301}xt")
                .unwrap()
                .as_str(),
            "2024-07-09 05-02-03 012.e\u{301}xt"
        );
    }

    #[test]
    fn canonical_names_reject_years_that_are_not_four_digits() {
        let invalid_date = date(-1, Month::July, 9);

        assert_eq!(
            year_directory(invalid_date),
            Err(CatalogError::YearOutOfRange { year: -1 })
        );
        assert_eq!(
            DatedCollectionRange::new(invalid_date, invalid_date),
            Err(CatalogError::YearOutOfRange { year: -1 })
        );
        assert_eq!(
            CanonicalStem::new(
                invalid_date.with_time(Time::MIDNIGHT),
                NonZeroU32::new(1).unwrap()
            ),
            Err(CatalogError::YearOutOfRange { year: -1 })
        );
        assert_eq!(
            allocate_date_change_counters(&BTreeSet::new(), [], invalid_date,),
            Err(CatalogError::YearOutOfRange { year: -1 })
        );
    }

    #[test]
    fn allocation_continues_above_highest_counter_without_filling_gaps() {
        let item_date = date(2024, Month::July, 9);
        let existing = BTreeMap::from([(
            item_date,
            BTreeSet::from([NonZeroU32::new(1).unwrap(), NonZeroU32::new(3).unwrap()]),
        )]);

        let allocations = allocate_collection_counters(
            &existing,
            [NewItemCounterRequest {
                item_id: "logical-item-a".into(),
                source_path_order_key: path("card/DCIM/a.jpg"),
                accepted_datetime: datetime(2024, Month::July, 9, 5),
            }],
        )
        .unwrap();

        assert_eq!(allocations[0].counter.get(), 4);
    }

    #[test]
    fn allocation_orders_paths_by_components() {
        let allocations = allocate_collection_counters(
            &BTreeMap::new(),
            [
                NewItemCounterRequest {
                    item_id: "joined-bytes-first".into(),
                    source_path_order_key: path("a-x/b.jpg"),
                    accepted_datetime: datetime(2024, Month::July, 9, 5),
                },
                NewItemCounterRequest {
                    item_id: "component-order-first".into(),
                    source_path_order_key: path("a/b.jpg"),
                    accepted_datetime: datetime(2024, Month::July, 9, 5),
                },
            ],
        )
        .unwrap();

        assert_eq!(allocations[0].item_id, "component-order-first");
        assert_eq!(allocations[0].counter.get(), 1);
    }

    #[test]
    fn source_order_keys_accept_nonportable_source_names() {
        let key = SourcePathOrderKey::new(["CON", "photo:one?.jpg"]);

        assert_eq!(key.components(), ["CON", "photo:one?.jpg"]);
    }

    #[test]
    fn date_change_preserves_free_counter_and_allocates_on_collision() {
        let existing = BTreeSet::from([NonZeroU32::new(2).unwrap(), NonZeroU32::new(7).unwrap()]);

        let allocations = allocate_date_change_counters(
            &existing,
            [
                DateChangeCounterRequest {
                    item_id: "free".into(),
                    source_path_order_key: path("a.jpg"),
                    current_counter: NonZeroU32::new(3).unwrap(),
                },
                DateChangeCounterRequest {
                    item_id: "collision-one".into(),
                    source_path_order_key: path("b.jpg"),
                    current_counter: NonZeroU32::new(2).unwrap(),
                },
                DateChangeCounterRequest {
                    item_id: "collision-two".into(),
                    source_path_order_key: path("c.jpg"),
                    current_counter: NonZeroU32::new(2).unwrap(),
                },
            ],
            date(2024, Month::July, 9),
        )
        .unwrap();

        assert_eq!(
            allocations
                .iter()
                .map(|allocation| allocation.counter.get())
                .collect::<Vec<_>>(),
            [3, 8, 9]
        );
    }

    #[test]
    fn date_change_reserves_free_counters_before_allocating_collisions() {
        let allocations = allocate_date_change_counters(
            &BTreeSet::from([NonZeroU32::new(2).unwrap()]),
            [
                DateChangeCounterRequest {
                    item_id: "collision".into(),
                    source_path_order_key: path("a.jpg"),
                    current_counter: NonZeroU32::new(2).unwrap(),
                },
                DateChangeCounterRequest {
                    item_id: "preserved".into(),
                    source_path_order_key: path("b.jpg"),
                    current_counter: NonZeroU32::new(3).unwrap(),
                },
            ],
            date(2024, Month::July, 9),
        )
        .unwrap();

        assert_eq!(
            allocations
                .iter()
                .map(|allocation| allocation.counter.get())
                .collect::<Vec<_>>(),
            [4, 3]
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            rng_seed: RngSeed::Fixed(0x5eed_2024),
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn ranges_contain_exactly_their_closed_interval(start_offset in 0i64..1000, span in 0i64..1000) {
            let epoch = date(2000, Month::January, 1);
            let start = epoch.checked_add(time::Duration::days(start_offset)).unwrap();
            let end = start.checked_add(time::Duration::days(span)).unwrap();
            let range = DatedCollectionRange::new(start, end).unwrap();

            prop_assert!(range.contains(start));
            prop_assert!(range.contains(end));
            if let Some(before) = start.previous_day() {
                prop_assert!(!range.contains(before));
            }
            if let Some(after) = end.next_day() {
                prop_assert!(!range.contains(after));
            }
        }

        #[test]
        fn allocation_is_independent_of_input_order(
            raw in prop::collection::vec((0u8..23, "[a-z]{1,8}"), 1..30)
        ) {
            let mut unique = BTreeMap::new();
            for (hour, path) in raw {
                unique.insert(path, hour);
            }
            let requests: Vec<_> = unique
                .into_iter()
                .map(|(path_value, hour)| NewItemCounterRequest {
                    item_id: path_value.clone(),
                    source_path_order_key: path(&path_value),
                    accepted_datetime: datetime(2024, Month::July, 9, hour),
                })
                .collect();
            let reversed: Vec<_> = requests.iter().cloned().rev().collect();

            let first = allocate_collection_counters(&BTreeMap::new(), requests).unwrap();
            let second = allocate_collection_counters(&BTreeMap::new(), reversed).unwrap();

            prop_assert_eq!(&first, &second);
        }

        #[test]
        fn allocations_never_reuse_existing_or_new_counters(existing_max in 0u32..10_000, count in 1usize..100) {
            let item_date = date(2024, Month::July, 9);
            let existing = if existing_max == 0 {
                BTreeMap::new()
            } else {
                BTreeMap::from([(
                    item_date,
                    BTreeSet::from([NonZeroU32::new(existing_max).unwrap()])
                )])
            };
            let requests = (0..count).map(|index| NewItemCounterRequest {
                item_id: format!("item-{index}"),
                source_path_order_key: path(&format!("{index:03}.jpg")),
                accepted_datetime: datetime(2024, Month::July, 9, 5),
            });

            let allocations = allocate_collection_counters(&existing, requests).unwrap();
            let counters: BTreeSet<_> = allocations.iter().map(|item| item.counter).collect();

            prop_assert_eq!(counters.len(), count);
            prop_assert!(counters.iter().all(|counter| counter.get() > existing_max));
        }

        #[test]
        fn date_change_allocation_is_independent_of_input_order(
            raw in prop::collection::vec((1u32..30, "[a-z]{1,8}"), 1..30)
        ) {
            let mut unique = BTreeMap::new();
            for (counter, path) in raw {
                unique.insert(path, counter);
            }
            let requests: Vec<_> = unique
                .into_iter()
                .map(|(path_value, counter)| DateChangeCounterRequest {
                    item_id: path_value.clone(),
                    source_path_order_key: path(&path_value),
                    current_counter: NonZeroU32::new(counter).unwrap(),
                })
                .collect();
            let reversed: Vec<_> = requests.iter().cloned().rev().collect();
            let existing = BTreeSet::from([NonZeroU32::new(1).unwrap()]);
            let target_date = date(2024, Month::July, 10);

            let first = allocate_date_change_counters(&existing, requests.clone(), target_date).unwrap();
            let second = allocate_date_change_counters(&existing, reversed, target_date).unwrap();

            prop_assert_eq!(&first, &second);
            prop_assert_eq!(
                first.iter().map(|item| item.counter).collect::<BTreeSet<_>>().len(),
                first.len()
            );
            prop_assert!(first.iter().all(|item| !existing.contains(&item.counter)));

            let current_counts = first.iter().fold(BTreeMap::new(), |mut counts, allocation| {
                let request = requests
                    .iter()
                    .find(|request| request.item_id == allocation.item_id)
                    .unwrap();
                *counts.entry(request.current_counter).or_insert(0usize) += 1;
                counts
            });
            for request in &requests {
                if !existing.contains(&request.current_counter)
                    && current_counts[&request.current_counter] == 1
                {
                    let allocation = first
                        .iter()
                        .find(|allocation| allocation.item_id == request.item_id)
                        .unwrap();
                    prop_assert_eq!(allocation.counter, request.current_counter);
                }
            }
        }
    }
}
