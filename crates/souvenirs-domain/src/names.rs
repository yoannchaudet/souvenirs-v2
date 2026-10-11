use std::collections::BTreeMap;
use std::fmt;

use caseless::Caseless;
use unicode_normalization::UnicodeNormalization;

const MAX_COMPONENT_UTF8_BYTES: usize = 255;
const MAX_COMPONENT_UTF16_UNITS: usize = 255;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CatalogLabel(String);

impl CatalogLabel {
    pub fn new(value: impl AsRef<str>) -> Result<Self, NameError> {
        let normalized: String = value.as_ref().nfc().collect();
        validate_component(&normalized)?;

        let first_token = normalized.split(' ').next().unwrap_or_default();
        if is_range_token(first_token) {
            return Err(NameError::ReservedRangeToken);
        }

        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn collision_key(&self) -> CollisionKey {
        collision_key(&self.0)
    }
}

impl fmt::Display for CatalogLabel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PortableComponent(String);

impl PortableComponent {
    pub fn new(value: impl AsRef<str>) -> Result<Self, NameError> {
        let value = value.as_ref();
        validate_component(value)?;
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn collision_key(&self) -> CollisionKey {
        collision_key(&self.0)
    }
}

impl fmt::Display for PortableComponent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PortableRelativePath(Vec<PortableComponent>);

impl PortableRelativePath {
    pub fn new<I, S>(components: I) -> Result<Self, NameError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let components = components
            .into_iter()
            .map(PortableComponent::new)
            .collect::<Result<Vec<_>, _>>()?;

        if components.is_empty() {
            return Err(NameError::EmptyPath);
        }

        Ok(Self(components))
    }

    pub fn components(&self) -> &[PortableComponent] {
        &self.0
    }

    pub fn collision_key(&self) -> PathCollisionKey {
        PathCollisionKey(
            self.0
                .iter()
                .map(PortableComponent::collision_key)
                .collect(),
        )
    }
}

impl fmt::Display for PortableRelativePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, component) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("/")?;
            }
            component.fmt(formatter)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CollisionKey(String);

impl CollisionKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PathCollisionKey(Vec<CollisionKey>);

impl PathCollisionKey {
    pub fn components(&self) -> &[CollisionKey] {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PathCollision {
    first: PortableRelativePath,
    second: PortableRelativePath,
}

impl PathCollision {
    pub fn first(&self) -> &PortableRelativePath {
        &self.first
    }

    pub fn second(&self) -> &PortableRelativePath {
        &self.second
    }
}

impl fmt::Display for PathCollision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "portable paths collide: '{}' and '{}'",
            self.first, self.second
        )
    }
}

impl std::error::Error for PathCollision {}

/// Rejects case-folded or normalization-equivalent destination paths.
///
/// Callers must include both planned destinations and existing catalog entries.
pub fn ensure_unique_portable_paths<'a>(
    paths: impl IntoIterator<Item = &'a PortableRelativePath>,
) -> Result<(), PathCollision> {
    let mut observed = BTreeMap::new();
    for path in paths {
        if let Some(first) = observed.insert(path.collision_key(), path) {
            return Err(PathCollision {
                first: (*first).clone(),
                second: path.clone(),
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NameError {
    Empty,
    EmptyPath,
    LeadingOrTrailingWhitespace,
    TrailingDot,
    ControlCharacter,
    NonSpaceWhitespace,
    ForbiddenCharacter(char),
    DotComponent,
    ReservedWindowsName,
    ReservedRangeToken,
    TooLongUtf8 { bytes: usize },
    TooLongUtf16 { units: usize },
}

impl fmt::Display for NameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("name must not be empty"),
            Self::EmptyPath => formatter.write_str("path must contain at least one component"),
            Self::LeadingOrTrailingWhitespace => {
                formatter.write_str("name must not have leading or trailing whitespace")
            }
            Self::TrailingDot => formatter.write_str("name must not end with a dot"),
            Self::ControlCharacter => {
                formatter.write_str("name must not contain control characters")
            }
            Self::NonSpaceWhitespace => {
                formatter.write_str("name must not contain non-space whitespace")
            }
            Self::ForbiddenCharacter(character) => {
                write!(formatter, "name contains forbidden character {character:?}")
            }
            Self::DotComponent => formatter.write_str("name must not be '.' or '..'"),
            Self::ReservedWindowsName => {
                formatter.write_str("name must not use a reserved Windows device name")
            }
            Self::ReservedRangeToken => {
                formatter.write_str("label must not start with a dated-range token")
            }
            Self::TooLongUtf8 { bytes } => {
                write!(formatter, "name uses {bytes} UTF-8 bytes; maximum is 255")
            }
            Self::TooLongUtf16 { units } => {
                write!(
                    formatter,
                    "name uses {units} UTF-16 code units; maximum is 255"
                )
            }
        }
    }
}

impl std::error::Error for NameError {}

fn validate_component(value: &str) -> Result<(), NameError> {
    if value.is_empty() {
        return Err(NameError::Empty);
    }
    if value == "." || value == ".." {
        return Err(NameError::DotComponent);
    }
    if value.trim() != value {
        return Err(NameError::LeadingOrTrailingWhitespace);
    }
    if value.ends_with('.') {
        return Err(NameError::TrailingDot);
    }

    for character in value.chars() {
        if character.is_control() {
            return Err(NameError::ControlCharacter);
        }
        if character.is_whitespace() && character != ' ' {
            return Err(NameError::NonSpaceWhitespace);
        }
        if matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        ) {
            return Err(NameError::ForbiddenCharacter(character));
        }
    }

    if is_reserved_windows_name(value) {
        return Err(NameError::ReservedWindowsName);
    }

    let utf8_bytes = value.len();
    if utf8_bytes > MAX_COMPONENT_UTF8_BYTES {
        return Err(NameError::TooLongUtf8 { bytes: utf8_bytes });
    }

    let utf16_units = value.encode_utf16().count();
    if utf16_units > MAX_COMPONENT_UTF16_UNITS {
        return Err(NameError::TooLongUtf16 { units: utf16_units });
    }

    Ok(())
}

fn collision_key(value: &str) -> CollisionKey {
    CollisionKey(value.nfd().default_case_fold().nfd().collect())
}

fn is_reserved_windows_name(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.']);
    let folded = stem.to_ascii_uppercase();
    matches!(
        folded.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || folded
        .strip_prefix("COM")
        .is_some_and(is_reserved_device_number)
        || folded
            .strip_prefix("LPT")
            .is_some_and(is_reserved_device_number)
}

fn is_reserved_device_number(suffix: &str) -> bool {
    matches!(
        suffix,
        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
    )
}

fn is_range_token(token: &str) -> bool {
    token
        .strip_prefix('+')
        .and_then(|rest| rest.strip_suffix('d'))
        .is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use proptest::test_runner::RngSeed;

    use super::*;

    #[test]
    fn label_normalizes_to_nfc() {
        let label = CatalogLabel::new("Cafe\u{301}").unwrap();

        assert_eq!(label.as_str(), "Café");
    }

    #[test]
    fn rejects_reserved_names_and_range_tokens() {
        for invalid in [
            "CON",
            "CON .txt",
            "nul.txt",
            "Lpt9",
            "COM¹.jpg",
            "+2d Vacation",
        ] {
            assert!(CatalogLabel::new(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn rejects_windows_console_device_aliases() {
        for invalid in ["conin$", "CONIN$", "ConIn$.txt", "conout$", "CONOUT$.log"] {
            assert_eq!(
                PortableComponent::new(invalid),
                Err(NameError::ReservedWindowsName),
                "{invalid}"
            );
            assert_eq!(
                CatalogLabel::new(invalid),
                Err(NameError::ReservedWindowsName),
                "{invalid}"
            );
        }
    }

    #[test]
    fn collision_key_uses_case_folding_and_normalization() {
        let first = PortableComponent::new("Straße").unwrap();
        let second = PortableComponent::new("STRASSE").unwrap();

        assert_eq!(first.collision_key(), second.collision_key());
    }

    #[test]
    fn collision_key_supports_post_unicode_nine_case_pairs() {
        let mtavruli = PortableComponent::new("\u{1c90}").unwrap();
        let mkhedruli = PortableComponent::new("\u{10d0}").unwrap();

        // This pair requires the Unicode 11 or newer case-folding data.
        assert_eq!(mtavruli.collision_key(), mkhedruli.collision_key());
    }

    #[test]
    fn portable_path_collisions_do_not_depend_on_traversal_order() {
        let first = PortableRelativePath::new(["Year", "Straße.JPG"]).unwrap();
        let second = PortableRelativePath::new(["year", "STRASSE.jpg"]).unwrap();

        let forward = ensure_unique_portable_paths([&first, &second]).unwrap_err();
        let reverse = ensure_unique_portable_paths([&second, &first]).unwrap_err();

        assert_eq!(forward.first(), &first);
        assert_eq!(forward.second(), &second);
        assert_eq!(reverse.first(), &second);
        assert_eq!(reverse.second(), &first);
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            rng_seed: RngSeed::Fixed(0x5eed_cafe),
            failure_persistence: None,
            ..ProptestConfig::default()
        })]

        #[test]
        fn accepted_labels_are_portable(suffix in "[A-Za-z0-9_-]{0,40}") {
            let label = CatalogLabel::new(format!("Album-{suffix}")).unwrap();
            let value = label.as_str();
            prop_assert!(!value.is_empty());
            prop_assert_eq!(value.trim(), value);
            prop_assert!(!value.ends_with('.'));
            prop_assert!(value.len() <= 255);
            prop_assert!(value.encode_utf16().count() <= 255);
            prop_assert!(!value.chars().any(|character| matches!(
                character,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
            )));
        }

        #[test]
        fn labels_with_forbidden_characters_are_rejected(
            prefix in "[A-Za-z]{0,12}",
            forbidden in prop::sample::select(vec!['<', '>', ':', '"', '/', '\\', '|', '?', '*']),
            suffix in "[A-Za-z]{0,12}",
        ) {
            let candidate = format!("{prefix}{forbidden}{suffix}");
            prop_assert!(CatalogLabel::new(candidate).is_err());
        }

        #[test]
        fn collision_keys_are_invariant_across_canonical_normalization(prefix in "[A-Za-z]{0,30}") {
            let composed = PortableComponent::new(format!("{prefix}é")).unwrap();
            let decomposed = PortableComponent::new(format!("{prefix}e\u{301}")).unwrap();

            prop_assert_eq!(composed.collision_key(), decomposed.collision_key());
        }

        #[test]
        fn case_only_destination_collisions_are_rejected(prefix in "[A-Za-z]{1,20}") {
            let lower_name = format!("{prefix}.jpg").to_lowercase();
            let upper_name = format!("{prefix}.jpg").to_uppercase();
            let lower = PortableRelativePath::new(["2024", lower_name.as_str()]).unwrap();
            let upper = PortableRelativePath::new(["2024", upper_name.as_str()]).unwrap();

            prop_assert!(ensure_unique_portable_paths([&lower, &upper]).is_err());
            prop_assert!(ensure_unique_portable_paths([&upper, &lower]).is_err());
        }
    }
}
