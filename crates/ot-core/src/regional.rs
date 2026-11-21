// Derived from Frappe Books (AGPL-3.0) frappe_books/regional.py @953e9b4, modified:
// the GSTIN pattern is checked by hand rather than with a regular expression.

//! Country rules: Indian states and GSTINs, and how addresses are shown.

use std::fmt;

/// Indian states and union territories by their GST state code, as Frappe Books
/// lists them (the spellings are Frappe's, e.g. "Chattisgarh").
pub const INDIAN_STATES: &[(&str, &str)] = &[
    ("01", "Jammu and Kashmir"),
    ("02", "Himachal Pradesh"),
    ("03", "Punjab"),
    ("04", "Chandigarh"),
    ("05", "Uttarakhand"),
    ("06", "Haryana"),
    ("07", "Delhi"),
    ("08", "Rajasthan"),
    ("09", "Uttar Pradesh"),
    ("10", "Bihar"),
    ("11", "Sikkim"),
    ("12", "Arunachal Pradesh"),
    ("13", "Nagaland"),
    ("14", "Manipur"),
    ("15", "Mizoram"),
    ("16", "Tripura"),
    ("17", "Meghalaya"),
    ("18", "Assam"),
    ("19", "West Bengal"),
    ("20", "Jharkhand"),
    ("21", "Odisha"),
    ("22", "Chattisgarh"),
    ("23", "Madhya Pradesh"),
    ("24", "Gujarat"),
    ("26", "Dadra and Nagar Haveli and Daman and Diu"),
    ("27", "Maharashtra"),
    ("29", "Karnataka"),
    ("30", "Goa"),
    ("31", "Lakshadweep"),
    ("32", "Kerala"),
    ("33", "Tamil Nadu"),
    ("34", "Puducherry"),
    ("35", "Andaman and Nicobar Islands"),
    ("36", "Telangana"),
    ("37", "Andhra Pradesh"),
    ("38", "Ladakh"),
];

/// The state a GST state code stands for.
pub fn indian_state(code: &str) -> Option<&'static str> {
    INDIAN_STATES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|(_, name)| *name)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidGstin;

impl fmt::Display for InvalidGstin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Enter a valid 15-character Indian GSTIN.")
    }
}

impl std::error::Error for InvalidGstin {}

/// Checks a GSTIN and returns it trimmed and in capitals. The pattern is Frappe's
/// `^[0-9]{2}[A-Z]{5}[0-9]{4}[A-Z][1-9A-Z]Z[0-9A-Z]$`, and the first two digits must
/// be a known state code.
pub fn validate_gstin(gstin: &str) -> Result<String, InvalidGstin> {
    let gstin = gstin.trim().to_ascii_uppercase();
    let b = gstin.as_bytes();
    let all = |range: std::ops::Range<usize>, ok: fn(&u8) -> bool| b[range].iter().all(ok);
    let valid = b.len() == 15
        && all(0..2, u8::is_ascii_digit)
        && all(2..7, u8::is_ascii_uppercase)
        && all(7..11, u8::is_ascii_digit)
        && b[11].is_ascii_uppercase()
        && (matches!(b[12], b'1'..=b'9') || b[12].is_ascii_uppercase())
        && b[13] == b'Z'
        && (b[14].is_ascii_digit() || b[14].is_ascii_uppercase())
        && indian_state(&gstin[..2]).is_some();
    if valid { Ok(gstin) } else { Err(InvalidGstin) }
}

/// The parts of an address that make up its display line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressParts<'a> {
    pub line1: &'a str,
    pub line2: Option<&'a str>,
    pub city: &'a str,
    pub state: Option<&'a str>,
    pub country: &'a str,
    pub postal_code: Option<&'a str>,
}

/// The one-line address Frappe shows, e.g. "42 Market Road, Mumbai, Maharashtra,
/// India, 400001". Empty parts are left out.
pub fn address_display(parts: &AddressParts<'_>) -> String {
    [
        Some(parts.line1),
        parts.line2,
        Some(parts.city),
        parts.state,
        Some(parts.country),
        parts.postal_code,
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(", ")
}

/// The place of supply: the state, for an Indian address in a known state.
pub fn place_of_supply(country: &str, state: Option<&str>) -> Option<&'static str> {
    if country != "India" {
        return None;
    }
    let state = state?.trim();
    INDIAN_STATES
        .iter()
        .find(|(_, name)| *name == state)
        .map(|(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from test_books_party: a registered party's GSTIN is checked and normalised.
    #[test]
    fn gstins_are_checked_and_normalised() {
        assert_eq!(
            validate_gstin("27AAAAA0000A1Z5").unwrap(),
            "27AAAAA0000A1Z5"
        );
        assert_eq!(
            validate_gstin(" 27aaaaa0000a1z5 ").unwrap(),
            "27AAAAA0000A1Z5"
        );
        assert_eq!(
            validate_gstin("24AAPFU0939F1ZV").unwrap(),
            "24AAPFU0939F1ZV"
        );
        for bad in [
            "invalid",
            "",
            "27AAAAA0000A1Z",  // 14 characters
            "27AAAAA0000A1Y5", // 14th must be Z
            "27AAAAA0000A0Z5", // 13th cannot be 0
            "2AAAAAA0000A1Z5", // state code must be digits
            "25AAAAA0000A1Z5", // 25 is not a state code
            "99AAAAA0000A1Z5",
            "27AAAA10000A1Z5",
            "27AAAAA0000A1Z5X",
        ] {
            assert_eq!(validate_gstin(bad), Err(InvalidGstin), "{bad:?}");
        }
    }

    // Ported from test_books_address: the display line and the Indian place of supply.
    #[test]
    fn address_display_and_indian_place_of_supply() {
        let parts = AddressParts {
            line1: "42 Market Road",
            line2: Some(""),
            city: "Mumbai",
            state: Some("Maharashtra"),
            country: "India",
            postal_code: Some("400001"),
        };
        assert_eq!(
            address_display(&parts),
            "42 Market Road, Mumbai, Maharashtra, India, 400001"
        );
        assert_eq!(
            place_of_supply("India", Some("Maharashtra")),
            Some("Maharashtra")
        );
        assert_eq!(place_of_supply("India", Some("Bavaria")), None);
        assert_eq!(place_of_supply("Germany", Some("Maharashtra")), None);
        assert_eq!(indian_state("24"), Some("Gujarat"));
    }
}
