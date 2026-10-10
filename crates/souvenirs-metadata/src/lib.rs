//! Media format classification and normalized metadata values.

use std::fmt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use souvenirs_domain::datetime::{DateTimeCandidate, DateTimeSource, DateTimeValue};
use souvenirs_domain::source::{
    CameraIdentity, OriginalRole, SidecarNamingForm, SourceFileKind, SourceMetadata,
};
use time::format_description::well_known::Iso8601;
use time::macros::format_description;
use time::{Date, Duration, Month, OffsetDateTime, PrimitiveDateTime, UtcOffset};

pub const SUPPORTED_ORIGINAL_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "heic", "dng", "cr3", "nef", "arw", "mov", "mp4",
];
pub const SUPPORTED_SIDECAR_EXTENSIONS: &[&str] = &["xmp", "aae", "dop", "pp3"];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    Photo,
    Video,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AssetKind {
    Original {
        media: MediaKind,
        role: OriginalRole,
    },
    Sidecar {
        naming_forms: Vec<SidecarNamingForm>,
    },
    Unsupported,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RelationshipMetadata {
    pub apple_content_identifier: Option<String>,
}

#[derive(Debug)]
pub struct MetadataError {
    path: PathBuf,
    source: std::io::Error,
}

impl MetadataError {
    fn new(path: &Path, source: std::io::Error) -> Self {
        Self {
            path: path.to_owned(),
            source,
        }
    }
}

impl fmt::Display for MetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "read metadata from {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for MetadataError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

impl AssetKind {
    pub fn source_file_kind(&self) -> SourceFileKind {
        match self {
            Self::Original { role, .. } => SourceFileKind::Original(*role),
            Self::Sidecar { naming_forms } => SourceFileKind::Sidecar {
                naming_forms: naming_forms.clone(),
            },
            Self::Unsupported => SourceFileKind::Unknown,
        }
    }
}

pub fn classify_path(path: &Path) -> AssetKind {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(classify_extension)
        .unwrap_or(AssetKind::Unsupported)
}

pub fn classify_extension(extension: &str) -> AssetKind {
    let extension = extension.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" => original(MediaKind::Photo, OriginalRole::Jpeg),
        "heic" => original(MediaKind::Photo, OriginalRole::Still),
        "dng" | "cr3" | "nef" | "arw" => original(MediaKind::Photo, OriginalRole::Raw),
        "mov" | "mp4" => original(MediaKind::Video, OriginalRole::Video),
        "xmp" => AssetKind::Sidecar {
            naming_forms: vec![
                SidecarNamingForm::Basename,
                SidecarNamingForm::AssetFilename,
                SidecarNamingForm::NumberedAssetFilename,
            ],
        },
        "aae" => AssetKind::Sidecar {
            naming_forms: vec![SidecarNamingForm::Basename],
        },
        "dop" | "pp3" => AssetKind::Sidecar {
            naming_forms: vec![SidecarNamingForm::AssetFilename],
        },
        _ => AssetKind::Unsupported,
    }
}

pub fn relationship_metadata(path: &Path) -> Result<RelationshipMetadata, MetadataError> {
    let identifier = match classify_path(path) {
        AssetKind::Original {
            media: MediaKind::Photo,
            role: OriginalRole::Jpeg | OriginalRole::Still,
        } => still_content_identifier(path)?,
        AssetKind::Original {
            media: MediaKind::Photo,
            role: OriginalRole::Raw,
        } if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dng")) =>
        {
            dng_content_identifier(path)?
        }
        AssetKind::Original {
            media: MediaKind::Video,
            ..
        } => quicktime_content_identifier(path)?,
        AssetKind::Original { .. } | AssetKind::Sidecar { .. } | AssetKind::Unsupported => None,
    };
    Ok(RelationshipMetadata {
        apple_content_identifier: identifier,
    })
}

pub fn normalized_metadata(path: &Path) -> Result<SourceMetadata, MetadataError> {
    let relationship = relationship_metadata(path)?;
    let mut datetime_candidates = Vec::new();
    let mut camera = empty_camera();

    match classify_path(path) {
        AssetKind::Original {
            media: MediaKind::Photo,
            role: OriginalRole::Jpeg | OriginalRole::Still,
        } => {
            if let Some(tiff) = read_supported_still_tiff(path)? {
                let extracted = tiff_normalized_metadata(&tiff)
                    .map_err(|error| MetadataError::new(path, error))?;
                datetime_candidates.extend(extracted.datetime_candidates);
                camera = extracted.camera;
            }
        }
        AssetKind::Original {
            media: MediaKind::Photo,
            role: OriginalRole::Raw,
        } if path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                ["dng", "nef", "arw"]
                    .iter()
                    .any(|candidate| extension.eq_ignore_ascii_case(candidate))
            }) =>
        {
            if let Some(tiff) = read_classic_tiff(path)? {
                let extracted = tiff_normalized_metadata(&tiff)
                    .map_err(|error| MetadataError::new(path, error))?;
                datetime_candidates.extend(extracted.datetime_candidates);
                camera = extracted.camera;
            }
        }
        AssetKind::Original {
            media: MediaKind::Video,
            ..
        } => {
            let extracted = quicktime_normalized_metadata(path)?;
            datetime_candidates.extend(extracted);
        }
        AssetKind::Sidecar { .. }
            if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("xmp")) =>
        {
            datetime_candidates.extend(xmp_datetime_candidates(path)?);
        }
        AssetKind::Original { .. } | AssetKind::Sidecar { .. } | AssetKind::Unsupported => {}
    }

    if let Some(candidate) = filename_datetime_candidate(path) {
        datetime_candidates.push(candidate);
    }

    Ok(SourceMetadata {
        datetime_candidates,
        camera,
        apple_content_identifier: relationship.apple_content_identifier,
    })
}

fn empty_camera() -> CameraIdentity {
    CameraIdentity {
        make: None,
        model: None,
        serial_number: None,
    }
}

struct ExtractedTiffMetadata {
    datetime_candidates: Vec<DateTimeCandidate>,
    camera: CameraIdentity,
}

fn read_supported_still_tiff(path: &Path) -> Result<Option<Vec<u8>>, MetadataError> {
    if !has_supported_still_signature(path)? {
        return Ok(None);
    }
    let bytes = read_bounded_file(path, 512 * 1024 * 1024)?;
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let tiff = if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_exif_tiff(&bytes)
    } else {
        heif_exif_tiff(&bytes)
    }
    .map_err(|error| MetadataError::new(path, error))?;
    Ok(tiff.map(ToOwned::to_owned))
}

fn read_classic_tiff(path: &Path) -> Result<Option<Vec<u8>>, MetadataError> {
    let Some(bytes) = read_bounded_file(path, 512 * 1024 * 1024)? else {
        return Ok(None);
    };
    if !matches!(bytes.get(..2), Some(value) if value == b"II" || value == b"MM") {
        return Ok(None);
    }
    if is_valid_bigtiff(&bytes).map_err(|error| MetadataError::new(path, error))? {
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn read_bounded_file(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, MetadataError> {
    let size = std::fs::metadata(path)
        .map_err(|error| MetadataError::new(path, error))?
        .len();
    if size > limit {
        return Ok(None);
    }
    std::fs::read(path)
        .map(Some)
        .map_err(|error| MetadataError::new(path, error))
}

fn tiff_normalized_metadata(tiff: &[u8]) -> std::io::Result<ExtractedTiffMetadata> {
    const EXIF_IFD_POINTER: u16 = 0x8769;
    const MAKE: u16 = 0x010f;
    const MODEL: u16 = 0x0110;
    const DATE_TIME_ORIGINAL: u16 = 0x9003;
    const DATE_TIME_DIGITIZED: u16 = 0x9004;
    const BODY_SERIAL_NUMBER: u16 = 0xa431;
    const MAX_IFD_ENTRIES: usize = 4096;

    let (byte_order, ifd0_offset) = tiff_header(tiff)?;
    let make = tiff_ascii_value(tiff, ifd0_offset, MAKE, byte_order, MAX_IFD_ENTRIES)?;
    let model = tiff_ascii_value(tiff, ifd0_offset, MODEL, byte_order, MAX_IFD_ENTRIES)?;
    let mut output = ExtractedTiffMetadata {
        datetime_candidates: Vec::new(),
        camera: CameraIdentity {
            make,
            model,
            serial_number: None,
        },
    };
    let Some(exif_entry) = tiff_entry(
        tiff,
        ifd0_offset,
        EXIF_IFD_POINTER,
        byte_order,
        MAX_IFD_ENTRIES,
    )?
    else {
        return Ok(output);
    };
    if !matches!(read_u16(&exif_entry, 2, byte_order), Some(4 | 13))
        || read_u32(&exif_entry, 4, byte_order) != Some(1)
    {
        return Ok(output);
    }
    let exif_offset = usize::try_from(
        read_u32(&exif_entry, 8, byte_order)
            .ok_or_else(|| invalid_data("TIFF ExifIFD pointer is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF ExifIFD offset exceeds usize"))?;

    for (tag, source) in [
        (DATE_TIME_ORIGINAL, DateTimeSource::ExifDateTimeOriginal),
        (DATE_TIME_DIGITIZED, DateTimeSource::ExifDateTimeDigitized),
    ] {
        let Some(value) = tiff_ascii_value(tiff, exif_offset, tag, byte_order, MAX_IFD_ENTRIES)?
        else {
            continue;
        };
        let Some(datetime) = parse_exif_datetime(&value) else {
            continue;
        };
        output.datetime_candidates.push(DateTimeCandidate {
            source,
            value: DateTimeValue::Local(datetime),
        });
    }
    output.camera.serial_number = tiff_ascii_value(
        tiff,
        exif_offset,
        BODY_SERIAL_NUMBER,
        byte_order,
        MAX_IFD_ENTRIES,
    )?;
    Ok(output)
}

fn tiff_ascii_value(
    tiff: &[u8],
    ifd_offset: usize,
    tag: u16,
    byte_order: ByteOrder,
    max_entries: usize,
) -> std::io::Result<Option<String>> {
    let Some(entry) = tiff_entry(tiff, ifd_offset, tag, byte_order, max_entries)? else {
        return Ok(None);
    };
    if read_u16(&entry, 2, byte_order) != Some(2) {
        return Ok(None);
    }
    let length = usize::try_from(
        read_u32(&entry, 4, byte_order)
            .ok_or_else(|| invalid_data("TIFF ASCII length is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF ASCII length exceeds usize"))?;
    if length == 0 || length > 4096 {
        return Ok(None);
    }
    let value = if length <= 4 {
        entry
            .get(8..8 + length)
            .ok_or_else(|| invalid_data("TIFF inline ASCII value is truncated"))?
    } else {
        let offset = usize::try_from(
            read_u32(&entry, 8, byte_order)
                .ok_or_else(|| invalid_data("TIFF ASCII offset is truncated"))?,
        )
        .map_err(|_| invalid_data("TIFF ASCII offset exceeds usize"))?;
        tiff.get(
            offset
                ..offset
                    .checked_add(length)
                    .ok_or_else(|| invalid_data("TIFF ASCII range overflows"))?,
        )
        .ok_or_else(|| invalid_data("TIFF ASCII value exceeds the EXIF data"))?
    };
    let value = value.split(|byte| *byte == 0).next().unwrap_or(value);
    let value = std::str::from_utf8(value)
        .map_err(|_| invalid_data("TIFF ASCII value is not UTF-8"))?
        .trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn quicktime_normalized_metadata(path: &Path) -> Result<Vec<DateTimeCandidate>, MetadataError> {
    let mut file = File::open(path).map_err(|error| MetadataError::new(path, error))?;
    let file_end = file
        .seek(SeekFrom::End(0))
        .map_err(|error| MetadataError::new(path, error))?;
    if !is_iso_base_media(&mut file, file_end).map_err(|error| MetadataError::new(path, error))? {
        return Ok(Vec::new());
    }

    let mut output = Vec::new();
    if let Some(value) =
        read_quicktime_text_key(&mut file, file_end, b"com.apple.quicktime.creationdate")
            .map_err(|error| MetadataError::new(path, error))?
    {
        let (local, offset) = parse_iso_datetime(&value)
            .and_then(|(local, offset)| offset.map(|offset| (local, offset)))
            .ok_or_else(|| {
                MetadataError::new(
                    path,
                    invalid_data("QuickTime creation date must contain a valid UTC offset"),
                )
            })?;
        output.push(DateTimeCandidate {
            source: DateTimeSource::QuickTimeCreationDate,
            value: DateTimeValue::LocalWithOffset { local, offset },
        });
    }
    if let Some(value) = read_container_creation_time(&mut file, file_end)
        .map_err(|error| MetadataError::new(path, error))?
    {
        output.push(DateTimeCandidate {
            source: DateTimeSource::ContainerCreationTime,
            value: DateTimeValue::AmbiguousContainer(value),
        });
    }
    Ok(output)
}

fn read_quicktime_text_key<R: Read + Seek>(
    reader: &mut R,
    file_end: u64,
    key: &[u8],
) -> std::io::Result<Option<String>> {
    const MAX_META_BYTES: u64 = 1 << 20;
    const MAX_VALUE_BYTES: u64 = 4096;
    let Some(moov) = find_atom(reader, b"moov", 0, file_end)? else {
        return Ok(None);
    };
    let meta = if let Some(meta) = find_atom(reader, b"meta", moov.start, moov.end)? {
        Some(meta)
    } else if let Some(udta) = find_atom(reader, b"udta", moov.start, moov.end)? {
        find_atom(reader, b"meta", udta.start, udta.end)?
    } else {
        None
    };
    let Some(meta) = meta else {
        return Ok(None);
    };
    let Some(meta_bytes) = read_limited(reader, meta.start, meta.end, MAX_META_BYTES)? else {
        return Ok(None);
    };
    let mut meta_reader = std::io::Cursor::new(meta_bytes);
    let meta_end = meta_reader.get_ref().len() as u64;
    let mut metadata_atoms = None;
    for offset in [0, 4] {
        if let (Some(keys), Some(ilst)) = (
            find_atom(&mut meta_reader, b"keys", offset, meta_end)?,
            find_atom(&mut meta_reader, b"ilst", offset, meta_end)?,
        ) {
            metadata_atoms = Some((keys, ilst));
            break;
        }
    }
    let Some((keys, ilst)) = metadata_atoms else {
        return Ok(None);
    };
    let mut key_index = None;
    for (index, entry) in atoms(&mut meta_reader, keys.start + 8, keys.end)?
        .into_iter()
        .enumerate()
    {
        let Some(name) = read_limited(&mut meta_reader, entry.start, entry.end, MAX_VALUE_BYTES)?
        else {
            continue;
        };
        if name == key {
            key_index = u32::try_from(index + 1).ok();
            break;
        }
    }
    let Some(key_index) = key_index else {
        return Ok(None);
    };
    let Some(item) = atoms(&mut meta_reader, ilst.start, ilst.end)?
        .into_iter()
        .find(|atom| u32::from_be_bytes(atom.kind) == key_index)
    else {
        return Ok(None);
    };
    let Some(data) = find_atom(&mut meta_reader, b"data", item.start, item.end)? else {
        return Ok(None);
    };
    let Some(value) = read_limited(&mut meta_reader, data.start + 8, data.end, MAX_VALUE_BYTES)?
    else {
        return Ok(None);
    };
    let Ok(value) = std::str::from_utf8(&value) else {
        return Ok(None);
    };
    let value = value.trim_matches(char::from(0)).trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn read_container_creation_time<R: Read + Seek>(
    reader: &mut R,
    file_end: u64,
) -> std::io::Result<Option<PrimitiveDateTime>> {
    let Some(moov) = find_atom(reader, b"moov", 0, file_end)? else {
        return Ok(None);
    };
    let Some(mvhd) = find_atom(reader, b"mvhd", moov.start, moov.end)? else {
        return Ok(None);
    };
    let header = read_exact_at(reader, mvhd.start, 12)?;
    let seconds = match header[0] {
        0 => u64::from(u32::from_be_bytes(
            header[4..8].try_into().expect("creation time"),
        )),
        1 => u64::from_be_bytes(header[4..12].try_into().expect("creation time")),
        _ => return Ok(None),
    };
    if seconds == 0 {
        return Ok(None);
    }
    let seconds = i64::try_from(seconds)
        .map_err(|_| invalid_data("QuickTime container creation time exceeds i64"))?;
    let epoch = Date::from_calendar_date(1904, Month::January, 1)
        .expect("QuickTime epoch is valid")
        .midnight();
    epoch
        .checked_add(Duration::seconds(seconds))
        .map(Some)
        .ok_or_else(|| invalid_data("QuickTime container creation time is out of range"))
}

fn xmp_datetime_candidates(path: &Path) -> Result<Vec<DateTimeCandidate>, MetadataError> {
    let Some(bytes) = read_bounded_file(path, 4 * 1024 * 1024)? else {
        return Ok(Vec::new());
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| MetadataError::new(path, invalid_data("XMP is not UTF-8")))?;
    let mut output = Vec::new();
    for (name, source) in [
        ("photoshop:DateCreated", DateTimeSource::XmpDateCreated),
        (
            "exif:DateTimeOriginal",
            DateTimeSource::XmpExifDateTimeOriginal,
        ),
    ] {
        let Some(value) = xmp_property(text, name) else {
            continue;
        };
        let Some((local, offset)) = parse_iso_datetime(value) else {
            continue;
        };
        output.push(DateTimeCandidate {
            source,
            value: offset.map_or(DateTimeValue::Local(local), |offset| {
                DateTimeValue::LocalWithOffset { local, offset }
            }),
        });
    }
    Ok(output)
}

fn xmp_property<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let attribute = format!("{name}=");
    if let Some(start) = text.find(&attribute) {
        let after = text.get(start + attribute.len()..)?;
        let quote = after.chars().next()?;
        if quote == '"' || quote == '\'' {
            let value = &after[quote.len_utf8()..];
            return value.find(quote).map(|end| value[..end].trim());
        }
    }
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let value = text.get(text.find(&open)? + open.len()..)?;
    value.find(&close).map(|end| value[..end].trim())
}

fn filename_datetime_candidate(path: &Path) -> Option<DateTimeCandidate> {
    let filename = path.file_name()?.to_str()?;
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let value = if let Some(value) = stem
        .strip_prefix("IMG_")
        .or_else(|| stem.strip_prefix("VID_"))
        .and_then(parse_compact_filename_datetime)
    {
        DateTimeValue::Local(value)
    } else if let Some(value) = stem
        .strip_prefix("PXL_")
        .and_then(parse_pixel_filename_datetime)
    {
        DateTimeValue::UtcWithoutLocalContext(value)
    } else if let Some(date) = stem
        .strip_prefix("IMG-")
        .or_else(|| stem.strip_prefix("VID-"))
        .and_then(parse_whatsapp_filename_date)
    {
        DateTimeValue::DateOnly(date)
    } else if let Some(value) = stem
        .strip_prefix("Screenshot ")
        .and_then(parse_screenshot_filename_datetime)
    {
        DateTimeValue::Local(value)
    } else {
        return None;
    };
    Some(DateTimeCandidate {
        source: DateTimeSource::FilenamePattern,
        value,
    })
}

fn parse_compact_filename_datetime(value: &str) -> Option<PrimitiveDateTime> {
    const FORMAT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]_[hour][minute][second]");
    PrimitiveDateTime::parse(value, FORMAT).ok()
}

fn parse_pixel_filename_datetime(value: &str) -> Option<PrimitiveDateTime> {
    if value.len() < 18
        || value.as_bytes().get(8) != Some(&b'_')
        || !value[15..].bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    parse_compact_filename_datetime(&value[..15])
}

fn parse_whatsapp_filename_date(value: &str) -> Option<Date> {
    const FORMAT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year][month][day]");
    if value.len() < 11 || value.as_bytes().get(8..11) != Some(b"-WA") {
        return None;
    }
    Date::parse(&value[..8], FORMAT).ok()
}

fn parse_screenshot_filename_datetime(value: &str) -> Option<PrimitiveDateTime> {
    const FORMAT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year]-[month]-[day] at [hour].[minute].[second]");
    PrimitiveDateTime::parse(value, FORMAT).ok()
}

fn parse_exif_datetime(value: &str) -> Option<PrimitiveDateTime> {
    const FORMAT: &[time::format_description::FormatItem<'_>] =
        format_description!("[year]:[month]:[day] [hour]:[minute]:[second]");
    PrimitiveDateTime::parse(value, FORMAT).ok()
}

fn parse_iso_datetime(value: &str) -> Option<(PrimitiveDateTime, Option<UtcOffset>)> {
    if let Ok(value) = OffsetDateTime::parse(value, &Iso8601::DEFAULT) {
        return Some((
            PrimitiveDateTime::new(
                value.date(),
                value.time().replace_nanosecond(0).expect("zero is valid"),
            ),
            Some(value.offset()),
        ));
    }
    PrimitiveDateTime::parse(value, &Iso8601::DEFAULT)
        .ok()
        .map(|value| {
            (
                value
                    .replace_nanosecond(0)
                    .expect("zero nanoseconds are valid"),
                None,
            )
        })
}

fn original(media: MediaKind, role: OriginalRole) -> AssetKind {
    AssetKind::Original { media, role }
}

fn still_content_identifier(path: &Path) -> Result<Option<String>, MetadataError> {
    if !has_supported_still_signature(path)? {
        return Ok(None);
    }
    const MAX_STILL_BYTES: u64 = 512 * 1024 * 1024;
    let size = std::fs::metadata(path)
        .map_err(|error| MetadataError::new(path, error))?
        .len();
    if size > MAX_STILL_BYTES {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|error| MetadataError::new(path, error))?;
    let tiff = if bytes.starts_with(&[0xff, 0xd8]) {
        jpeg_exif_tiff(&bytes)
    } else {
        heif_exif_tiff(&bytes)
    }
    .map_err(|error| MetadataError::new(path, error))?;
    let Some(tiff) = tiff else {
        return Ok(None);
    };
    let maker_note = tiff_maker_note(tiff).map_err(|error| MetadataError::new(path, error))?;
    Ok(maker_note.and_then(apple_maker_note_content_identifier))
}

fn dng_content_identifier(path: &Path) -> Result<Option<String>, MetadataError> {
    const MAX_STILL_BYTES: u64 = 512 * 1024 * 1024;
    let size = std::fs::metadata(path)
        .map_err(|error| MetadataError::new(path, error))?
        .len();
    if size > MAX_STILL_BYTES {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|error| MetadataError::new(path, error))?;
    if !matches!(bytes.get(..2), Some(value) if value == b"II" || value == b"MM") {
        return Ok(None);
    }
    if is_valid_bigtiff(&bytes).map_err(|error| MetadataError::new(path, error))? {
        return Ok(None);
    }
    let maker_note = tiff_maker_note(&bytes).map_err(|error| MetadataError::new(path, error))?;
    Ok(maker_note.and_then(apple_maker_note_content_identifier))
}

fn is_valid_bigtiff(tiff: &[u8]) -> std::io::Result<bool> {
    let byte_order = match tiff.get(..2) {
        Some(b"MM") => ByteOrder::Big,
        Some(b"II") => ByteOrder::Little,
        _ => return Err(invalid_data("TIFF byte order is invalid")),
    };
    match read_u16(tiff, 2, byte_order) {
        Some(42) => Ok(false),
        Some(43) => {
            if tiff.len() < 16 {
                return Err(invalid_data("BigTIFF header is truncated"));
            }
            if read_u16(tiff, 4, byte_order) != Some(8) {
                return Err(invalid_data("BigTIFF offset size is unsupported"));
            }
            if read_u16(tiff, 6, byte_order) != Some(0) {
                return Err(invalid_data("BigTIFF reserved header field is invalid"));
            }
            let first_ifd = usize::try_from(
                read_u64(tiff, 8, byte_order)
                    .ok_or_else(|| invalid_data("BigTIFF first IFD offset is truncated"))?,
            )
            .map_err(|_| invalid_data("BigTIFF first IFD offset exceeds usize"))?;
            let first_ifd_end = first_ifd
                .checked_add(8)
                .ok_or_else(|| invalid_data("BigTIFF first IFD range overflows"))?;
            if first_ifd < 16 || first_ifd_end > tiff.len() {
                return Err(invalid_data("BigTIFF first IFD is outside the file"));
            }
            Ok(true)
        }
        Some(_) => Err(invalid_data("TIFF magic is invalid")),
        None => Err(invalid_data("TIFF magic is truncated")),
    }
}

fn has_supported_still_signature(path: &Path) -> Result<bool, MetadataError> {
    let mut file = File::open(path).map_err(|error| MetadataError::new(path, error))?;
    let mut prefix = [0_u8; 12];
    if let Err(error) = file.read_exact(&mut prefix) {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            return Ok(false);
        }
        return Err(MetadataError::new(path, error));
    }
    let prefix = &prefix[..];
    Ok(prefix.starts_with(&[0xff, 0xd8])
        || (prefix.len() >= 12
            && &prefix[4..8] == b"ftyp"
            && matches!(
                &prefix[8..12],
                b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1"
            )))
}

fn jpeg_exif_tiff(bytes: &[u8]) -> std::io::Result<Option<&[u8]>> {
    const MAX_SEGMENTS: usize = 4096;
    let mut offset = 2;
    for _ in 0..MAX_SEGMENTS {
        if offset >= bytes.len() {
            return Ok(None);
        }
        if bytes.get(offset) != Some(&0xff) {
            return Err(invalid_data("JPEG marker prefix is missing"));
        }
        while bytes.get(offset) == Some(&0xff) {
            offset = offset
                .checked_add(1)
                .ok_or_else(|| invalid_data("JPEG marker offset overflows"))?;
        }
        let marker = *bytes
            .get(offset)
            .ok_or_else(|| invalid_data("JPEG marker is truncated"))?;
        offset = offset
            .checked_add(1)
            .ok_or_else(|| invalid_data("JPEG marker offset overflows"))?;
        if marker == 0xda || marker == 0xd9 {
            return Ok(None);
        }
        if marker == 0x01 || (0xd0..=0xd8).contains(&marker) {
            continue;
        }
        let length = usize::from(read_be_u16(bytes, offset)?);
        if length < 2 {
            return Err(invalid_data("JPEG segment length is invalid"));
        }
        let payload_start = offset
            .checked_add(2)
            .ok_or_else(|| invalid_data("JPEG payload offset overflows"))?;
        let segment_end = offset
            .checked_add(length)
            .ok_or_else(|| invalid_data("JPEG segment length overflows"))?;
        let payload = bytes
            .get(payload_start..segment_end)
            .ok_or_else(|| invalid_data("JPEG segment exceeds the file"))?;
        if marker == 0xe1 && payload.starts_with(b"Exif\0\0") {
            let tiff = &payload[6..];
            tiff_header(tiff)?;
            return Ok(Some(tiff));
        }
        offset = segment_end;
    }
    Err(invalid_data("JPEG segment count exceeds the safety limit"))
}

fn heif_exif_tiff(bytes: &[u8]) -> std::io::Result<Option<&[u8]>> {
    const MAX_BOXES: usize = 4096;
    const MAX_ITEMS: usize = 4096;
    const MAX_EXTENTS: usize = 64;
    const MAX_EXIF_BYTES: usize = 4 * 1024 * 1024;

    let top_level = byte_atoms(bytes, 0, bytes.len(), MAX_BOXES)?;
    let mut meta_boxes = top_level.iter().filter(|atom| atom.kind == *b"meta");
    let Some(meta) = meta_boxes.next() else {
        return Ok(None);
    };
    if meta_boxes.next().is_some() {
        return Err(invalid_data("HEIF contains multiple top-level meta boxes"));
    }
    let children_start = meta
        .start
        .checked_add(4)
        .ok_or_else(|| invalid_data("HEIF meta child offset overflows"))?;
    let children = byte_atoms(bytes, children_start, meta.end, MAX_BOXES)?;
    let iinf = unique_atom(&children, b"iinf", "HEIF item information")?;
    let iloc = unique_atom(&children, b"iloc", "HEIF item locations")?;
    let idat = unique_atom(&children, b"idat", "HEIF item data")?;
    let (Some(iinf), Some(iloc)) = (iinf, iloc) else {
        return Ok(None);
    };

    let exif_item_ids = heif_exif_item_ids(bytes, iinf, MAX_ITEMS, MAX_BOXES)?;
    let Some(item_id) = exif_item_ids.first().copied() else {
        return Ok(None);
    };
    if exif_item_ids.len() != 1 {
        return Ok(None);
    }
    let payload = heif_item_payload(
        bytes,
        iloc,
        idat,
        item_id,
        MAX_ITEMS,
        MAX_EXTENTS,
        MAX_EXIF_BYTES,
    )?
    .ok_or_else(|| invalid_data("HEIF Exif item has no location"))?;
    normalize_heif_exif_item(payload).map(Some)
}

fn heif_exif_item_ids(
    bytes: &[u8],
    iinf: ByteAtom,
    max_items: usize,
    max_boxes: usize,
) -> std::io::Result<Vec<u32>> {
    let iinf_bytes = bytes
        .get(iinf.start..iinf.end)
        .ok_or_else(|| invalid_data("HEIF iinf box exceeds the file"))?;
    let version = *iinf_bytes
        .first()
        .ok_or_else(|| invalid_data("HEIF iinf full-box header is truncated"))?;
    let (count, entries_offset) = match version {
        0 => (usize::from(read_be_u16(iinf_bytes, 4)?), 6),
        1 => (
            usize::try_from(read_be_u32(iinf_bytes, 4)?)
                .map_err(|_| invalid_data("HEIF item count exceeds usize"))?,
            8,
        ),
        _ => return Ok(Vec::new()),
    };
    if count > max_items {
        return Err(invalid_data("HEIF item count exceeds the safety limit"));
    }
    let entries_start = checked_add(iinf.start, entries_offset, "HEIF iinf entries")?;
    let entries = byte_atoms(bytes, entries_start, iinf.end, max_boxes)?;
    if entries.len() < count {
        return Err(invalid_data(
            "HEIF iinf entry count exceeds its child boxes",
        ));
    }
    let mut exif_ids = Vec::new();
    for entry in entries.into_iter().take(count) {
        if entry.kind != *b"infe" {
            continue;
        }
        let entry_bytes = bytes
            .get(entry.start..entry.end)
            .ok_or_else(|| invalid_data("HEIF infe box exceeds the file"))?;
        let version = *entry_bytes
            .first()
            .ok_or_else(|| invalid_data("HEIF infe full-box header is truncated"))?;
        let (item_id, item_type_offset) = match version {
            2 => (u32::from(read_be_u16(entry_bytes, 4)?), 8),
            3 => (read_be_u32(entry_bytes, 4)?, 10),
            _ => continue,
        };
        let item_type_end = checked_add(item_type_offset, 4, "HEIF infe item type")?;
        let item_type = entry_bytes
            .get(item_type_offset..item_type_end)
            .ok_or_else(|| invalid_data("HEIF infe item type is truncated"))?;
        if item_type == b"Exif" {
            exif_ids.push(item_id);
        }
    }
    Ok(exif_ids)
}

#[allow(clippy::too_many_arguments)]
fn heif_item_payload(
    bytes: &[u8],
    iloc: ByteAtom,
    idat: Option<ByteAtom>,
    target_item_id: u32,
    max_items: usize,
    max_extents: usize,
    max_payload: usize,
) -> std::io::Result<Option<&[u8]>> {
    let iloc_bytes = bytes
        .get(iloc.start..iloc.end)
        .ok_or_else(|| invalid_data("HEIF iloc box exceeds the file"))?;
    let version = *iloc_bytes
        .first()
        .ok_or_else(|| invalid_data("HEIF iloc full-box header is truncated"))?;
    if version > 2 {
        return Ok(None);
    }
    let sizes = *iloc_bytes
        .get(4)
        .ok_or_else(|| invalid_data("HEIF iloc field sizes are truncated"))?;
    let sizes2 = *iloc_bytes
        .get(5)
        .ok_or_else(|| invalid_data("HEIF iloc field sizes are truncated"))?;
    let offset_size = usize::from(sizes >> 4);
    let length_size = usize::from(sizes & 0x0f);
    let base_offset_size = usize::from(sizes2 >> 4);
    let index_size = if version == 0 {
        0
    } else {
        usize::from(sizes2 & 0x0f)
    };
    if [offset_size, length_size, base_offset_size, index_size]
        .into_iter()
        .any(|size| size > 8)
    {
        return Ok(None);
    }
    let mut cursor = 6;
    let item_count = if version < 2 {
        let count = usize::from(read_be_u16(iloc_bytes, cursor)?);
        cursor = checked_add(cursor, 2, "HEIF iloc item count")?;
        count
    } else {
        let count = usize::try_from(read_be_u32(iloc_bytes, cursor)?)
            .map_err(|_| invalid_data("HEIF iloc item count exceeds usize"))?;
        cursor = checked_add(cursor, 4, "HEIF iloc item count")?;
        count
    };
    if item_count > max_items {
        return Err(invalid_data(
            "HEIF iloc item count exceeds the safety limit",
        ));
    }

    for _ in 0..item_count {
        let item_id = if version < 2 {
            let value = u32::from(read_be_u16(iloc_bytes, cursor)?);
            cursor = checked_add(cursor, 2, "HEIF iloc item identifier")?;
            value
        } else {
            let value = read_be_u32(iloc_bytes, cursor)?;
            cursor = checked_add(cursor, 4, "HEIF iloc item identifier")?;
            value
        };
        let construction_method = if version == 0 {
            0
        } else {
            let value = read_be_u16(iloc_bytes, cursor)? & 0x000f;
            cursor = checked_add(cursor, 2, "HEIF iloc construction method")?;
            value
        };
        let data_reference_index = read_be_u16(iloc_bytes, cursor)?;
        cursor = checked_add(cursor, 2, "HEIF iloc data reference")?;
        let (base_offset, next) = read_variable_uint(iloc_bytes, cursor, base_offset_size)?;
        cursor = next;
        let extent_count = usize::from(read_be_u16(iloc_bytes, cursor)?);
        cursor = checked_add(cursor, 2, "HEIF iloc extent count")?;
        if extent_count > max_extents {
            return Err(invalid_data("HEIF extent count exceeds the safety limit"));
        }
        for _ in 0..extent_count {
            if version > 0 && index_size > 0 {
                cursor = read_variable_uint(iloc_bytes, cursor, index_size)?.1;
            }
            let (extent_offset, next) = read_variable_uint(iloc_bytes, cursor, offset_size)?;
            cursor = next;
            let (extent_length, next) = read_variable_uint(iloc_bytes, cursor, length_size)?;
            cursor = next;
            if item_id != target_item_id {
                continue;
            }
            if data_reference_index != 0 {
                return Ok(None);
            }
            if extent_count != 1 {
                return Ok(None);
            }
            let origin = match construction_method {
                0 => 0_u64,
                1 => u64::try_from(
                    idat.ok_or_else(|| invalid_data("HEIF idat construction has no idat box"))?
                        .start,
                )
                .map_err(|_| invalid_data("HEIF idat offset exceeds u64"))?,
                _ => return Ok(None),
            };
            let start = origin
                .checked_add(base_offset)
                .and_then(|value| value.checked_add(extent_offset))
                .ok_or_else(|| invalid_data("HEIF Exif item offset overflows"))?;
            let length = usize::try_from(extent_length)
                .map_err(|_| invalid_data("HEIF Exif item length exceeds usize"))?;
            if length == 0 || length > max_payload {
                return Ok(None);
            }
            let start = usize::try_from(start)
                .map_err(|_| invalid_data("HEIF Exif item offset exceeds usize"))?;
            let end = start
                .checked_add(length)
                .ok_or_else(|| invalid_data("HEIF Exif item range overflows"))?;
            return bytes
                .get(start..end)
                .map(Some)
                .ok_or_else(|| invalid_data("HEIF Exif item exceeds the file"));
        }
    }
    Ok(None)
}

fn normalize_heif_exif_item(payload: &[u8]) -> std::io::Result<&[u8]> {
    if payload.starts_with(b"II") || payload.starts_with(b"MM") {
        tiff_header(payload)?;
        return Ok(payload);
    }
    let offset = usize::try_from(read_be_u32(payload, 0)?)
        .map_err(|_| invalid_data("HEIF Exif TIFF offset exceeds usize"))?;
    let offset_after_header = checked_add(offset, 4, "HEIF Exif TIFF")?;
    for start in [offset, offset_after_header] {
        let Some(candidate) = payload.get(start..) else {
            continue;
        };
        let candidate = candidate.strip_prefix(b"Exif\0\0").unwrap_or(candidate);
        if candidate.starts_with(b"II") || candidate.starts_with(b"MM") {
            tiff_header(candidate)?;
            return Ok(candidate);
        }
    }
    Err(invalid_data("HEIF Exif item does not locate a TIFF header"))
}

fn tiff_maker_note(tiff: &[u8]) -> std::io::Result<Option<&[u8]>> {
    const EXIF_IFD_POINTER: u16 = 0x8769;
    const MAKER_NOTE: u16 = 0x927c;
    const MAX_IFD_ENTRIES: usize = 4096;
    const MAX_MAKER_NOTE_BYTES: usize = 1024 * 1024;

    let (byte_order, ifd0_offset) = tiff_header(tiff)?;
    let Some(exif_entry) = tiff_entry(
        tiff,
        ifd0_offset,
        EXIF_IFD_POINTER,
        byte_order,
        MAX_IFD_ENTRIES,
    )?
    else {
        return Ok(None);
    };
    if !matches!(read_u16(&exif_entry, 2, byte_order), Some(4 | 13))
        || read_u32(&exif_entry, 4, byte_order) != Some(1)
    {
        return Ok(None);
    }
    let exif_offset = usize::try_from(
        read_u32(&exif_entry, 8, byte_order)
            .ok_or_else(|| invalid_data("TIFF ExifIFD pointer is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF ExifIFD offset exceeds usize"))?;
    let Some(note_entry) = tiff_entry(tiff, exif_offset, MAKER_NOTE, byte_order, MAX_IFD_ENTRIES)?
    else {
        return Ok(None);
    };
    if !matches!(read_u16(&note_entry, 2, byte_order), Some(1 | 7)) {
        return Ok(None);
    }
    let length = usize::try_from(
        read_u32(&note_entry, 4, byte_order)
            .ok_or_else(|| invalid_data("TIFF MakerNote length is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF MakerNote length exceeds usize"))?;
    if length == 0 || length > MAX_MAKER_NOTE_BYTES {
        return Ok(None);
    }
    if length <= 4 {
        return Ok(None);
    }
    let offset = usize::try_from(
        read_u32(&note_entry, 8, byte_order)
            .ok_or_else(|| invalid_data("TIFF MakerNote offset is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF MakerNote offset exceeds usize"))?;
    let end = offset
        .checked_add(length)
        .ok_or_else(|| invalid_data("TIFF MakerNote range overflows"))?;
    tiff.get(offset..end)
        .map(Some)
        .ok_or_else(|| invalid_data("TIFF MakerNote exceeds the EXIF data"))
}

fn tiff_header(tiff: &[u8]) -> std::io::Result<(ByteOrder, usize)> {
    let byte_order = match tiff.get(..2) {
        Some(b"MM") => ByteOrder::Big,
        Some(b"II") => ByteOrder::Little,
        _ => return Err(invalid_data("TIFF byte order is invalid")),
    };
    if read_u16(tiff, 2, byte_order) != Some(42) {
        return Err(invalid_data("TIFF magic is invalid"));
    }
    let offset = usize::try_from(
        read_u32(tiff, 4, byte_order)
            .ok_or_else(|| invalid_data("TIFF IFD0 offset is truncated"))?,
    )
    .map_err(|_| invalid_data("TIFF IFD0 offset exceeds usize"))?;
    if offset < 8 || offset >= tiff.len() {
        return Err(invalid_data("TIFF IFD0 offset is outside the EXIF data"));
    }
    Ok((byte_order, offset))
}

fn tiff_entry(
    tiff: &[u8],
    ifd_offset: usize,
    target_tag: u16,
    byte_order: ByteOrder,
    max_entries: usize,
) -> std::io::Result<Option<[u8; 12]>> {
    let count = usize::from(
        read_u16(tiff, ifd_offset, byte_order)
            .ok_or_else(|| invalid_data("TIFF IFD entry count is truncated"))?,
    );
    if count > max_entries {
        return Err(invalid_data(
            "TIFF IFD entry count exceeds the safety limit",
        ));
    }
    let entries_start = ifd_offset
        .checked_add(2)
        .ok_or_else(|| invalid_data("TIFF IFD entry offset overflows"))?;
    let entries_length = count
        .checked_mul(12)
        .ok_or_else(|| invalid_data("TIFF IFD entry length overflows"))?;
    let entries_end = entries_start
        .checked_add(entries_length)
        .ok_or_else(|| invalid_data("TIFF IFD entry range overflows"))?;
    let entries = tiff
        .get(entries_start..entries_end)
        .ok_or_else(|| invalid_data("TIFF IFD entries are truncated"))?;
    let mut found = None;
    for entry in entries.chunks_exact(12) {
        if read_u16(entry, 0, byte_order) == Some(target_tag) {
            if found.is_some() {
                return Ok(None);
            }
            found = Some(entry.try_into().expect("TIFF entry is 12 bytes"));
        }
    }
    Ok(found)
}

fn apple_maker_note_content_identifier(maker_note: &[u8]) -> Option<String> {
    const HEADER: &[u8] = b"Apple iOS\0";
    const BYTE_ORDER_OFFSET: usize = 12;
    const IFD_OFFSET: usize = 14;
    const ENTRY_SIZE: usize = 12;
    const CONTENT_IDENTIFIER_TAG: u16 = 0x0011;
    const ASCII_TYPE: u16 = 2;
    const MAX_IDENTIFIER_BYTES: usize = 128;
    const MAX_MAKER_NOTE_ENTRIES: usize = 4096;

    if !maker_note.starts_with(HEADER) {
        return None;
    }
    let byte_order = match maker_note.get(BYTE_ORDER_OFFSET..BYTE_ORDER_OFFSET + 2)? {
        b"MM" => ByteOrder::Big,
        b"II" => ByteOrder::Little,
        _ => return None,
    };
    let entry_count = read_u16(maker_note, IFD_OFFSET, byte_order)? as usize;
    if entry_count > MAX_MAKER_NOTE_ENTRIES {
        return None;
    }
    for index in 0..entry_count {
        let entry_offset = IFD_OFFSET
            .checked_add(2)?
            .checked_add(index.checked_mul(ENTRY_SIZE)?)?;
        if read_u16(maker_note, entry_offset, byte_order)? != CONTENT_IDENTIFIER_TAG {
            continue;
        }
        let type_offset = entry_offset.checked_add(2)?;
        let length_offset = entry_offset.checked_add(4)?;
        let value_field_offset = entry_offset.checked_add(8)?;
        if read_u16(maker_note, type_offset, byte_order)? != ASCII_TYPE {
            return None;
        }
        let length = read_u32(maker_note, length_offset, byte_order)? as usize;
        if length == 0 || length > MAX_IDENTIFIER_BYTES {
            return None;
        }
        let value = if length <= 4 {
            maker_note.get(value_field_offset..value_field_offset.checked_add(length)?)?
        } else {
            let value_offset =
                usize::try_from(read_u32(maker_note, value_field_offset, byte_order)?).ok()?;
            maker_note.get(value_offset..value_offset.checked_add(length)?)?
        };
        return normalized_identifier(value);
    }
    None
}

fn quicktime_content_identifier(path: &Path) -> Result<Option<String>, MetadataError> {
    let mut file = File::open(path).map_err(|error| MetadataError::new(path, error))?;
    let file_end = file
        .seek(SeekFrom::End(0))
        .map_err(|error| MetadataError::new(path, error))?;
    if !is_iso_base_media(&mut file, file_end).map_err(|error| MetadataError::new(path, error))? {
        return Ok(None);
    }
    read_quicktime_identifier(&mut file, file_end).map_err(|error| MetadataError::new(path, error))
}

fn is_iso_base_media<R: Read + Seek>(reader: &mut R, end: u64) -> std::io::Result<bool> {
    if end < 12 {
        return Ok(false);
    }
    let prefix = read_exact_at(reader, 0, 12)?;
    if &prefix[4..8] != b"ftyp" {
        return Ok(false);
    }
    let size = u64::from(u32::from_be_bytes(
        prefix[..4].try_into().expect("four-byte atom size"),
    ));
    Ok(size >= 12 && size <= end)
}

fn read_quicktime_identifier<R: Read + Seek>(
    reader: &mut R,
    file_end: u64,
) -> std::io::Result<Option<String>> {
    Ok(
        read_quicktime_text_key(reader, file_end, b"com.apple.quicktime.content.identifier")?
            .and_then(|value| normalized_identifier(value.as_bytes())),
    )
}

fn invalid_data(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message.into())
}

fn checked_add(base: usize, amount: usize, context: &str) -> std::io::Result<usize> {
    base.checked_add(amount)
        .ok_or_else(|| invalid_data(format!("{context} offset overflows")))
}

fn checked_add_u64(base: u64, amount: u64, context: &str) -> std::io::Result<u64> {
    base.checked_add(amount)
        .ok_or_else(|| invalid_data(format!("{context} offset overflows")))
}

fn read_be_u16(bytes: &[u8], offset: usize) -> std::io::Result<u16> {
    let end = checked_add(offset, 2, "big-endian u16")?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| invalid_data("big-endian u16 is truncated"))?;
    Ok(u16::from_be_bytes(
        value.try_into().expect("two-byte integer"),
    ))
}

fn read_be_u32(bytes: &[u8], offset: usize) -> std::io::Result<u32> {
    let end = checked_add(offset, 4, "big-endian u32")?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| invalid_data("big-endian u32 is truncated"))?;
    Ok(u32::from_be_bytes(
        value.try_into().expect("four-byte integer"),
    ))
}

fn read_variable_uint(bytes: &[u8], offset: usize, width: usize) -> std::io::Result<(u64, usize)> {
    let end = checked_add(offset, width, "variable-width integer")?;
    let value = bytes
        .get(offset..end)
        .ok_or_else(|| invalid_data("variable-width integer is truncated"))?;
    let mut output = 0_u64;
    for byte in value {
        output = output
            .checked_shl(8)
            .and_then(|current| current.checked_add(u64::from(*byte)))
            .ok_or_else(|| invalid_data("variable-width integer overflows"))?;
    }
    Ok((output, end))
}

#[derive(Clone, Copy)]
struct ByteAtom {
    kind: [u8; 4],
    start: usize,
    end: usize,
}

fn byte_atoms(
    bytes: &[u8],
    start: usize,
    end: usize,
    max_boxes: usize,
) -> std::io::Result<Vec<ByteAtom>> {
    if start > end || end > bytes.len() {
        return Err(invalid_data(
            "ISO base media box bounds exceed their parent",
        ));
    }
    let mut output = Vec::new();
    let mut offset = start;
    while offset < end {
        if output.len() >= max_boxes {
            return Err(invalid_data(
                "ISO base media box count exceeds the safety limit",
            ));
        }
        let header_end = offset
            .checked_add(8)
            .ok_or_else(|| invalid_data("ISO base media box header overflows"))?;
        if header_end > end {
            return Err(invalid_data("ISO base media box header is truncated"));
        }
        let header = bytes
            .get(offset..header_end)
            .ok_or_else(|| invalid_data("ISO base media box header exceeds the file"))?;
        let size = read_be_u32(header, 0)?;
        let kind = header[4..8].try_into().expect("four-byte box kind");
        let (header_size, box_size) = match size {
            0 => (8_usize, end - offset),
            1 => {
                let large_start = checked_add(offset, 8, "large ISO base media box")?;
                let large_end = checked_add(offset, 16, "large ISO base media box")?;
                let size = usize::try_from(
                    bytes
                        .get(large_start..large_end)
                        .map(|value| u64::from_be_bytes(value.try_into().expect("eight-byte size")))
                        .ok_or_else(|| invalid_data("large ISO base media box is truncated"))?,
                )
                .map_err(|_| invalid_data("large ISO base media box exceeds usize"))?;
                (16, size)
            }
            size => (
                8,
                usize::try_from(size)
                    .map_err(|_| invalid_data("ISO base media box size exceeds usize"))?,
            ),
        };
        if box_size < header_size {
            return Err(invalid_data(
                "ISO base media box is smaller than its header",
            ));
        }
        let box_end = offset
            .checked_add(box_size)
            .ok_or_else(|| invalid_data("ISO base media box range overflows"))?;
        if box_end > end {
            return Err(invalid_data("ISO base media box exceeds its parent"));
        }
        output.push(ByteAtom {
            kind,
            start: checked_add(offset, header_size, "ISO base media box payload")?,
            end: box_end,
        });
        offset = box_end;
    }
    Ok(output)
}

fn unique_atom(
    atoms: &[ByteAtom],
    kind: &[u8; 4],
    context: &str,
) -> std::io::Result<Option<ByteAtom>> {
    let mut matches = atoms.iter().filter(|atom| atom.kind == *kind).copied();
    let first = matches.next();
    if matches.next().is_some() {
        return Err(invalid_data(format!("{context} appears more than once")));
    }
    Ok(first)
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Big,
    Little,
}

fn read_u16(bytes: &[u8], offset: usize, byte_order: ByteOrder) -> Option<u16> {
    let end = offset.checked_add(2)?;
    let bytes: [u8; 2] = bytes.get(offset..end)?.try_into().ok()?;
    Some(match byte_order {
        ByteOrder::Big => u16::from_be_bytes(bytes),
        ByteOrder::Little => u16::from_le_bytes(bytes),
    })
}

fn read_u32(bytes: &[u8], offset: usize, byte_order: ByteOrder) -> Option<u32> {
    let end = offset.checked_add(4)?;
    let bytes: [u8; 4] = bytes.get(offset..end)?.try_into().ok()?;
    Some(match byte_order {
        ByteOrder::Big => u32::from_be_bytes(bytes),
        ByteOrder::Little => u32::from_le_bytes(bytes),
    })
}

fn read_u64(bytes: &[u8], offset: usize, byte_order: ByteOrder) -> Option<u64> {
    let end = offset.checked_add(8)?;
    let bytes: [u8; 8] = bytes.get(offset..end)?.try_into().ok()?;
    Some(match byte_order {
        ByteOrder::Big => u64::from_be_bytes(bytes),
        ByteOrder::Little => u64::from_le_bytes(bytes),
    })
}

fn normalized_identifier(bytes: &[u8]) -> Option<String> {
    let value = bytes.split(|byte| *byte == 0).next().unwrap_or(bytes);
    if !value
        .iter()
        .all(|byte| byte.is_ascii_graphic() || byte.is_ascii_whitespace())
    {
        return None;
    }
    let value = std::str::from_utf8(value).ok()?.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

#[derive(Clone, Copy)]
struct Atom {
    kind: [u8; 4],
    start: u64,
    end: u64,
}

fn find_atom<R: Read + Seek>(
    reader: &mut R,
    kind: &[u8; 4],
    start: u64,
    end: u64,
) -> std::io::Result<Option<Atom>> {
    Ok(atoms(reader, start, end)?
        .into_iter()
        .find(|atom| atom.kind == *kind))
}

fn atoms<R: Read + Seek>(reader: &mut R, start: u64, end: u64) -> std::io::Result<Vec<Atom>> {
    const HEADER_BYTES: u64 = 8;
    const LARGE_HEADER_BYTES: u64 = 16;
    const MAX_ATOMS: usize = 4096;

    let mut output = Vec::new();
    let mut offset = start;
    while offset < end {
        if output.len() >= MAX_ATOMS {
            return Err(invalid_data(
                "QuickTime atom count exceeds the safety limit",
            ));
        }
        if !offset
            .checked_add(HEADER_BYTES)
            .is_some_and(|value| value <= end)
        {
            return Err(invalid_data("QuickTime atom header is truncated"));
        }
        let header = read_exact_at(reader, offset, HEADER_BYTES)?;
        let size = u32::from_be_bytes(header[..4].try_into().expect("four-byte atom size"));
        let kind = header[4..8].try_into().expect("four-byte atom kind");
        let (header_size, atom_size) = match size {
            0 => (HEADER_BYTES, end - offset),
            1 => {
                let large_offset = checked_add_u64(offset, HEADER_BYTES, "large QuickTime atom")?;
                let large = read_exact_at(reader, large_offset, 8)?;
                (
                    LARGE_HEADER_BYTES,
                    u64::from_be_bytes(large.try_into().expect("eight-byte atom size")),
                )
            }
            size => (HEADER_BYTES, u64::from(size)),
        };
        let atom_end = offset
            .checked_add(atom_size)
            .ok_or_else(|| invalid_data("QuickTime atom range overflows"))?;
        if atom_size < header_size || atom_end > end {
            return Err(invalid_data("QuickTime atom exceeds its parent"));
        }
        output.push(Atom {
            kind,
            start: checked_add_u64(offset, header_size, "QuickTime atom payload")?,
            end: atom_end,
        });
        offset = atom_end;
    }
    Ok(output)
}

fn read_limited<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    end: u64,
    limit: u64,
) -> std::io::Result<Option<Vec<u8>>> {
    let length = end
        .checked_sub(start)
        .ok_or_else(|| invalid_data("metadata range ends before it starts"))?;
    if length > limit {
        return Ok(None);
    }
    read_exact_at(reader, start, length).map(Some)
}

fn read_exact_at<R: Read + Seek>(
    reader: &mut R,
    start: u64,
    length: u64,
) -> std::io::Result<Vec<u8>> {
    start
        .checked_add(length)
        .ok_or_else(|| invalid_data("metadata read range overflows"))?;
    reader.seek(SeekFrom::Start(start))?;
    let length = usize::try_from(length).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "metadata is too large")
    })?;
    let mut output = vec![0; length];
    reader.read_exact(&mut output)?;
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Cursor;
    use tempfile::TempDir;

    const IDENTIFIER: &str = "11111111-2222-3333-4444-555555555555";

    fn atom(kind: [u8; 4], body: &[u8]) -> Vec<u8> {
        let mut output = u32::try_from(body.len() + 8)
            .unwrap()
            .to_be_bytes()
            .to_vec();
        output.extend_from_slice(&kind);
        output.extend_from_slice(body);
        output
    }

    fn apple_maker_note(identifier: &str) -> Vec<u8> {
        let mut value = identifier.as_bytes().to_vec();
        value.push(0);
        let value_offset = 14 + 2 + 12 + 4;
        let mut output = b"Apple iOS\0\0\x01MM".to_vec();
        output.extend_from_slice(&1_u16.to_be_bytes());
        output.extend_from_slice(&0x0011_u16.to_be_bytes());
        output.extend_from_slice(&2_u16.to_be_bytes());
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(&(value_offset as u32).to_be_bytes());
        output.extend_from_slice(&0_u32.to_be_bytes());
        output.extend_from_slice(&value);
        output
    }

    fn jpeg_with_maker_note(identifier: &str) -> Vec<u8> {
        let maker_note = apple_maker_note(identifier);
        let exif_ifd_offset = 26_u32;
        let maker_note_offset = 44_u32;
        let mut tiff = b"MM\0\x2a".to_vec();
        tiff.extend_from_slice(&8_u32.to_be_bytes());
        tiff.extend_from_slice(&1_u16.to_be_bytes());
        tiff.extend_from_slice(&0x8769_u16.to_be_bytes());
        tiff.extend_from_slice(&4_u16.to_be_bytes());
        tiff.extend_from_slice(&1_u32.to_be_bytes());
        tiff.extend_from_slice(&exif_ifd_offset.to_be_bytes());
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(&1_u16.to_be_bytes());
        tiff.extend_from_slice(&0x927c_u16.to_be_bytes());
        tiff.extend_from_slice(&7_u16.to_be_bytes());
        tiff.extend_from_slice(&(maker_note.len() as u32).to_be_bytes());
        tiff.extend_from_slice(&maker_note_offset.to_be_bytes());
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(&maker_note);

        let mut segment = b"Exif\0\0".to_vec();
        segment.extend_from_slice(&tiff);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&u16::try_from(segment.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(&segment);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    fn quicktime(identifier: &str) -> Vec<u8> {
        let mut keys = vec![0, 0, 0, 0];
        keys.extend_from_slice(&1_u32.to_be_bytes());
        keys.extend_from_slice(&atom(*b"mdta", b"com.apple.quicktime.content.identifier"));
        let keys = atom(*b"keys", &keys);

        let mut data = vec![0, 0, 0, 1, 0, 0, 0, 0];
        data.extend_from_slice(identifier.as_bytes());
        let item = atom(*b"data", &data);
        let ilst = atom(*b"ilst", &atom(1_u32.to_be_bytes(), &item));
        let mut moov_body = atom(*b"meta", &[keys, ilst].concat());
        moov_body.extend_from_slice(&atom(*b"trak", &[]));

        [atom(*b"ftyp", b"qt  "), atom(*b"moov", &moov_body)].concat()
    }

    fn normalized_tiff(identifier: &str) -> Vec<u8> {
        fn entry(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
            [
                tag.to_be_bytes().as_slice(),
                kind.to_be_bytes().as_slice(),
                count.to_be_bytes().as_slice(),
                value.to_be_bytes().as_slice(),
            ]
            .concat()
        }

        let maker_note = apple_maker_note(identifier);
        let make = b"Canon\0";
        let model = b"Camera R5\0";
        let original = b"2024:07:09 10:11:12\0";
        let digitized = b"2024:07:09 10:11:13\0";
        let serial = b"SN123\0";
        let ifd0_values = 8 + 2 + 3 * 12 + 4;
        let make_offset = ifd0_values;
        let model_offset = make_offset + make.len();
        let exif_offset = model_offset + model.len();
        let exif_values = exif_offset + 2 + 4 * 12 + 4;
        let original_offset = exif_values;
        let digitized_offset = original_offset + original.len();
        let serial_offset = digitized_offset + digitized.len();
        let maker_note_offset = serial_offset + serial.len();

        let mut tiff = b"MM\0\x2a".to_vec();
        tiff.extend_from_slice(&8_u32.to_be_bytes());
        tiff.extend_from_slice(&3_u16.to_be_bytes());
        tiff.extend_from_slice(&entry(0x010f, 2, make.len() as u32, make_offset as u32));
        tiff.extend_from_slice(&entry(0x0110, 2, model.len() as u32, model_offset as u32));
        tiff.extend_from_slice(&entry(0x8769, 4, 1, exif_offset as u32));
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(make);
        tiff.extend_from_slice(model);
        tiff.extend_from_slice(&4_u16.to_be_bytes());
        tiff.extend_from_slice(&entry(
            0x9003,
            2,
            original.len() as u32,
            original_offset as u32,
        ));
        tiff.extend_from_slice(&entry(
            0x9004,
            2,
            digitized.len() as u32,
            digitized_offset as u32,
        ));
        tiff.extend_from_slice(&entry(0xa431, 2, serial.len() as u32, serial_offset as u32));
        tiff.extend_from_slice(&entry(
            0x927c,
            7,
            maker_note.len() as u32,
            maker_note_offset as u32,
        ));
        tiff.extend_from_slice(&0_u32.to_be_bytes());
        tiff.extend_from_slice(original);
        tiff.extend_from_slice(digitized);
        tiff.extend_from_slice(serial);
        tiff.extend_from_slice(&maker_note);
        tiff
    }

    fn jpeg_with_tiff(tiff: &[u8]) -> Vec<u8> {
        let mut segment = b"Exif\0\0".to_vec();
        segment.extend_from_slice(tiff);
        let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
        jpeg.extend_from_slice(&u16::try_from(segment.len() + 2).unwrap().to_be_bytes());
        jpeg.extend_from_slice(&segment);
        jpeg.extend_from_slice(&[0xff, 0xd9]);
        jpeg
    }

    fn quicktime_with_normalized_dates(
        identifier: &str,
        creation_date: &str,
        container: PrimitiveDateTime,
    ) -> Vec<u8> {
        let keys = [
            &[0, 0, 0, 0][..],
            &2_u32.to_be_bytes(),
            &atom(*b"mdta", b"com.apple.quicktime.content.identifier"),
            &atom(*b"mdta", b"com.apple.quicktime.creationdate"),
        ]
        .concat();
        let data = |value: &[u8]| {
            let mut body = vec![0, 0, 0, 1, 0, 0, 0, 0];
            body.extend_from_slice(value);
            atom(*b"data", &body)
        };
        let ilst = atom(
            *b"ilst",
            &[
                atom(1_u32.to_be_bytes(), &data(identifier.as_bytes())),
                atom(2_u32.to_be_bytes(), &data(creation_date.as_bytes())),
            ]
            .concat(),
        );
        let epoch = Date::from_calendar_date(1904, Month::January, 1)
            .unwrap()
            .midnight();
        let seconds = u32::try_from((container - epoch).whole_seconds()).unwrap();
        let mut mvhd = vec![0, 0, 0, 0];
        mvhd.extend_from_slice(&seconds.to_be_bytes());
        mvhd.extend_from_slice(&seconds.to_be_bytes());
        let moov = atom(
            *b"moov",
            &[
                atom(*b"mvhd", &mvhd),
                atom(*b"meta", &[atom(*b"keys", &keys), ilst].concat()),
            ]
            .concat(),
        );
        [atom(*b"ftyp", b"qt  "), moov].concat()
    }

    fn heif_with_maker_note(identifier: &str) -> Vec<u8> {
        let jpeg = jpeg_with_maker_note(identifier);
        let tiff_start = jpeg
            .windows(6)
            .position(|window| window == b"Exif\0\0")
            .unwrap()
            + 6;
        let mut exif_item = 4_u32.to_be_bytes().to_vec();
        exif_item.extend_from_slice(&jpeg[tiff_start..jpeg.len() - 2]);

        let mut infe = vec![2, 0, 0, 0];
        infe.extend_from_slice(&1_u16.to_be_bytes());
        infe.extend_from_slice(&0_u16.to_be_bytes());
        infe.extend_from_slice(b"Exif");
        let infe = atom(*b"infe", &infe);
        let mut iinf = vec![0, 0, 0, 0];
        iinf.extend_from_slice(&1_u16.to_be_bytes());
        iinf.extend_from_slice(&infe);
        let iinf = atom(*b"iinf", &iinf);

        let ftyp = atom(*b"ftyp", b"heic\0\0\0\0heicmif1");
        let mut iloc = vec![0, 0, 0, 0, 0x44, 0];
        iloc.extend_from_slice(&1_u16.to_be_bytes());
        iloc.extend_from_slice(&1_u16.to_be_bytes());
        iloc.extend_from_slice(&0_u16.to_be_bytes());
        iloc.extend_from_slice(&1_u16.to_be_bytes());
        iloc.extend_from_slice(&[0; 8]);
        let preliminary_meta = atom(
            *b"meta",
            &[
                &[0, 0, 0, 0],
                iinf.as_slice(),
                atom(*b"iloc", &iloc).as_slice(),
            ]
            .concat(),
        );
        let payload_offset = u32::try_from(ftyp.len() + preliminary_meta.len() + 8).unwrap();
        let fields = iloc.len() - 8;
        iloc[fields..fields + 4].copy_from_slice(&payload_offset.to_be_bytes());
        iloc[fields + 4..].copy_from_slice(&u32::try_from(exif_item.len()).unwrap().to_be_bytes());
        let meta = atom(
            *b"meta",
            &[
                &[0, 0, 0, 0],
                iinf.as_slice(),
                atom(*b"iloc", &iloc).as_slice(),
            ]
            .concat(),
        );
        [ftyp, meta, atom(*b"mdat", &exif_item)].concat()
    }

    fn bigtiff(little_endian: bool) -> Vec<u8> {
        let mut bytes = if little_endian {
            b"II".to_vec()
        } else {
            b"MM".to_vec()
        };
        let u16_bytes = |value: u16| {
            if little_endian {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        let u64_bytes = |value: u64| {
            if little_endian {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            }
        };
        bytes.extend_from_slice(&u16_bytes(43));
        bytes.extend_from_slice(&u16_bytes(8));
        bytes.extend_from_slice(&u16_bytes(0));
        bytes.extend_from_slice(&u64_bytes(16));
        bytes.extend_from_slice(&u64_bytes(0));
        bytes
    }

    #[test]
    fn classification_is_case_insensitive() {
        assert_eq!(
            classify_extension("JpG"),
            AssetKind::Original {
                media: MediaKind::Photo,
                role: OriginalRole::Jpeg,
            }
        );
        assert_eq!(classify_extension("LRV"), AssetKind::Unsupported);
    }

    #[test]
    fn xmp_supports_all_declared_naming_forms() {
        assert_eq!(
            classify_extension("xmp"),
            AssetKind::Sidecar {
                naming_forms: vec![
                    SidecarNamingForm::Basename,
                    SidecarNamingForm::AssetFilename,
                    SidecarNamingForm::NumberedAssetFilename,
                ],
            }
        );
    }

    #[test]
    fn reads_apple_identifier_from_maker_note() {
        assert_eq!(
            apple_maker_note_content_identifier(&apple_maker_note(IDENTIFIER)).as_deref(),
            Some(IDENTIFIER)
        );
    }

    #[test]
    fn extracts_normalized_identifier_from_supported_still() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("still.jpg");
        fs::write(&path, jpeg_with_maker_note(IDENTIFIER)).unwrap();

        assert_eq!(
            relationship_metadata(&path)
                .unwrap()
                .apple_content_identifier
                .as_deref(),
            Some(IDENTIFIER)
        );
    }

    #[test]
    fn extracts_normalized_identifier_from_heif_exif_item() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("still.heic");
        fs::write(&path, heif_with_maker_note(IDENTIFIER)).unwrap();

        assert_eq!(
            relationship_metadata(&path)
                .unwrap()
                .apple_content_identifier
                .as_deref(),
            Some(IDENTIFIER)
        );
    }

    #[test]
    fn extracts_identifier_from_dng_and_legal_tiff_type_variants() {
        let temporary = TempDir::new().unwrap();
        let mut jpeg = jpeg_with_maker_note(IDENTIFIER);
        let exif_pointer = jpeg
            .windows(2)
            .position(|window| window == 0x8769_u16.to_be_bytes())
            .unwrap();
        jpeg[exif_pointer + 2..exif_pointer + 4].copy_from_slice(&13_u16.to_be_bytes());
        let maker_note = jpeg
            .windows(2)
            .position(|window| window == 0x927c_u16.to_be_bytes())
            .unwrap();
        jpeg[maker_note + 2..maker_note + 4].copy_from_slice(&1_u16.to_be_bytes());
        let jpeg_path = temporary.path().join("still.jpg");
        fs::write(&jpeg_path, &jpeg).unwrap();
        assert_eq!(
            relationship_metadata(&jpeg_path)
                .unwrap()
                .apple_content_identifier
                .as_deref(),
            Some(IDENTIFIER)
        );

        let tiff_start = jpeg
            .windows(6)
            .position(|window| window == b"Exif\0\0")
            .unwrap()
            + 6;
        let dng_path = temporary.path().join("still.dng");
        fs::write(&dng_path, &jpeg[tiff_start..jpeg.len() - 2]).unwrap();
        assert_eq!(
            relationship_metadata(&dng_path)
                .unwrap()
                .apple_content_identifier
                .as_deref(),
            Some(IDENTIFIER)
        );
    }

    #[test]
    fn valid_sips_heic_without_exif_is_safe() {
        let bytes = include_bytes!("../tests/fixtures/sips-1px.heic");
        assert_eq!(heif_exif_tiff(bytes).unwrap(), None);
    }

    #[test]
    fn valid_bigtiff_dng_is_an_unsupported_candidate_without_an_error() {
        let temporary = TempDir::new().unwrap();
        for (name, bytes) in [("little.dng", bigtiff(true)), ("big.dng", bigtiff(false))] {
            let path = temporary.path().join(name);
            fs::write(&path, bytes).unwrap();
            assert_eq!(
                relationship_metadata(&path)
                    .unwrap()
                    .apple_content_identifier,
                None
            );
        }
    }

    #[test]
    fn malformed_bigtiff_headers_return_contextual_errors() {
        let mut truncated = bigtiff(true);
        truncated.truncate(12);
        assert!(is_valid_bigtiff(&truncated)
            .unwrap_err()
            .to_string()
            .contains("header is truncated"));

        let mut invalid_offset_size = bigtiff(false);
        invalid_offset_size[4..6].copy_from_slice(&4_u16.to_be_bytes());
        assert!(is_valid_bigtiff(&invalid_offset_size)
            .unwrap_err()
            .to_string()
            .contains("offset size"));

        let mut invalid_ifd = bigtiff(true);
        invalid_ifd[8..16].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(is_valid_bigtiff(&invalid_ifd)
            .unwrap_err()
            .to_string()
            .contains("first IFD"));
    }

    #[test]
    fn reads_apple_identifier_from_quicktime_metadata() {
        let bytes = quicktime(IDENTIFIER);
        assert_eq!(
            read_quicktime_identifier(&mut Cursor::new(bytes.clone()), bytes.len() as u64)
                .unwrap()
                .as_deref(),
            Some(IDENTIFIER)
        );
    }

    #[test]
    fn malformed_relationship_metadata_is_ignored_safely() {
        assert_eq!(apple_maker_note_content_identifier(b"Apple iOS\0"), None);
        assert!(read_quicktime_identifier(&mut Cursor::new(b"not media".to_vec()), 9).is_err());
    }

    #[test]
    fn normalizes_tiff_datetimes_camera_and_apple_identifier() {
        let temporary = TempDir::new().unwrap();
        let tiff = normalized_tiff(IDENTIFIER);
        for (name, bytes) in [
            ("IMG_0001.JPG", jpeg_with_tiff(&tiff)),
            ("IMG_0001.DNG", tiff),
        ] {
            let path = temporary.path().join(name);
            fs::write(&path, bytes).unwrap();
            let metadata = normalized_metadata(&path).unwrap();
            assert_eq!(
                metadata.camera,
                CameraIdentity {
                    make: Some("Canon".into()),
                    model: Some("Camera R5".into()),
                    serial_number: Some("SN123".into()),
                }
            );
            assert_eq!(
                metadata.apple_content_identifier.as_deref(),
                Some(IDENTIFIER)
            );
            assert_eq!(
                metadata.datetime_candidates,
                [
                    DateTimeCandidate {
                        source: DateTimeSource::ExifDateTimeOriginal,
                        value: DateTimeValue::Local(
                            parse_exif_datetime("2024:07:09 10:11:12").unwrap()
                        ),
                    },
                    DateTimeCandidate {
                        source: DateTimeSource::ExifDateTimeDigitized,
                        value: DateTimeValue::Local(
                            parse_exif_datetime("2024:07:09 10:11:13").unwrap()
                        ),
                    },
                ]
            );
        }
    }

    #[test]
    fn normalizes_quicktime_offset_and_ambiguous_container_dates() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("clip.mov");
        let container = parse_iso_datetime("2024-08-03T13:30:45").unwrap().0;
        fs::write(
            &path,
            quicktime_with_normalized_dates(IDENTIFIER, "2024-08-03T15:30:45.999+0200", container),
        )
        .unwrap();

        let metadata = normalized_metadata(&path).unwrap();
        assert_eq!(
            metadata.apple_content_identifier.as_deref(),
            Some(IDENTIFIER)
        );
        assert_eq!(
            metadata.datetime_candidates,
            [
                DateTimeCandidate {
                    source: DateTimeSource::QuickTimeCreationDate,
                    value: DateTimeValue::LocalWithOffset {
                        local: parse_iso_datetime("2024-08-03T15:30:45").unwrap().0,
                        offset: UtcOffset::from_hms(2, 0, 0).unwrap(),
                    },
                },
                DateTimeCandidate {
                    source: DateTimeSource::ContainerCreationTime,
                    value: DateTimeValue::AmbiguousContainer(container),
                },
            ]
        );
    }

    #[test]
    fn normalizes_owned_xmp_datetime_candidates() {
        let temporary = TempDir::new().unwrap();
        let path = temporary.path().join("IMG_0001.JPG.xmp");
        fs::write(
            &path,
            br#"<rdf:Description photoshop:DateCreated="2024-07-09T10:11:12+02:00">
                <exif:DateTimeOriginal>2024-07-09T08:11:12Z</exif:DateTimeOriginal>
            </rdf:Description>"#,
        )
        .unwrap();

        let metadata = normalized_metadata(&path).unwrap();
        assert_eq!(metadata.datetime_candidates.len(), 2);
        assert_eq!(
            metadata.datetime_candidates[0].source,
            DateTimeSource::XmpDateCreated
        );
        assert!(matches!(
            metadata.datetime_candidates[0].value,
            DateTimeValue::LocalWithOffset { .. }
        ));
        assert_eq!(
            metadata.datetime_candidates[1].source,
            DateTimeSource::XmpExifDateTimeOriginal
        );
    }

    #[test]
    fn recognizes_filename_datetime_proposals() {
        let temporary = TempDir::new().unwrap();
        for (name, expected) in [
            (
                "IMG_20190714_153012.jpg",
                DateTimeValue::Local(parse_iso_datetime("2019-07-14T15:30:12").unwrap().0),
            ),
            (
                "VID_20190714_153012.mp4",
                DateTimeValue::Local(parse_iso_datetime("2019-07-14T15:30:12").unwrap().0),
            ),
            (
                "PXL_20190714_153012345.jpg",
                DateTimeValue::UtcWithoutLocalContext(
                    parse_iso_datetime("2019-07-14T15:30:12").unwrap().0,
                ),
            ),
            (
                "IMG-20190714-WA0001.jpg",
                DateTimeValue::DateOnly(parse_whatsapp_filename_date("20190714-WA").unwrap()),
            ),
            (
                "Screenshot 2019-07-14 at 15.30.12.png",
                DateTimeValue::Local(parse_iso_datetime("2019-07-14T15:30:12").unwrap().0),
            ),
        ] {
            let path = temporary.path().join(name);
            fs::write(&path, []).unwrap();
            let metadata = normalized_metadata(&path).unwrap();
            assert_eq!(
                metadata.datetime_candidates.last(),
                Some(&DateTimeCandidate {
                    source: DateTimeSource::FilenamePattern,
                    value: expected,
                })
            );
        }
    }

    #[test]
    fn rejects_truncated_and_out_of_bounds_supported_stills() {
        let temporary = TempDir::new().unwrap();
        let jpeg = temporary.path().join("truncated.jpg");
        fs::write(
            &jpeg,
            [0xff, 0xd8, 0xff, 0xe1, 0xff, 0xff, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
        assert!(relationship_metadata(&jpeg).is_err());

        let mut heif = heif_with_maker_note(IDENTIFIER);
        heif.truncate(heif.len() - 1);
        let heic = temporary.path().join("truncated.heic");
        fs::write(&heic, heif).unwrap();
        assert!(relationship_metadata(&heic).is_err());

        let mut invalid_offset = heif_with_maker_note(IDENTIFIER);
        let payload_offset = invalid_offset
            .windows(4)
            .position(|window| window == b"mdat")
            .unwrap()
            + 4;
        let encoded_offset = u32::try_from(payload_offset).unwrap().to_be_bytes();
        let location = invalid_offset
            .windows(4)
            .position(|window| window == encoded_offset)
            .unwrap();
        invalid_offset[location..location + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(heif_exif_tiff(&invalid_offset).is_err());
    }

    #[test]
    fn rejects_oversized_counts_and_skips_unknown_structures() {
        let mut oversized_ifd = b"MM\0\x2a".to_vec();
        oversized_ifd.extend_from_slice(&8_u32.to_be_bytes());
        oversized_ifd.extend_from_slice(&u16::MAX.to_be_bytes());
        assert!(tiff_maker_note(&oversized_ifd).is_err());

        let unknown_jpeg = [&[0xff, 0xd8, 0xff, 0xe2, 0, 6][..], b"nope", &[0xff, 0xd9]].concat();
        assert_eq!(jpeg_exif_tiff(&unknown_jpeg).unwrap(), None);

        let unknown_heif = [
            atom(*b"ftyp", b"heic\0\0\0\0heic"),
            atom(*b"free", b"unknown"),
        ]
        .concat();
        assert_eq!(heif_exif_tiff(&unknown_heif).unwrap(), None);

        let mut oversized_iinf = vec![0, 0, 0, 0];
        oversized_iinf.extend_from_slice(&u16::MAX.to_be_bytes());
        let meta = atom(
            *b"meta",
            &[
                &[0, 0, 0, 0],
                atom(*b"iinf", &oversized_iinf).as_slice(),
                atom(*b"iloc", &[0; 8]).as_slice(),
            ]
            .concat(),
        );
        assert!(heif_exif_tiff(&[atom(*b"ftyp", b"heic"), meta].concat()).is_err());

        let truncated_quicktime = atom(*b"moov", &[0, 0, 0, 32, b'm', b'e', b't', b'a']);
        assert!(read_quicktime_identifier(
            &mut Cursor::new(truncated_quicktime.clone()),
            truncated_quicktime.len() as u64,
        )
        .is_err());
    }
}
