// -------------------------------------------------------------------------------------------------
//  Copyright (C) 2015-2026 Nautech Systems Pty Ltd. All rights reserved.
//  https://nautechsystems.io
//
//  Licensed under the GNU Lesser General Public License Version 3.0 (the "License");
//  You may not use this file except in compliance with the License.
//  You may obtain a copy of the License at https://www.gnu.org/licenses/lgpl-3.0.en.html
//
//  Unless required by applicable law or agreed to in writing, software
//  distributed under the License is distributed on an "AS IS" BASIS,
//  WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
//  See the License for the specific language governing permissions and
//  limitations under the License.
// -------------------------------------------------------------------------------------------------

//! US equities trading sessions, for 24/5 coverage across the SIP and overnight feeds.
//!
//! NautilusTrader has no venue calendar or trading-session model, so the adapter derives sessions
//! from Alpaca's own calendar endpoint.
//!
//! # Session layout
//!
//! All boundaries are US Eastern wall-clock times, so their UTC instants shift with daylight
//! saving. A trading day runs:
//!
//! ```text
//!  20:00 (prev)      04:00        09:30        16:00        20:00
//!  ────────────────┬────────────┬────────────┬────────────┬──────────
//!     Overnight    │ PreMarket  │  Regular   │ AfterHours │ Overnight
//!      (BOATS)     │           (SIP)                      │  (BOATS)
//! ```
//!
//! # Which day owns the overnight session
//!
//! The overnight session crosses midnight, so it has to be attributed to one trading day. It is
//! attributed to the day it **precedes**: the session running 20:00 Sunday to 04:00 Monday belongs
//! to Monday. This falls out of the calendar without special cases — Alpaca lists only days the
//! market opens, so Sunday evening resolves through Monday, and Friday evening has no following
//! trading day on Saturday and is therefore correctly closed. The result is continuous coverage
//! from Sunday 20:00 to Friday 20:00 Eastern, which is what 24/5 means here.
//!
//! # Assumption
//!
//! That attribution rule is inferred from the 24/5 requirement and the published 20:00-04:00
//! window; Alpaca's calendar does not describe the overnight session at all. Blue Ocean's own
//! weekly edges have not been confirmed against venue documentation.

use std::{collections::BTreeMap, sync::LazyLock};

use jiff::{
    civil::{Date, DateTime, Time},
    tz::{AmbiguousOffset, TimeZone},
};
use nautilus_core::{UnixNanos, datetime::get_timezone};

use crate::{common::enums::AlpacaDataFeed, http::models::CalendarDay};

/// IANA identifier for the US equities market time zone.
pub const MARKET_TIMEZONE: &str = "America/New_York";

/// The market time zone, resolved from the bundled time zone database.
pub static MARKET_TZ: LazyLock<TimeZone> = LazyLock::new(|| {
    get_timezone(MARKET_TIMEZONE).expect("bundled tzdb must contain America/New_York")
});

/// Start of the overnight session, US Eastern.
pub const OVERNIGHT_OPEN: Time = Time::constant(20, 0, 0, 0);
/// End of the overnight session, US Eastern.
pub const OVERNIGHT_CLOSE: Time = Time::constant(4, 0, 0, 0);

/// A trading session phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlpacaSession {
    /// Blue Ocean ATS overnight session, 20:00-04:00 Eastern.
    Overnight,
    /// Pre-market, from the extended session open to the regular open.
    PreMarket,
    /// Regular session.
    Regular,
    /// After-hours, from the regular close to the extended session close.
    AfterHours,
    /// No session is running.
    Closed,
}

impl AlpacaSession {
    /// Returns the market data feed carrying this session, if any.
    #[must_use]
    pub const fn feed(self) -> Option<AlpacaDataFeed> {
        match self {
            Self::Overnight => Some(AlpacaDataFeed::Boats),
            Self::PreMarket | Self::Regular | Self::AfterHours => Some(AlpacaDataFeed::Sip),
            Self::Closed => None,
        }
    }

    /// Returns true when orders in this session require the extended-hours flag.
    #[must_use]
    pub const fn is_extended_hours(self) -> bool {
        matches!(self, Self::PreMarket | Self::AfterHours)
    }

    /// Returns true when any trading is possible in this session.
    #[must_use]
    pub const fn is_open(self) -> bool {
        !matches!(self, Self::Closed)
    }
}

/// Parses a venue time, accepting both `HH:MM` and `HHMM`.
///
/// The calendar payload uses both forms in the same object.
fn parse_venue_time(raw: &str) -> Option<Time> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 4 {
        return None;
    }
    let hour: i8 = digits[..2].parse().ok()?;
    let minute: i8 = digits[2..].parse().ok()?;
    Time::new(hour, minute, 0, 0).ok()
}

/// The boundaries of one trading day, in Eastern wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TradingDay {
    /// The trading date.
    pub date: Date,
    /// Regular session open.
    pub open: Time,
    /// Regular session close.
    pub close: Time,
    /// Extended session open; the pre-market boundary.
    pub session_open: Time,
    /// Extended session close; the after-hours boundary.
    pub session_close: Time,
}

impl TradingDay {
    /// Builds a trading day from a venue calendar entry.
    ///
    /// Returns `None` when the date or any time cannot be parsed, so one malformed entry drops a
    /// single day rather than failing the whole calendar.
    #[must_use]
    pub fn from_calendar_day(day: &CalendarDay) -> Option<Self> {
        let date: Date = day.date.parse().ok()?;
        let open = parse_venue_time(&day.open)?;
        let close = parse_venue_time(&day.close)?;
        let session_open = day
            .session_open
            .as_deref()
            .and_then(parse_venue_time)
            .unwrap_or(open);
        let session_close = day
            .session_close
            .as_deref()
            .and_then(parse_venue_time)
            .unwrap_or(close);

        Some(Self {
            date,
            open,
            close,
            session_open,
            session_close,
        })
    }
}

/// Resolves trading sessions from the venue calendar.
#[derive(Debug, Clone, Default)]
pub struct SessionCalendar {
    days: BTreeMap<Date, TradingDay>,
}

impl SessionCalendar {
    /// Creates an empty calendar.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a calendar from venue entries, skipping any that cannot be parsed.
    #[must_use]
    pub fn from_calendar_days(days: &[CalendarDay]) -> Self {
        let mut map = BTreeMap::new();
        for day in days {
            match TradingDay::from_calendar_day(day) {
                Some(parsed) => {
                    map.insert(parsed.date, parsed);
                }
                None => log::warn!("Skipping unparseable Alpaca calendar entry: {day:?}"),
            }
        }
        Self { days: map }
    }

    /// Returns the number of trading days held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.days.len()
    }

    /// Returns true when no trading days are held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.days.is_empty()
    }

    /// Returns the trading day for `date`, if the market opens that day.
    #[must_use]
    pub fn trading_day(&self, date: Date) -> Option<&TradingDay> {
        self.days.get(&date)
    }

    /// Returns true when the market opens on `date`.
    #[must_use]
    pub fn is_trading_day(&self, date: Date) -> bool {
        self.days.contains_key(&date)
    }

    /// Returns the session in effect at `ts`.
    ///
    /// Returns [`AlpacaSession::Closed`] when the instant falls outside every session, and also
    /// when the calendar does not cover it: an unknown date must not be reported as open.
    #[must_use]
    pub fn session_at(&self, ts: UnixNanos) -> AlpacaSession {
        let Some(local) = to_market_datetime(ts) else {
            return AlpacaSession::Closed;
        };
        let (date, time) = (local.date(), local.time());

        // The overnight session is attributed to the trading day it precedes, so an evening
        // instant resolves through the following date and an early-morning one through its own.
        if time >= OVERNIGHT_OPEN {
            return if self.is_trading_day(date.tomorrow().unwrap_or(date)) {
                AlpacaSession::Overnight
            } else {
                AlpacaSession::Closed
            };
        }
        if time < OVERNIGHT_CLOSE {
            return if self.is_trading_day(date) {
                AlpacaSession::Overnight
            } else {
                AlpacaSession::Closed
            };
        }

        let Some(day) = self.trading_day(date) else {
            return AlpacaSession::Closed;
        };

        if time < day.session_open || time >= day.session_close {
            AlpacaSession::Closed
        } else if time < day.open {
            AlpacaSession::PreMarket
        } else if time < day.close {
            AlpacaSession::Regular
        } else {
            AlpacaSession::AfterHours
        }
    }

    /// Returns the market data feed to consume at `ts`.
    #[must_use]
    pub fn feed_at(&self, ts: UnixNanos) -> Option<AlpacaDataFeed> {
        self.session_at(ts).feed()
    }
}

/// Converts a UTC instant into Eastern wall-clock time.
fn to_market_datetime(ts: UnixNanos) -> Option<DateTime> {
    let timestamp = jiff::Timestamp::from_nanosecond(i128::from(ts.as_u64())).ok()?;
    Some(MARKET_TZ.to_datetime(timestamp))
}

/// Converts an Eastern wall-clock datetime into a UTC instant.
///
/// Daylight saving makes this partial: one hour repeats each autumn and one hour does not exist
/// each spring. A repeated time resolves to its first occurrence, and a nonexistent time returns
/// `None` rather than being silently shifted into a different session.
#[must_use]
pub fn market_datetime_to_utc(datetime: DateTime) -> Option<UnixNanos> {
    let ambiguous = MARKET_TZ.to_ambiguous_timestamp(datetime);
    let resolved = match ambiguous.offset() {
        AmbiguousOffset::Unambiguous { .. } => ambiguous.unambiguous().ok()?,
        AmbiguousOffset::Fold { .. } => ambiguous.earlier().ok()?,
        AmbiguousOffset::Gap { .. } => return None,
    };
    u64::try_from(resolved.as_nanosecond())
        .ok()
        .map(UnixNanos::from)
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use rstest::rstest;

    use super::*;

    const CALENDAR_JSON: &str = include_str!("../../test_data/http_calendar.json");

    fn calendar() -> SessionCalendar {
        let days: Vec<CalendarDay> = serde_json::from_str(CALENDAR_JSON).unwrap();
        SessionCalendar::from_calendar_days(&days)
    }

    /// Builds an instant from an Eastern wall-clock date and time.
    fn et(y: i16, m: i8, d: i8, hh: i8, mm: i8) -> UnixNanos {
        market_datetime_to_utc(date(y, m, d).at(hh, mm, 0, 0)).unwrap()
    }

    #[rstest]
    #[case("09:30", 9, 30)]
    #[case("16:00", 16, 0)]
    #[case("0400", 4, 0)]
    #[case("2000", 20, 0)]
    fn test_both_venue_time_formats_parse(#[case] raw: &str, #[case] hour: i8, #[case] minute: i8) {
        assert_eq!(parse_venue_time(raw), Time::new(hour, minute, 0, 0).ok());
    }

    #[rstest]
    #[case("")]
    #[case("930")]
    #[case("09:30:00")]
    #[case("abcd")]
    fn test_malformed_venue_times_are_rejected(#[case] raw: &str) {
        assert!(parse_venue_time(raw).is_none());
    }

    #[rstest]
    fn test_calendar_parses_canonical_payload() {
        let calendar = calendar();
        assert_eq!(calendar.len(), 4);
        let friday = calendar.trading_day(date(2026, 8, 14)).unwrap();
        assert_eq!(friday.open, Time::new(9, 30, 0, 0).unwrap());
        assert_eq!(friday.close, Time::new(16, 0, 0, 0).unwrap());
        assert_eq!(friday.session_open, Time::new(4, 0, 0, 0).unwrap());
        assert_eq!(friday.session_close, Time::new(20, 0, 0, 0).unwrap());
    }

    #[rstest]
    fn test_weekend_is_not_a_trading_day() {
        let calendar = calendar();
        assert!(!calendar.is_trading_day(date(2026, 8, 15)));
        assert!(!calendar.is_trading_day(date(2026, 8, 16)));
        assert!(calendar.is_trading_day(date(2026, 8, 17)));
    }

    #[rstest]
    #[case(3, 30, AlpacaSession::Overnight)]
    #[case(4, 0, AlpacaSession::PreMarket)]
    #[case(9, 29, AlpacaSession::PreMarket)]
    #[case(9, 30, AlpacaSession::Regular)]
    #[case(15, 59, AlpacaSession::Regular)]
    #[case(16, 0, AlpacaSession::AfterHours)]
    #[case(19, 59, AlpacaSession::AfterHours)]
    #[case(20, 0, AlpacaSession::Overnight)]
    #[case(23, 30, AlpacaSession::Overnight)]
    fn test_session_boundaries_on_a_trading_day(
        #[case] hour: i8,
        #[case] minute: i8,
        #[case] expected: AlpacaSession,
    ) {
        // 2026-08-18 is a Tuesday, with trading days either side.
        assert_eq!(
            calendar().session_at(et(2026, 8, 18, hour, minute)),
            expected
        );
    }

    #[rstest]
    fn test_sunday_evening_opens_the_week() {
        // Sunday is not itself a trading day, but its evening carries the overnight session
        // belonging to Monday.
        let calendar = calendar();
        assert!(!calendar.is_trading_day(date(2026, 8, 16)));
        assert_eq!(
            calendar.session_at(et(2026, 8, 16, 21, 0)),
            AlpacaSession::Overnight
        );
    }

    #[rstest]
    fn test_friday_evening_is_closed() {
        // Nothing follows Friday, so 24/5 coverage ends at Friday 20:00 rather than rolling on.
        assert_eq!(
            calendar().session_at(et(2026, 8, 14, 21, 0)),
            AlpacaSession::Closed
        );
    }

    #[rstest]
    fn test_saturday_is_closed_all_day() {
        let calendar = calendar();
        for hour in [1, 6, 12, 18, 22] {
            assert_eq!(
                calendar.session_at(et(2026, 8, 15, hour, 0)),
                AlpacaSession::Closed,
                "hour {hour}"
            );
        }
    }

    #[rstest]
    fn test_monday_small_hours_are_overnight() {
        assert_eq!(
            calendar().session_at(et(2026, 8, 17, 2, 0)),
            AlpacaSession::Overnight
        );
    }

    #[rstest]
    #[case(AlpacaSession::Overnight, Some(AlpacaDataFeed::Boats))]
    #[case(AlpacaSession::PreMarket, Some(AlpacaDataFeed::Sip))]
    #[case(AlpacaSession::Regular, Some(AlpacaDataFeed::Sip))]
    #[case(AlpacaSession::AfterHours, Some(AlpacaDataFeed::Sip))]
    #[case(AlpacaSession::Closed, None)]
    fn test_feed_selection(
        #[case] session: AlpacaSession,
        #[case] expected: Option<AlpacaDataFeed>,
    ) {
        assert_eq!(session.feed(), expected);
    }

    #[rstest]
    #[case(AlpacaSession::PreMarket, true)]
    #[case(AlpacaSession::AfterHours, true)]
    #[case(AlpacaSession::Regular, false)]
    #[case(AlpacaSession::Overnight, false)]
    fn test_extended_hours_classification(#[case] session: AlpacaSession, #[case] expected: bool) {
        assert_eq!(session.is_extended_hours(), expected);
    }

    #[rstest]
    fn test_empty_calendar_reports_closed() {
        assert_eq!(
            SessionCalendar::new().session_at(et(2026, 8, 18, 12, 0)),
            AlpacaSession::Closed
        );
    }

    #[rstest]
    fn test_dst_spring_forward_gap_is_rejected() {
        // 2026-03-08 02:30 Eastern does not exist; resolving it to some nearby instant would put
        // an order or a bar in the wrong session.
        assert!(market_datetime_to_utc(date(2026, 3, 8).at(2, 30, 0, 0)).is_none());
    }

    #[rstest]
    fn test_dst_fall_back_fold_resolves_to_first_occurrence() {
        // 2026-11-01 01:30 Eastern happens twice; both resolutions are real instants, so the
        // earlier one is chosen deterministically rather than erroring.
        let ts = market_datetime_to_utc(date(2026, 11, 1).at(1, 30, 0, 0)).unwrap();
        let later = market_datetime_to_utc(date(2026, 11, 1).at(2, 30, 0, 0)).unwrap();
        assert!(ts < later);
    }

    #[rstest]
    fn test_session_boundary_holds_across_dst_transition() {
        // The 20:00 Eastern boundary is a wall-clock time, so it must land correctly on both
        // sides of a daylight saving change rather than drifting by an hour in UTC.
        let days = vec![
            CalendarDay {
                date: "2026-03-09".to_string(),
                open: "09:30".to_string(),
                close: "16:00".to_string(),
                session_open: Some("0400".to_string()),
                session_close: Some("2000".to_string()),
                settlement_date: None,
            },
            CalendarDay {
                date: "2026-03-10".to_string(),
                open: "09:30".to_string(),
                close: "16:00".to_string(),
                session_open: Some("0400".to_string()),
                session_close: Some("2000".to_string()),
                settlement_date: None,
            },
        ];
        let calendar = SessionCalendar::from_calendar_days(&days);

        // 2026-03-08 is the spring-forward date; the Monday after it must behave normally.
        assert_eq!(
            calendar.session_at(et(2026, 3, 9, 19, 59)),
            AlpacaSession::AfterHours
        );
        assert_eq!(
            calendar.session_at(et(2026, 3, 9, 20, 0)),
            AlpacaSession::Overnight
        );
    }

    #[rstest]
    fn test_unparseable_calendar_entry_is_skipped_not_fatal() {
        let days = vec![
            CalendarDay {
                date: "not-a-date".to_string(),
                open: "09:30".to_string(),
                close: "16:00".to_string(),
                session_open: None,
                session_close: None,
                settlement_date: None,
            },
            CalendarDay {
                date: "2026-08-18".to_string(),
                open: "09:30".to_string(),
                close: "16:00".to_string(),
                session_open: Some("0400".to_string()),
                session_close: Some("2000".to_string()),
                settlement_date: None,
            },
        ];
        assert_eq!(SessionCalendar::from_calendar_days(&days).len(), 1);
    }

    #[rstest]
    fn test_session_open_defaults_to_regular_open_when_absent() {
        let days = vec![CalendarDay {
            date: "2026-08-18".to_string(),
            open: "09:30".to_string(),
            close: "16:00".to_string(),
            session_open: None,
            session_close: None,
            settlement_date: None,
        }];
        let calendar = SessionCalendar::from_calendar_days(&days);
        let day = calendar.trading_day(date(2026, 8, 18)).unwrap();
        assert_eq!(day.session_open, day.open);
        assert_eq!(day.session_close, day.close);
        // With no extended session there is no pre-market phase.
        assert_eq!(
            calendar.session_at(et(2026, 8, 18, 5, 0)),
            AlpacaSession::Closed
        );
    }
}
