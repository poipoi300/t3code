//! IANA zones from platform zone data (feature `iana`): offsets and local times on both sides of a
//! daylight-saving change, and a zone without one.
#![cfg(feature = "iana")]

use t3_i18n::format::{TimeZone, civil};

/// 2024-01-15T12:00:00Z and 2024-07-04T12:00:00Z.
const WINTER: i64 = 1_705_320_000_000;
const SUMMER: i64 = 1_720_094_400_000;
/// New York springs forward at 2024-03-10T07:00:00Z (02:00 EST becomes 03:00 EDT).
const BEFORE_CHANGE: i64 = 1_710_053_999_000;
const AT_CHANGE: i64 = 1_710_054_000_000;

#[test]
fn new_york_follows_daylight_saving_and_kolkata_has_none() {
    let new_york = TimeZone::Iana("America/New_York".into());
    let kolkata = TimeZone::Iana("Asia/Kolkata".into());
    assert!(
        new_york.is_known(),
        "the platform zone data resolves America/New_York"
    );
    assert!(
        kolkata.is_known(),
        "the platform zone data resolves Asia/Kolkata"
    );

    assert_eq!(new_york.offset_seconds(WINTER), -5 * 3600);
    assert_eq!(new_york.offset_seconds(SUMMER), -4 * 3600);
    assert_eq!(new_york.offset_seconds(BEFORE_CHANGE), -5 * 3600);
    assert_eq!(new_york.offset_seconds(AT_CHANGE), -4 * 3600);
    assert_eq!(kolkata.offset_seconds(WINTER), 5 * 3600 + 30 * 60);
    assert_eq!(kolkata.offset_seconds(SUMMER), 5 * 3600 + 30 * 60);

    let summer_in_new_york = civil(SUMMER, &new_york);
    assert_eq!(
        (
            summer_in_new_york.day.month,
            summer_in_new_york.day.day,
            summer_in_new_york.hour
        ),
        (7, 4, 8)
    );
    let winter_in_kolkata = civil(WINTER, &kolkata);
    assert_eq!(
        (
            winter_in_kolkata.day.day,
            winter_in_kolkata.hour,
            winter_in_kolkata.minute
        ),
        (15, 17, 30)
    );
}
