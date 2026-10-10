use std::collections::BTreeMap;
use std::fmt;

use time::{Date, Duration, PrimitiveDateTime, Time, UtcOffset};

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
    fn rank(self) -> u8 {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingDateTime {
    pub candidate: DateTimeCandidate,
    pub reason: ConfirmationReason,
    pub disagreements: Vec<DateTimeCandidate>,
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
    InvalidConfirmation,
    UtcInterpretationOutOfRange,
}

impl fmt::Display for DateTimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClockCorrectionOutOfRange => {
                formatter.write_str("clock correction exceeds the supported datetime range")
            }
            Self::InvalidConfirmation => {
                formatter.write_str("confirmation does not match the pending datetime value")
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
}
