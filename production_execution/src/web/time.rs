//! Company-time entry shared by the floor panels, mirroring FloorLive.zone,
//! local_time and corrected_time. Storage remains UTC. Local display mode uses
//! the company zone for entry (as Bilimbi does), UTC mode uses UTC. A fold
//! chooses its first instant; a gap is refused instead of silently moved.
//!
//! The labour panel consumes this contract. The runnable floor-pages
//! walkthrough includes this exact private source to prove entry conversions.
use bilibi_datetime::{DisplayContext, Mode};
use jiff::{
    civil::DateTime,
    tz::{Disambiguation, TimeZone},
    Timestamp,
};

pub(super) fn zone_name(display: &DisplayContext) -> &str {
    if display.mode == Mode::Utc {
        "UTC"
    } else {
        &display.timezone
    }
}
pub(super) fn parse(display: &DisplayContext, value: &str) -> Option<Timestamp> {
    let local = value.parse::<DateTime>().ok()?;
    let zone = TimeZone::get(zone_name(display)).ok()?;
    let instant = zone
        .to_ambiguous_timestamp(local)
        .disambiguate(Disambiguation::Earlier)
        .ok()?;
    // Earlier resolves a fold, but must not manufacture a time inside a gap.
    (instant.to_zoned(zone).datetime() == local).then_some(instant)
}
pub(super) fn input(display: &DisplayContext, instant: Timestamp) -> Option<String> {
    let zone = TimeZone::get(zone_name(display)).ok()?;
    Some(
        instant
            .to_zoned(zone)
            .strftime("%Y-%m-%dT%H:%M")
            .to_string(),
    )
}
pub(super) fn corrected(
    display: &DisplayContext,
    value: &str,
    original: Option<Timestamp>,
) -> Option<Option<Timestamp>> {
    if original.and_then(|t| input(display, t)).as_deref() == Some(value) {
        return Some(original);
    }
    if value.is_empty() {
        Some(None)
    } else {
        parse(display, value).map(Some)
    }
}
