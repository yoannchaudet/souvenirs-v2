use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use time::{Date, Duration, PrimitiveDateTime, Time, UtcOffset};

use crate::source::CameraIdentity;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DateTimeSource {
    ExifDateTimeOriginal,
    ExifDateTimeDigitized,
    QuickTimeCreationDate,
    XmpDateCreated,
    XmpExifDateTimeOriginal,
    ContainerCreationTime,
    GoogleTakeoutPhotoTakenTime,
    FilenamePattern,
}

impl DateTimeSource {
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::ExifDateTimeOriginal => 1,
            Self::ExifDateTimeDigitized => 2,
            Self::QuickTimeCreationDate => 3,
            Self::XmpDateCreated | Self::XmpExifDateTimeOriginal => 4,
            Self::ContainerCreationTime => 5,
            Self::GoogleTakeoutPhotoTakenTime => 6,
            Self::FilenamePattern => 7,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DateTimeValue {
    Local(PrimitiveDateTime),
    LocalWithOffset {
        local: PrimitiveDateTime,
        offset: UtcOffset,
    },
    UtcWithoutLocalContext(PrimitiveDateTime),
    AmbiguousContainer(PrimitiveDateTime),
    DateOnly(Date),
}

impl DateTimeValue {
    fn comparable_value(&self) -> ComparableDateTimeValue {
        match self {
            Self::Local(value) => ComparableDateTimeValue::Local(truncate_to_seconds(*value)),
            Self::LocalWithOffset { local, .. } => {
                ComparableDateTimeValue::Local(truncate_to_seconds(*local))
            }
            Self::UtcWithoutLocalContext(value) => {
                ComparableDateTimeValue::Utc(truncate_to_seconds(*value))
            }
            Self::AmbiguousContainer(value) => {
                ComparableDateTimeValue::Ambiguous(truncate_to_seconds(*value))
            }
            Self::DateOnly(date) => ComparableDateTimeValue::Date(*date),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ComparableDateTimeValue {
    Local(PrimitiveDateTime),
    Utc(PrimitiveDateTime),
    Ambiguous(PrimitiveDateTime),
    Date(Date),
}

impl ComparableDateTimeValue {
    fn date(&self) -> Date {
        match self {
            Self::Local(value) | Self::Utc(value) | Self::Ambiguous(value) => value.date(),
            Self::Date(date) => *date,
        }
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct DateTimeCandidate {
    pub source: DateTimeSource,
    pub value: DateTimeValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedDateTime {
    pub wall_clock: PrimitiveDateTime,
    pub source: AcceptedDateTimeSource,
    pub recorded_offset: Option<UtcOffset>,
}

impl AcceptedDateTime {
    pub fn apply_clock_offset(&self, offset: Duration) -> Result<Self, DateTimeError> {
        let wall_clock = self
            .wall_clock
            .checked_add(offset)
            .ok_or(DateTimeError::ClockCorrectionOutOfRange)?;
        Ok(Self {
            wall_clock: truncate_to_seconds(wall_clock),
            source: AcceptedDateTimeSource::Corrected {
                original: Box::new(self.source.clone()),
                offset,
            },
            recorded_offset: self.recorded_offset,
        })
    }

    pub fn assigned(wall_clock: PrimitiveDateTime) -> Self {
        Self {
            wall_clock: truncate_to_seconds(wall_clock),
            source: AcceptedDateTimeSource::UserAssigned,
            recorded_offset: None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcceptedDateTimeSource {
    Metadata(DateTimeSource),
    Confirmed(DateTimeSource),
    UserAssigned,
    Corrected {
        original: Box<AcceptedDateTimeSource>,
        offset: Duration,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationReason {
    AmbiguousMetadata,
    Proposal,
    UtcInterpretation,
    MissingClock,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConfirmationRequirement {
    AcceptCandidate,
    ChooseLocalOrUtc,
    SelectUtcOffset,
    AssignClock,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ConfirmationBatchKey {
    pub source: DateTimeSource,
    pub requirement: ConfirmationRequirement,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDateTime {
    pub candidate: DateTimeCandidate,
    pub reason: ConfirmationReason,
    pub disagreements: Vec<DateTimeCandidate>,
}

impl PendingDateTime {
    pub fn batch_key(&self) -> Result<ConfirmationBatchKey, DateTimeError> {
        let requirement = match &self.candidate.value {
            DateTimeValue::Local(_) | DateTimeValue::AmbiguousContainer(_) => match self.reason {
                ConfirmationReason::AmbiguousMetadata => ConfirmationRequirement::ChooseLocalOrUtc,
                ConfirmationReason::Proposal => ConfirmationRequirement::AcceptCandidate,
                ConfirmationReason::UtcInterpretation | ConfirmationReason::MissingClock => {
                    return Err(DateTimeError::InvalidPendingDateTime)
                }
            },
            DateTimeValue::LocalWithOffset { .. } => match self.reason {
                ConfirmationReason::AmbiguousMetadata | ConfirmationReason::Proposal => {
                    ConfirmationRequirement::AcceptCandidate
                }
                ConfirmationReason::UtcInterpretation | ConfirmationReason::MissingClock => {
                    return Err(DateTimeError::InvalidPendingDateTime)
                }
            },
            DateTimeValue::UtcWithoutLocalContext(_) => match self.reason {
                ConfirmationReason::UtcInterpretation => ConfirmationRequirement::SelectUtcOffset,
                ConfirmationReason::AmbiguousMetadata
                | ConfirmationReason::Proposal
                | ConfirmationReason::MissingClock => {
                    return Err(DateTimeError::InvalidPendingDateTime)
                }
            },
            DateTimeValue::DateOnly(_) => match self.reason {
                ConfirmationReason::MissingClock => ConfirmationRequirement::AssignClock,
                ConfirmationReason::AmbiguousMetadata
                | ConfirmationReason::Proposal
                | ConfirmationReason::UtcInterpretation => {
                    return Err(DateTimeError::InvalidPendingDateTime)
                }
            },
        };
        Ok(ConfirmationBatchKey {
            source: self.candidate.source,
            requirement,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingConfirmationItem {
    pub item_id: String,
    pub pending: PendingDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingConfirmationBatch {
    pub key: ConfirmationBatchKey,
    pub items: Vec<PendingConfirmationItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptedDateTimeItem {
    pub item_id: String,
    pub accepted: AcceptedDateTime,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CameraCorrectionKey {
    pub make: String,
    pub model: String,
    pub serial_number: String,
}

impl CameraCorrectionKey {
    pub fn from_identity(identity: &CameraIdentity) -> Option<Self> {
        let make = identity.make.as_ref().filter(|value| !value.is_empty())?;
        let model = identity.model.as_ref().filter(|value| !value.is_empty())?;
        let serial_number = identity
            .serial_number
            .as_ref()
            .filter(|value| !value.is_empty())?;
        Some(Self {
            make: make.clone(),
            model: model.clone(),
            serial_number: serial_number.clone(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraCorrectionItem {
    pub item_id: String,
    pub camera: CameraIdentity,
    pub accepted: AcceptedDateTime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CameraCorrectionBatch {
    pub key: CameraCorrectionKey,
    pub items: Vec<AcceptedDateTimeItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DateTimeResolution {
    Accepted {
        accepted: AcceptedDateTime,
        disagreements: Vec<DateTimeCandidate>,
    },
    NeedsConfirmation(PendingDateTime),
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ItemDateTimeDecision {
    Included(DateTimeResolution),
    Excluded { reason: Option<String> },
}

pub fn pending_confirmation_batches(
    items: impl IntoIterator<Item = PendingConfirmationItem>,
) -> Result<Vec<PendingConfirmationBatch>, DateTimeError> {
    let mut item_ids = BTreeSet::new();
    let mut batches = BTreeMap::<_, Vec<_>>::new();
    for item in items {
        if !item_ids.insert(item.item_id.clone()) {
            return Err(DateTimeError::DuplicateItemId(item.item_id));
        }
        batches
            .entry(item.pending.batch_key()?)
            .or_default()
            .push(item);
    }
    Ok(batches
        .into_iter()
        .map(|(key, mut items)| {
            items.sort_by(|left, right| left.item_id.cmp(&right.item_id));
            PendingConfirmationBatch { key, items }
        })
        .collect())
}

pub fn confirm_batch(
    batch: PendingConfirmationBatch,
    decision: ConfirmationDecision,
) -> Result<Vec<AcceptedDateTimeItem>, DateTimeError> {
    batch
        .items
        .into_iter()
        .map(|item| {
            if item.pending.batch_key()? != batch.key {
                return Err(DateTimeError::MismatchedBatchKey(item.item_id));
            }
            Ok(AcceptedDateTimeItem {
                item_id: item.item_id,
                accepted: confirm_candidate(item.pending, decision)?,
            })
        })
        .collect()
}

pub fn camera_correction_batches(
    items: impl IntoIterator<Item = CameraCorrectionItem>,
) -> Result<Vec<CameraCorrectionBatch>, DateTimeError> {
    let mut item_ids = BTreeSet::new();
    let mut batches = BTreeMap::<_, Vec<_>>::new();
    for item in items {
        if !item_ids.insert(item.item_id.clone()) {
            return Err(DateTimeError::DuplicateItemId(item.item_id));
        }
        let Some(key) = CameraCorrectionKey::from_identity(&item.camera) else {
            continue;
        };
        batches.entry(key).or_default().push(AcceptedDateTimeItem {
            item_id: item.item_id,
            accepted: item.accepted,
        });
    }
    Ok(batches
        .into_iter()
        .map(|(key, mut items)| {
            items.sort_by(|left, right| left.item_id.cmp(&right.item_id));
            CameraCorrectionBatch { key, items }
        })
        .collect())
}

pub fn apply_clock_correction(
    batch: CameraCorrectionBatch,
    offset: Duration,
) -> Result<Vec<AcceptedDateTimeItem>, DateTimeError> {
    batch
        .items
        .into_iter()
        .map(|item| {
            Ok(AcceptedDateTimeItem {
                item_id: item.item_id,
                accepted: item.accepted.apply_clock_offset(offset)?,
            })
        })
        .collect()
}

pub fn assign_datetime_batch(
    item_ids: impl IntoIterator<Item = String>,
    wall_clock: PrimitiveDateTime,
) -> Result<Vec<AcceptedDateTimeItem>, DateTimeError> {
    let mut item_ids = item_ids.into_iter().collect::<Vec<_>>();
    item_ids.sort();
    for pair in item_ids.windows(2) {
        if pair[0] == pair[1] {
            return Err(DateTimeError::DuplicateItemId(pair[0].clone()));
        }
    }
    Ok(item_ids
        .into_iter()
        .map(|item_id| AcceptedDateTimeItem {
            item_id,
            accepted: AcceptedDateTime::assigned(wall_clock),
        })
        .collect())
}

pub fn resolve_datetime(
    candidates: impl IntoIterator<Item = DateTimeCandidate>,
) -> DateTimeResolution {
    // A date-only value remains usable after confirmation; lower-ranked values do not fill gaps.
    let mut by_rank: BTreeMap<u8, Vec<DateTimeCandidate>> = BTreeMap::new();
    for candidate in candidates {
        by_rank
            .entry(candidate.source.rank())
            .or_default()
            .push(candidate);
    }

    let Some((_, mut selected_rank)) = by_rank.pop_first() else {
        return DateTimeResolution::Missing;
    };
    selected_rank
        .sort_by(|left, right| (left.source, &left.value).cmp(&(right.source, &right.value)));

    let candidate = selected_rank.remove(0);
    let mut disagreements: Vec<_> = selected_rank
        .into_iter()
        .filter(|other| values_disagree(&other.value, &candidate.value))
        .collect();
    disagreements.extend(
        by_rank
            .into_values()
            .flatten()
            .filter(|other| values_disagree(&other.value, &candidate.value)),
    );
    disagreements.sort_by(|left, right| {
        (left.source.rank(), left.source, &left.value).cmp(&(
            right.source.rank(),
            right.source,
            &right.value,
        ))
    });

    match &candidate.value {
        DateTimeValue::DateOnly(_) => {
            needs_confirmation(candidate, ConfirmationReason::MissingClock, disagreements)
        }
        DateTimeValue::UtcWithoutLocalContext(_) => needs_confirmation(
            candidate,
            ConfirmationReason::UtcInterpretation,
            disagreements,
        ),
        DateTimeValue::AmbiguousContainer(_) => needs_confirmation(
            candidate,
            ConfirmationReason::AmbiguousMetadata,
            disagreements,
        ),
        DateTimeValue::Local(_) | DateTimeValue::LocalWithOffset { .. }
            if candidate.source == DateTimeSource::ContainerCreationTime =>
        {
            needs_confirmation(
                candidate,
                ConfirmationReason::AmbiguousMetadata,
                disagreements,
            )
        }
        DateTimeValue::Local(_) | DateTimeValue::LocalWithOffset { .. }
            if matches!(
                candidate.source,
                DateTimeSource::GoogleTakeoutPhotoTakenTime | DateTimeSource::FilenamePattern
            ) =>
        {
            needs_confirmation(candidate, ConfirmationReason::Proposal, disagreements)
        }
        DateTimeValue::Local(value) => DateTimeResolution::Accepted {
            accepted: AcceptedDateTime {
                wall_clock: truncate_to_seconds(*value),
                source: AcceptedDateTimeSource::Metadata(candidate.source),
                recorded_offset: None,
            },
            disagreements,
        },
        DateTimeValue::LocalWithOffset { local, offset } => DateTimeResolution::Accepted {
            accepted: AcceptedDateTime {
                wall_clock: truncate_to_seconds(*local),
                source: AcceptedDateTimeSource::Metadata(candidate.source),
                recorded_offset: Some(*offset),
            },
            disagreements,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationDecision {
    AcceptCandidate,
    AssignClock(Time),
    InterpretUtc(UtcOffset),
}

pub fn confirm_candidate(
    pending: PendingDateTime,
    decision: ConfirmationDecision,
) -> Result<AcceptedDateTime, DateTimeError> {
    let (wall_clock, recorded_offset) = match (pending.reason, &pending.candidate.value, decision) {
        (
            _,
            DateTimeValue::Local(value) | DateTimeValue::AmbiguousContainer(value),
            ConfirmationDecision::AcceptCandidate,
        ) => (truncate_to_seconds(*value), None),
        (
            _,
            DateTimeValue::LocalWithOffset { local, offset },
            ConfirmationDecision::AcceptCandidate,
        ) => (truncate_to_seconds(*local), Some(*offset)),
        (_, DateTimeValue::DateOnly(date), ConfirmationDecision::AssignClock(time)) => {
            (truncate_to_seconds(date.with_time(time)), None)
        }
        (
            ConfirmationReason::UtcInterpretation,
            DateTimeValue::UtcWithoutLocalContext(utc),
            ConfirmationDecision::InterpretUtc(offset),
        )
        | (
            ConfirmationReason::AmbiguousMetadata,
            DateTimeValue::AmbiguousContainer(utc) | DateTimeValue::Local(utc),
            ConfirmationDecision::InterpretUtc(offset),
        ) => {
            let wall_clock = utc
                .checked_add(Duration::seconds(i64::from(offset.whole_seconds())))
                .ok_or(DateTimeError::UtcInterpretationOutOfRange)?;
            (truncate_to_seconds(wall_clock), Some(offset))
        }
        _ => return Err(DateTimeError::InvalidConfirmation),
    };

    Ok(AcceptedDateTime {
        wall_clock,
        source: AcceptedDateTimeSource::Confirmed(pending.candidate.source),
        recorded_offset,
    })
}

fn needs_confirmation(
    candidate: DateTimeCandidate,
    reason: ConfirmationReason,
    disagreements: Vec<DateTimeCandidate>,
) -> DateTimeResolution {
    DateTimeResolution::NeedsConfirmation(PendingDateTime {
        candidate,
        reason,
        disagreements,
    })
}

fn truncate_to_seconds(value: PrimitiveDateTime) -> PrimitiveDateTime {
    value
        .replace_nanosecond(0)
        .expect("zero nanoseconds are always valid")
}

fn values_disagree(left: &DateTimeValue, right: &DateTimeValue) -> bool {
    let left = left.comparable_value();
    let right = right.comparable_value();
    match (&left, &right) {
        (ComparableDateTimeValue::Date(_), _) | (_, ComparableDateTimeValue::Date(_)) => {
            left.date() != right.date()
        }
        _ => left != right,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DateTimeError {
    ClockCorrectionOutOfRange,
    DuplicateItemId(String),
    InvalidConfirmation,
    InvalidPendingDateTime,
    MismatchedBatchKey(String),
    UnknownItemId(String),
    UtcInterpretationOutOfRange,
}

impl fmt::Display for DateTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockCorrectionOutOfRange => {
                formatter.write_str("clock correction exceeds the supported datetime range")
            }
            Self::DuplicateItemId(item_id) => {
                write!(
                    formatter,
                    "datetime batch contains duplicate item '{item_id}'"
                )
            }
            Self::InvalidConfirmation => {
                formatter.write_str("confirmation does not match the pending datetime value")
            }
            Self::InvalidPendingDateTime => {
                formatter.write_str("pending datetime reason does not match its candidate value")
            }
            Self::MismatchedBatchKey(item_id) => {
                write!(
                    formatter,
                    "datetime item '{item_id}' does not match its confirmation batch key"
                )
            }
            Self::UnknownItemId(item_id) => {
                write!(
                    formatter,
                    "datetime batch contains unknown item '{item_id}'"
                )
            }
            Self::UtcInterpretationOutOfRange => {
                formatter.write_str("UTC interpretation exceeds the supported datetime range")
            }
        }
    }
}

impl std::error::Error for DateTimeError {}

#[cfg(test)]
mod tests {
    use time::{Date, Month, Time};

    use super::*;

    fn datetime(hour: u8, nanosecond: u32) -> PrimitiveDateTime {
        Date::from_calendar_date(2024, Month::July, 9)
            .unwrap()
            .with_time(Time::from_hms_nano(hour, 2, 3, nanosecond).unwrap())
    }

    fn candidate(source: DateTimeSource, value: DateTimeValue) -> DateTimeCandidate {
        DateTimeCandidate { source, value }
    }

    fn pending_item(
        item_id: &str,
        source: DateTimeSource,
        value: DateTimeValue,
    ) -> PendingConfirmationItem {
        let DateTimeResolution::NeedsConfirmation(pending) =
            resolve_datetime([candidate(source, value)])
        else {
            panic!("test candidate should require confirmation");
        };
        PendingConfirmationItem {
            item_id: item_id.into(),
            pending,
        }
    }

    fn camera(make: Option<&str>, model: Option<&str>, serial: Option<&str>) -> CameraIdentity {
        CameraIdentity {
            make: make.map(str::to_owned),
            model: model.map(str::to_owned),
            serial_number: serial.map(str::to_owned),
        }
    }

    fn accepted(source: DateTimeSource, value: PrimitiveDateTime) -> AcceptedDateTime {
        AcceptedDateTime {
            wall_clock: value,
            source: AcceptedDateTimeSource::Metadata(source),
            recorded_offset: None,
        }
    }

    #[test]
    fn metadata_precedence_is_independent_of_input_order() {
        let original = candidate(
            DateTimeSource::ExifDateTimeOriginal,
            DateTimeValue::Local(datetime(5, 900_000_000)),
        );
        let digitized = candidate(
            DateTimeSource::ExifDateTimeDigitized,
            DateTimeValue::Local(datetime(6, 0)),
        );

        let forward = resolve_datetime([digitized.clone(), original.clone()]);
        let reverse = resolve_datetime([original, digitized]);

        assert_eq!(forward, reverse);
        let DateTimeResolution::Accepted {
            accepted,
            disagreements,
        } = forward
        else {
            panic!("metadata should be accepted");
        };
        assert_eq!(accepted.wall_clock, datetime(5, 0));
        assert_eq!(disagreements.len(), 1);
    }

    #[test]
    fn duplicate_sources_are_resolved_independently_of_input_order() {
        let earlier = candidate(
            DateTimeSource::XmpDateCreated,
            DateTimeValue::Local(datetime(5, 0)),
        );
        let later = candidate(
            DateTimeSource::XmpDateCreated,
            DateTimeValue::Local(datetime(6, 0)),
        );

        let forward = resolve_datetime([later.clone(), earlier.clone()]);
        let reverse = resolve_datetime([earlier, later]);

        assert_eq!(forward, reverse);
        let DateTimeResolution::Accepted {
            accepted,
            disagreements,
        } = forward
        else {
            panic!("XMP metadata should be accepted");
        };
        assert_eq!(accepted.wall_clock, datetime(5, 0));
        assert_eq!(disagreements.len(), 1);
    }

    #[test]
    fn duplicate_lower_rank_disagreements_are_ordered_deterministically() {
        let selected = candidate(
            DateTimeSource::ExifDateTimeOriginal,
            DateTimeValue::Local(datetime(4, 0)),
        );
        let earlier = candidate(
            DateTimeSource::XmpDateCreated,
            DateTimeValue::Local(datetime(5, 0)),
        );
        let later = candidate(
            DateTimeSource::XmpDateCreated,
            DateTimeValue::Local(datetime(6, 0)),
        );

        let forward = resolve_datetime([selected.clone(), later.clone(), earlier.clone()]);
        let reverse = resolve_datetime([earlier, later, selected]);

        assert_eq!(forward, reverse);
    }

    #[test]
    fn proposals_and_ambiguous_values_require_confirmation() {
        for (source, value, reason) in [
            (
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime(5, 0)),
                ConfirmationReason::Proposal,
            ),
            (
                DateTimeSource::ContainerCreationTime,
                DateTimeValue::AmbiguousContainer(datetime(5, 0)),
                ConfirmationReason::AmbiguousMetadata,
            ),
            (
                DateTimeSource::ContainerCreationTime,
                DateTimeValue::Local(datetime(5, 0)),
                ConfirmationReason::AmbiguousMetadata,
            ),
            (
                DateTimeSource::QuickTimeCreationDate,
                DateTimeValue::UtcWithoutLocalContext(datetime(5, 0)),
                ConfirmationReason::UtcInterpretation,
            ),
        ] {
            let DateTimeResolution::NeedsConfirmation(pending) =
                resolve_datetime([candidate(source, value)])
            else {
                panic!("candidate should require confirmation");
            };
            assert_eq!(pending.reason, reason);
        }
    }

    #[test]
    fn fractional_seconds_do_not_create_disagreements() {
        let resolution = resolve_datetime([
            candidate(
                DateTimeSource::XmpDateCreated,
                DateTimeValue::Local(datetime(5, 900_000_000)),
            ),
            candidate(
                DateTimeSource::XmpExifDateTimeOriginal,
                DateTimeValue::Local(datetime(5, 0)),
            ),
        ]);

        let DateTimeResolution::Accepted { disagreements, .. } = resolution else {
            panic!("equivalent XMP metadata should be accepted");
        };
        assert!(disagreements.is_empty());
    }

    #[test]
    fn equal_rank_disagreement_is_reported_without_blocking() {
        let resolution = resolve_datetime([
            candidate(
                DateTimeSource::XmpDateCreated,
                DateTimeValue::Local(datetime(5, 0)),
            ),
            candidate(
                DateTimeSource::XmpExifDateTimeOriginal,
                DateTimeValue::Local(datetime(6, 0)),
            ),
        ]);

        let DateTimeResolution::Accepted { disagreements, .. } = resolution else {
            panic!("metadata disagreement must not block import");
        };
        assert_eq!(disagreements.len(), 1);
    }

    #[test]
    fn differing_clockless_values_are_reported() {
        let first_date = datetime(5, 0).date();
        let second_date = first_date.next_day().unwrap();
        let DateTimeResolution::NeedsConfirmation(pending) = resolve_datetime([
            candidate(
                DateTimeSource::GoogleTakeoutPhotoTakenTime,
                DateTimeValue::DateOnly(first_date),
            ),
            candidate(
                DateTimeSource::FilenamePattern,
                DateTimeValue::DateOnly(second_date),
            ),
        ]) else {
            panic!("date-only proposal should require confirmation");
        };

        assert_eq!(pending.disagreements.len(), 1);
    }

    #[test]
    fn offset_metadata_preserves_local_fields_and_records_offset() {
        let offset = UtcOffset::from_hms(2, 30, 0).unwrap();
        let DateTimeResolution::Accepted { accepted, .. } = resolve_datetime([candidate(
            DateTimeSource::QuickTimeCreationDate,
            DateTimeValue::LocalWithOffset {
                local: datetime(5, 500),
                offset,
            },
        )]) else {
            panic!("offset metadata should be accepted");
        };

        assert_eq!(accepted.wall_clock, datetime(5, 0));
        assert_eq!(accepted.recorded_offset, Some(offset));
    }

    #[test]
    fn offsets_do_not_create_wall_clock_disagreements() {
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let DateTimeResolution::Accepted { disagreements, .. } = resolve_datetime([
            candidate(
                DateTimeSource::ExifDateTimeOriginal,
                DateTimeValue::Local(datetime(5, 0)),
            ),
            candidate(
                DateTimeSource::QuickTimeCreationDate,
                DateTimeValue::LocalWithOffset {
                    local: datetime(5, 0),
                    offset,
                },
            ),
        ]) else {
            panic!("local metadata should be accepted");
        };

        assert!(disagreements.is_empty());
    }

    #[test]
    fn exclusion_is_explicit() {
        let decision = ItemDateTimeDecision::Excluded {
            reason: Some("not part of this import".into()),
        };

        assert!(matches!(decision, ItemDateTimeDecision::Excluded { .. }));
    }

    #[test]
    fn confirmations_preserve_candidate_semantics() {
        let date = datetime(5, 0).date();
        let DateTimeResolution::NeedsConfirmation(date_only) = resolve_datetime([candidate(
            DateTimeSource::FilenamePattern,
            DateTimeValue::DateOnly(date),
        )]) else {
            panic!("date-only value should require confirmation");
        };
        let accepted = confirm_candidate(
            date_only,
            ConfirmationDecision::AssignClock(Time::from_hms_nano(12, 0, 0, 900_000_000).unwrap()),
        )
        .unwrap();
        assert_eq!(accepted.wall_clock, date.with_hms(12, 0, 0).unwrap());

        let DateTimeResolution::NeedsConfirmation(utc) = resolve_datetime([candidate(
            DateTimeSource::QuickTimeCreationDate,
            DateTimeValue::UtcWithoutLocalContext(datetime(5, 0)),
        )]) else {
            panic!("UTC value should require confirmation");
        };
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let accepted = confirm_candidate(utc, ConfirmationDecision::InterpretUtc(offset)).unwrap();
        assert_eq!(accepted.wall_clock, datetime(7, 0));
        assert_eq!(accepted.recorded_offset, Some(offset));
    }

    #[test]
    fn ambiguous_container_times_can_be_confirmed_as_local_or_utc() {
        let candidate = candidate(
            DateTimeSource::ContainerCreationTime,
            DateTimeValue::AmbiguousContainer(datetime(5, 0)),
        );
        let DateTimeResolution::NeedsConfirmation(pending) = resolve_datetime([candidate.clone()])
        else {
            panic!("container time should require confirmation");
        };
        let accepted = confirm_candidate(pending, ConfirmationDecision::AcceptCandidate).unwrap();
        assert_eq!(accepted.wall_clock, datetime(5, 0));
        assert_eq!(accepted.recorded_offset, None);

        let DateTimeResolution::NeedsConfirmation(pending) = resolve_datetime([candidate]) else {
            panic!("container time should require confirmation");
        };
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let accepted =
            confirm_candidate(pending, ConfirmationDecision::InterpretUtc(offset)).unwrap();
        assert_eq!(accepted.wall_clock, datetime(7, 0));
        assert_eq!(accepted.recorded_offset, Some(offset));
    }

    #[test]
    fn local_container_times_can_be_confirmed_as_utc() {
        let DateTimeResolution::NeedsConfirmation(pending) = resolve_datetime([candidate(
            DateTimeSource::ContainerCreationTime,
            DateTimeValue::Local(datetime(5, 0)),
        )]) else {
            panic!("container time should require confirmation");
        };
        let offset = UtcOffset::from_hms(-3, 0, 0).unwrap();
        let accepted =
            confirm_candidate(pending, ConfirmationDecision::InterpretUtc(offset)).unwrap();

        assert_eq!(accepted.wall_clock, datetime(2, 0));
        assert_eq!(accepted.recorded_offset, Some(offset));
    }

    #[test]
    fn pending_confirmations_group_by_source_and_required_interpretation() {
        let items = vec![
            pending_item(
                "filename",
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime(5, 0)),
            ),
            pending_item(
                "container-local",
                DateTimeSource::ContainerCreationTime,
                DateTimeValue::Local(datetime(6, 0)),
            ),
            pending_item(
                "container-ambiguous",
                DateTimeSource::ContainerCreationTime,
                DateTimeValue::AmbiguousContainer(datetime(7, 0)),
            ),
            pending_item(
                "clockless",
                DateTimeSource::FilenamePattern,
                DateTimeValue::DateOnly(datetime(8, 0).date()),
            ),
        ];
        let mut reversed = items.clone();
        reversed.reverse();

        let forward = pending_confirmation_batches(items).unwrap();
        let reverse = pending_confirmation_batches(reversed).unwrap();

        assert_eq!(forward, reverse);
        assert_eq!(forward.len(), 3);
        assert_eq!(
            forward[0].key,
            ConfirmationBatchKey {
                source: DateTimeSource::ContainerCreationTime,
                requirement: ConfirmationRequirement::ChooseLocalOrUtc,
            }
        );
        assert_eq!(
            forward[0]
                .items
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            ["container-ambiguous", "container-local"]
        );
    }

    #[test]
    fn confirmation_batches_return_sorted_per_item_provenance() {
        let batch = pending_confirmation_batches([
            pending_item(
                "b",
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime(6, 0)),
            ),
            pending_item(
                "a",
                DateTimeSource::FilenamePattern,
                DateTimeValue::Local(datetime(5, 0)),
            ),
        ])
        .unwrap()
        .remove(0);

        let confirmed = confirm_batch(batch, ConfirmationDecision::AcceptCandidate).unwrap();

        assert_eq!(
            confirmed
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(confirmed.iter().all(|item| {
            item.accepted.source
                == AcceptedDateTimeSource::Confirmed(DateTimeSource::FilenamePattern)
        }));
    }

    #[test]
    fn camera_correction_batches_require_complete_nonempty_identities() {
        let accepted = accepted(DateTimeSource::ExifDateTimeOriginal, datetime(5, 0));
        let items = [
            CameraCorrectionItem {
                item_id: "complete".into(),
                camera: camera(Some("Canon"), Some("R5"), Some("123")),
                accepted: accepted.clone(),
            },
            CameraCorrectionItem {
                item_id: "missing-serial".into(),
                camera: camera(Some("Canon"), Some("R5"), None),
                accepted: accepted.clone(),
            },
            CameraCorrectionItem {
                item_id: "empty-model".into(),
                camera: camera(Some("Canon"), Some(""), Some("456")),
                accepted,
            },
        ];

        let batches = camera_correction_batches(items).unwrap();

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].items[0].item_id, "complete");
        assert_eq!(
            batches[0].key,
            CameraCorrectionKey {
                make: "Canon".into(),
                model: "R5".into(),
                serial_number: "123".into(),
            }
        );
    }

    #[test]
    fn camera_corrections_are_input_order_independent_and_can_cross_dates() {
        let before_midnight = Date::from_calendar_date(2024, Month::July, 9)
            .unwrap()
            .with_hms(23, 45, 0)
            .unwrap();
        let after_midnight = Date::from_calendar_date(2024, Month::July, 10)
            .unwrap()
            .with_hms(0, 15, 0)
            .unwrap();
        let items = vec![
            CameraCorrectionItem {
                item_id: "later".into(),
                camera: camera(Some("Canon"), Some("R5"), Some("123")),
                accepted: accepted(DateTimeSource::ExifDateTimeOriginal, after_midnight),
            },
            CameraCorrectionItem {
                item_id: "earlier".into(),
                camera: camera(Some("Canon"), Some("R5"), Some("123")),
                accepted: accepted(DateTimeSource::ExifDateTimeOriginal, before_midnight),
            },
        ];
        let mut reversed = items.clone();
        reversed.reverse();

        let forward = camera_correction_batches(items).unwrap();
        let reverse = camera_correction_batches(reversed).unwrap();
        assert_eq!(forward, reverse);

        let corrected =
            apply_clock_correction(forward.into_iter().next().unwrap(), Duration::hours(1))
                .unwrap();
        assert_eq!(
            corrected[0].accepted.wall_clock,
            Date::from_calendar_date(2024, Month::July, 10)
                .unwrap()
                .with_hms(0, 45, 0)
                .unwrap()
        );
        assert!(matches!(
            &corrected[0].accepted.source,
            AcceptedDateTimeSource::Corrected {
                original,
                offset
            } if original.as_ref()
                == &AcceptedDateTimeSource::Metadata(DateTimeSource::ExifDateTimeOriginal)
                && *offset == Duration::hours(1)
        ));
    }

    #[test]
    fn explicit_assignment_is_sorted_and_preserves_user_provenance() {
        let assigned = assign_datetime_batch(
            ["selected-b".to_owned(), "selected-a".to_owned()],
            datetime(5, 900_000_000),
        )
        .unwrap();

        assert_eq!(
            assigned
                .iter()
                .map(|item| item.item_id.as_str())
                .collect::<Vec<_>>(),
            ["selected-a", "selected-b"]
        );
        assert!(assigned.iter().all(|item| {
            item.accepted.wall_clock == datetime(5, 0)
                && item.accepted.source == AcceptedDateTimeSource::UserAssigned
        }));
    }

    #[test]
    fn duplicate_item_ids_are_rejected_before_batching() {
        let duplicate = pending_item(
            "same",
            DateTimeSource::FilenamePattern,
            DateTimeValue::Local(datetime(5, 0)),
        );

        assert!(matches!(
            pending_confirmation_batches([duplicate.clone(), duplicate]),
            Err(DateTimeError::DuplicateItemId(item_id)) if item_id == "same"
        ));
        assert!(matches!(
            assign_datetime_batch(["same".to_owned(), "same".to_owned()], datetime(5, 0)),
            Err(DateTimeError::DuplicateItemId(item_id)) if item_id == "same"
        ));
    }

    #[test]
    fn offset_container_values_form_explicit_acceptance_batches() {
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let item = pending_item(
            "offset-container",
            DateTimeSource::ContainerCreationTime,
            DateTimeValue::LocalWithOffset {
                local: datetime(5, 0),
                offset,
            },
        );

        let batch = pending_confirmation_batches([item]).unwrap().remove(0);

        assert_eq!(
            batch.key.requirement,
            ConfirmationRequirement::AcceptCandidate
        );
        let accepted = confirm_batch(batch, ConfirmationDecision::AcceptCandidate).unwrap();
        assert_eq!(accepted[0].accepted.recorded_offset, Some(offset));
    }

    #[test]
    fn invalid_pending_reason_and_value_pairs_are_rejected() {
        let invalid = PendingConfirmationItem {
            item_id: "invalid".into(),
            pending: PendingDateTime {
                candidate: candidate(
                    DateTimeSource::FilenamePattern,
                    DateTimeValue::DateOnly(datetime(5, 0).date()),
                ),
                reason: ConfirmationReason::Proposal,
                disagreements: Vec::new(),
            },
        };

        assert_eq!(
            pending_confirmation_batches([invalid]),
            Err(DateTimeError::InvalidPendingDateTime)
        );
    }
}
