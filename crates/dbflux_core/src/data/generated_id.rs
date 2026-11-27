//! Identifiers generated on the client for a grid cell: UUID v4, UUID v7,
//! KSUID and ULID, written as literal values so every driver can store them.

use std::time::{SystemTime, UNIX_EPOCH};

/// A kind of identifier the grid can generate for a cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeneratedIdKind {
    /// Random UUID, canonical hyphenated form.
    UuidV4,
    /// Time-ordered UUID (RFC 9562), canonical hyphenated form.
    UuidV7,
    /// 27 base62 characters: a 4-byte timestamp since the KSUID epoch and
    /// 16 random bytes. Sorts by creation second.
    Ksuid,
    /// 26 Crockford base32 characters: a 48-bit millisecond timestamp and
    /// 80 random bits. Sorts by creation millisecond.
    Ulid,
}

impl GeneratedIdKind {
    pub const ALL: [Self; 4] = [Self::UuidV4, Self::UuidV7, Self::Ksuid, Self::Ulid];

    /// The kind's name as the grid shows it.
    pub fn label(self) -> &'static str {
        match self {
            Self::UuidV4 => "UUID v4",
            Self::UuidV7 => "UUID v7",
            Self::Ksuid => "KSUID",
            Self::Ulid => "ULID",
        }
    }

    /// The length of the text form.
    pub fn text_len(self) -> usize {
        match self {
            Self::UuidV4 | Self::UuidV7 => 36,
            Self::Ksuid => KSUID_LEN,
            Self::Ulid => ULID_LEN,
        }
    }

    /// A new identifier of this kind, as text.
    pub fn generate(self) -> String {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();

        match self {
            Self::UuidV4 => uuid::Uuid::new_v4().hyphenated().to_string(),
            Self::UuidV7 => uuid::Uuid::now_v7().hyphenated().to_string(),
            Self::Ksuid => ksuid_at(now.as_secs(), random_bytes()),
            Self::Ulid => {
                let mut tail = [0u8; 10];
                for (slot, byte) in tail.iter_mut().zip(random_bytes()) {
                    *slot = byte;
                }
                ulid_at(u64::try_from(now.as_millis()).unwrap_or(u64::MAX), tail)
            }
        }
    }

    /// The kinds a column of driver-reported type `type_name` can store as
    /// text, most fitting first, or none for a type that is not text-like.
    ///
    /// A native uuid column takes only UUIDs. A text column takes a KSUID
    /// or ULID only when its declared length, if known, fits them; one of
    /// 36 characters or more (or of unknown length) takes UUIDs too. Binary
    /// columns get nothing: the grid writes text, and a `binary(16)` needs
    /// the raw bytes.
    pub fn fitting(type_name: &str) -> Vec<Self> {
        let lower = type_name.trim().to_ascii_lowercase();
        let base = lower
            .split(|c: char| c == '(' || c.is_whitespace())
            .next()
            .unwrap_or("");

        if matches!(base, "uuid" | "uniqueidentifier") {
            return vec![Self::UuidV4, Self::UuidV7];
        }

        let text_like = matches!(
            base,
            "char"
                | "character"
                | "varchar"
                | "nchar"
                | "nvarchar"
                | "text"
                | "tinytext"
                | "mediumtext"
                | "longtext"
                | "ntext"
                | "citext"
                | "clob"
                | "string"
                | "bpchar"
                | "varchar2"
                | "nvarchar2"
        );
        if !text_like {
            return Vec::new();
        }

        let length = declared_length(&lower);
        let fixed =
            matches!(base, "char" | "character" | "nchar" | "bpchar") && !lower.contains("varying");

        Self::ALL
            .into_iter()
            .filter(|kind| match length {
                // A fixed-width column pads a shorter value with spaces, so
                // only the kind that fills it exactly fits.
                Some(length) if fixed => kind.text_len() == length,
                Some(length) => kind.text_len() <= length,
                None => true,
            })
            .collect()
    }
}

/// Whether the cell editor of a column should offer generated identifiers
/// without being asked: a uuid column, a text column sized for one of the
/// kinds, a key column or one named like an identifier. Any column that
/// [`GeneratedIdKind::fitting`] accepts still offers them from its menu.
pub fn looks_like_id_column(column_name: &str, type_name: &str, is_primary_key: bool) -> bool {
    let lower = type_name.trim().to_ascii_lowercase();
    if lower.starts_with("uuid") || lower.starts_with("uniqueidentifier") {
        return true;
    }
    if declared_length(&lower).is_some_and(|length| {
        GeneratedIdKind::ALL
            .iter()
            .any(|kind| kind.text_len() == length)
    }) {
        return true;
    }

    let name = column_name.to_ascii_lowercase();
    is_primary_key
        || name == "id"
        || ["_id", "uuid", "guid", "ksuid", "ulid"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

/// The single declared length of a type such as `varchar(36)`.
fn declared_length(type_name: &str) -> Option<usize> {
    let open = type_name.find('(')?;
    let close = type_name[open..].find(')')? + open;
    type_name.get(open + 1..close)?.trim().parse().ok()
}

/// Seconds between the Unix epoch and the KSUID epoch (2014-05-13).
pub const KSUID_EPOCH: u64 = 1_400_000_000;
const KSUID_LEN: usize = 27;
const ULID_LEN: usize = 26;
const BASE62: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// The KSUID for `unix_secs` and `payload`: the 20 bytes of a big-endian
/// timestamp and the payload, as 27 base62 digits.
pub fn ksuid_at(unix_secs: u64, payload: [u8; 16]) -> String {
    let timestamp = u32::try_from(unix_secs.saturating_sub(KSUID_EPOCH)).unwrap_or(u32::MAX);

    let mut bytes = [0u8; 20];
    for (slot, byte) in bytes
        .iter_mut()
        .zip(timestamp.to_be_bytes().into_iter().chain(payload))
    {
        *slot = byte;
    }

    // Repeated division of the 160-bit number by 62, least significant
    // digit first.
    let mut digits = [0u8; KSUID_LEN];
    let mut number = bytes;
    for digit in digits.iter_mut().rev() {
        let mut remainder: u32 = 0;
        for byte in number.iter_mut() {
            let value = (remainder << 8) | u32::from(*byte);
            // value / 62 < 256 because remainder < 62.
            *byte = u8::try_from(value / 62).unwrap_or(u8::MAX);
            remainder = value % 62;
        }
        *digit = base62_digit(remainder);
    }

    String::from_utf8_lossy(&digits).into_owned()
}

/// The timestamp of a KSUID, in Unix seconds, or `None` for text that is
/// not one.
pub fn ksuid_unix_secs(ksuid: &str) -> Option<u64> {
    if ksuid.len() != KSUID_LEN {
        return None;
    }

    let mut number = [0u8; 20];
    for ch in ksuid.bytes() {
        let digit = u32::try_from(BASE62.iter().position(|&b| b == ch)?).ok()?;
        let mut carry = digit;
        for byte in number.iter_mut().rev() {
            let value = u32::from(*byte) * 62 + carry;
            let [.., low] = value.to_be_bytes();
            *byte = low;
            carry = value >> 8;
        }
        if carry != 0 {
            return None;
        }
    }

    let mut timestamp = [0u8; 4];
    for (slot, byte) in timestamp.iter_mut().zip(number) {
        *slot = byte;
    }
    Some(u64::from(u32::from_be_bytes(timestamp)) + KSUID_EPOCH)
}

/// The ULID for `unix_millis` and 80 random bits, as 26 Crockford base32
/// digits.
pub fn ulid_at(unix_millis: u64, random: [u8; 10]) -> String {
    let mut value: u128 = u128::from(unix_millis & 0xFFFF_FFFF_FFFF) << 80;
    for (index, byte) in random.iter().enumerate() {
        value |= u128::from(*byte) << (72 - 8 * index);
    }

    let mut digits = [0u8; ULID_LEN];
    for digit in digits.iter_mut().rev() {
        let index = usize::try_from(value & 0x1f).unwrap_or_default();
        *digit = CROCKFORD.get(index).copied().unwrap_or(b'0');
        value >>= 5;
    }

    String::from_utf8_lossy(&digits).into_owned()
}

fn base62_digit(value: u32) -> u8 {
    usize::try_from(value)
        .ok()
        .and_then(|index| BASE62.get(index))
        .copied()
        .unwrap_or(b'0')
}

/// 16 random bytes from the operating system's generator, taken from the
/// bytes of two v4 UUIDs that carry no version or variant bits, so no new
/// dependency is needed for them.
fn random_bytes() -> [u8; 16] {
    let fully_random = |uuid: uuid::Uuid| {
        uuid.into_bytes()
            .into_iter()
            .enumerate()
            .filter(|&(index, _)| index != 6 && index != 8)
            .map(|(_, byte)| byte)
    };

    let mut bytes = [0u8; 16];
    let pool = fully_random(uuid::Uuid::new_v4()).chain(fully_random(uuid::Uuid::new_v4()));
    for (slot, byte) in bytes.iter_mut().zip(pool) {
        *slot = byte;
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuids_carry_their_version_and_variant() {
        for (kind, version) in [(GeneratedIdKind::UuidV4, 4), (GeneratedIdKind::UuidV7, 7)] {
            let text = kind.generate();
            let parsed = uuid::Uuid::parse_str(&text).expect("a uuid");

            assert_eq!(text, parsed.hyphenated().to_string(), "canonical form");
            assert_eq!(parsed.get_version_num(), version);
            assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
        }
    }

    #[test]
    fn uuid_v7_values_follow_the_clock() {
        let first = GeneratedIdKind::UuidV7.generate();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let second = GeneratedIdKind::UuidV7.generate();

        assert!(first < second, "{first} sorts before {second}");
    }

    #[test]
    fn a_ksuid_is_27_base62_digits_that_decode_to_its_second() {
        let before = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ksuid = GeneratedIdKind::Ksuid.generate();
        let after = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        assert_eq!(ksuid.len(), 27);
        assert!(ksuid.bytes().all(|b| b.is_ascii_alphanumeric()), "{ksuid}");
        let secs = ksuid_unix_secs(&ksuid).expect("decodes");
        assert!(
            (before..=after).contains(&secs),
            "{secs} in {before}..={after}"
        );
    }

    #[test]
    fn ksuid_encoding_matches_the_reference_values() {
        // The smallest and largest KSUIDs of the reference implementation.
        assert_eq!(
            ksuid_at(KSUID_EPOCH, [0; 16]),
            "000000000000000000000000000"
        );
        assert_eq!(
            ksuid_at(KSUID_EPOCH + u64::from(u32::MAX), [0xff; 16]),
            "aWgEPTl1tmebfsQzFP4bxwgy80V"
        );
        assert_eq!(
            ksuid_unix_secs("aWgEPTl1tmebfsQzFP4bxwgy80V"),
            Some(KSUID_EPOCH + u64::from(u32::MAX))
        );
    }

    #[test]
    fn a_ulid_is_26_crockford_digits_led_by_its_millisecond() {
        assert_eq!(ulid_at(0, [0; 10]), "00000000000000000000000000");
        assert_eq!(
            ulid_at(1_469_918_176_385, [0; 10]),
            "01ARYZ6S410000000000000000"
        );

        let ulid = GeneratedIdKind::Ulid.generate();
        assert_eq!(ulid.len(), 26);
        assert!(ulid.bytes().all(|b| CROCKFORD.contains(&b)), "{ulid}");
    }

    #[test]
    fn generated_values_do_not_repeat() {
        for kind in GeneratedIdKind::ALL {
            assert_ne!(kind.generate(), kind.generate(), "{kind:?}");
        }
    }

    #[test]
    fn id_like_columns_are_recognised_by_type_name_or_key() {
        assert!(looks_like_id_column("anything", "uuid", false));
        assert!(looks_like_id_column("ref", "char(27)", false));
        assert!(looks_like_id_column("user_id", "varchar(255)", false));
        assert!(looks_like_id_column("code", "text", true));
        assert!(!looks_like_id_column("title", "varchar(255)", false));
    }

    #[test]
    fn only_columns_that_store_the_text_are_offered_a_kind() {
        use GeneratedIdKind::*;

        assert_eq!(GeneratedIdKind::fitting("uuid"), [UuidV4, UuidV7]);
        assert_eq!(
            GeneratedIdKind::fitting("uniqueidentifier"),
            [UuidV4, UuidV7]
        );
        assert_eq!(GeneratedIdKind::fitting("char(36)"), [UuidV4, UuidV7]);
        assert_eq!(GeneratedIdKind::fitting("char(27)"), [Ksuid]);
        assert_eq!(GeneratedIdKind::fitting("varchar(27)"), [Ksuid, Ulid]);
        assert_eq!(
            GeneratedIdKind::fitting("varchar(255)"),
            GeneratedIdKind::ALL
        );
        assert_eq!(GeneratedIdKind::fitting("text"), GeneratedIdKind::ALL);
        assert_eq!(GeneratedIdKind::fitting("varchar(20)"), []);
        assert_eq!(GeneratedIdKind::fitting("binary(16)"), []);
        assert_eq!(GeneratedIdKind::fitting("integer"), []);
    }
}
