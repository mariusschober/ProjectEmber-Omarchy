use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveTime, Offset, TimeZone, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};

use crate::models::epoch_seconds;

/// Local solar coordinates. Rounded to 0.1° before storage (≈11 km), matching
/// the macOS privacy posture.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolarCoordinate {
    pub latitude: f64,
    pub longitude: f64,
}

impl SolarCoordinate {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        Self {
            latitude: latitude.clamp(-90.0, 90.0),
            longitude: longitude.clamp(-180.0, 180.0),
        }
    }

    pub fn rounded(self, places: u32) -> Self {
        let scale = 10f64.powi(places.min(6) as i32);
        Self {
            latitude: (self.latitude * scale).round() / scale,
            longitude: (self.longitude * scale).round() / scale,
        }
    }

    pub fn is_valid(self) -> bool {
        self.latitude.is_finite()
            && self.longitude.is_finite()
            && (-90.0..=90.0).contains(&self.latitude)
            && (-180.0..=180.0).contains(&self.longitude)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CachedSolarLocation {
    pub coordinate: SolarCoordinate,
    #[serde(with = "epoch_seconds")]
    pub captured_at: DateTime<Utc>,
}

impl CachedSolarLocation {
    pub fn new(coordinate: SolarCoordinate, captured_at: DateTime<Utc>) -> Self {
        Self {
            coordinate: coordinate.rounded(1),
            captured_at,
        }
    }

    pub fn age(&self, now: DateTime<Utc>) -> Duration {
        now.signed_duration_since(self.captured_at)
    }

    pub fn is_fresh(&self, now: DateTime<Utc>, max_age: Duration) -> bool {
        let age = self.age(now);
        age >= Duration::seconds(-60) && age <= max_age
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SolarEventKind {
    Sunrise,
    Sunset,
}

impl SolarEventKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Sunrise => "Sunrise",
            Self::Sunset => "Sunset",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolarEvent {
    pub kind: SolarEventKind,
    #[serde(with = "epoch_seconds")]
    pub date: DateTime<Utc>,
}

impl SolarEvent {
    pub fn new(kind: SolarEventKind, date: DateTime<Utc>) -> Self {
        Self { kind, date }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolarDayCondition {
    Normal {
        sunrise: DateTime<Utc>,
        sunset: DateTime<Utc>,
    },
    SunAlwaysAboveHorizon,
    SunAlwaysBelowHorizon,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolarSchedule {
    pub is_night: bool,
    pub next_event: Option<SolarEvent>,
}

/// NOAA solar calculator (90.833° sunrise/sunset zenith), ported from the macOS
/// implementation. All maths run on-device; no network, no place names.
pub struct SolarCalculator;

impl SolarCalculator {
    pub const SUNRISE_ZENITH_DEGREES: f64 = 90.833;
    /// A transition must be at least this far in the future to be "next";
    /// prevents an event that fired one frame ago from being replayed.
    pub const FUTURE_MARGIN: Duration = Duration::milliseconds(500);

    pub fn schedule(at: DateTime<Utc>, coordinate: SolarCoordinate, tz: Tz) -> SolarSchedule {
        let condition = Self::day_condition(at, coordinate, tz);
        let is_night = match condition {
            SolarDayCondition::Normal { sunrise, sunset } => at < sunrise || at >= sunset,
            SolarDayCondition::SunAlwaysAboveHorizon => false,
            SolarDayCondition::SunAlwaysBelowHorizon => true,
        };

        let wanted = if is_night {
            SolarEventKind::Sunrise
        } else {
            SolarEventKind::Sunset
        };
        let local = at.with_timezone(&tz);
        let start = local
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|naive| tz.from_local_datetime(&naive).earliest())
            .unwrap_or(local);

        let mut next_event = None;
        for offset in 0..=370_i64 {
            let candidate_day = start + Duration::days(offset);
            let day = candidate_day.with_timezone(&tz).date_naive();
            let Ok(day_condition) = Self::day_condition_for_date(day, coordinate, tz) else {
                continue;
            };
            let candidate = match (wanted, day_condition) {
                (SolarEventKind::Sunrise, SolarDayCondition::Normal { sunrise, .. }) => {
                    SolarEvent::new(SolarEventKind::Sunrise, sunrise)
                }
                (SolarEventKind::Sunset, SolarDayCondition::Normal { sunset, .. }) => {
                    SolarEvent::new(SolarEventKind::Sunset, sunset)
                }
                _ => continue,
            };
            if candidate.date > at + Self::FUTURE_MARGIN {
                next_event = Some(candidate);
                break;
            }
        }

        SolarSchedule {
            is_night,
            next_event,
        }
    }

    pub fn day_condition(
        date: DateTime<Utc>,
        coordinate: SolarCoordinate,
        tz: Tz,
    ) -> SolarDayCondition {
        let local = date.with_timezone(&tz);
        match Self::day_condition_for_date(local.date_naive(), coordinate, tz) {
            Ok(condition) => condition,
            Err(()) => SolarDayCondition::SunAlwaysBelowHorizon,
        }
    }

    fn day_condition_for_date(
        date: NaiveDate,
        coordinate: SolarCoordinate,
        tz: Tz,
    ) -> Result<SolarDayCondition, ()> {
        let start_of_day = tz
            .from_local_datetime(&date.and_hms_opt(0, 0, 0).ok_or(())?)
            .earliest()
            .ok_or(())?;
        let local_noon = start_of_day + Duration::hours(12);
        let day_of_year = local_noon.ordinal();
        let days_in_year = if local_noon.date_naive().leap_year() {
            366
        } else {
            365
        };

        let fractional_year =
            (2.0 * std::f64::consts::PI / f64::from(days_in_year)) * f64::from(day_of_year - 1);
        let equation_of_time = 229.18
            * (0.000_075 + (0.001_868 * fractional_year.cos())
                - (0.032_077 * fractional_year.sin())
                - (0.014_615 * (2.0 * fractional_year).cos())
                - (0.040_849 * (2.0 * fractional_year).sin()));
        let declination = 0.006_918 - (0.399_912 * fractional_year.cos())
            + (0.070_257 * fractional_year.sin())
            - (0.006_758 * (2.0 * fractional_year).cos())
            + (0.000_907 * (2.0 * fractional_year).sin())
            - (0.002_697 * (3.0 * fractional_year).cos())
            + (0.001_48 * (3.0 * fractional_year).sin());

        let latitude = coordinate.latitude.to_radians();
        let zenith = Self::SUNRISE_ZENITH_DEGREES.to_radians();

        // Exact poles make the hour-angle formula singular. The sun's altitude
        // there is essentially the declination, so decide from its sign.
        if coordinate.latitude.abs() >= 89.5 {
            let above = if coordinate.latitude > 0.0 {
                declination > 0.0
            } else {
                declination < 0.0
            };
            return Ok(if above {
                SolarDayCondition::SunAlwaysAboveHorizon
            } else {
                SolarDayCondition::SunAlwaysBelowHorizon
            });
        }

        let cosine_hour_angle = (zenith.cos() / (latitude.cos() * declination.cos()))
            - (latitude.tan() * declination.tan());

        if !cosine_hour_angle.is_finite() || cosine_hour_angle > 1.0 {
            return Ok(SolarDayCondition::SunAlwaysBelowHorizon);
        }
        if cosine_hour_angle < -1.0 {
            return Ok(SolarDayCondition::SunAlwaysAboveHorizon);
        }

        let hour_angle_degrees = cosine_hour_angle.acos().to_degrees();
        let offset_minutes = f64::from(local_noon.offset().fix().local_minus_utc()) / 60.0;
        let solar_noon_minutes =
            720.0 - (4.0 * coordinate.longitude) - equation_of_time + offset_minutes;
        let sunrise_minutes = solar_noon_minutes - (4.0 * hour_angle_degrees);
        let sunset_minutes = solar_noon_minutes + (4.0 * hour_angle_degrees);

        let sunrise = Self::event_date(date, sunrise_minutes, tz).ok_or(())?;
        let sunset = Self::event_date(date, sunset_minutes, tz).ok_or(())?;
        Ok(SolarDayCondition::Normal { sunrise, sunset })
    }

    fn event_date(date: NaiveDate, minutes_after_midnight: f64, tz: Tz) -> Option<DateTime<Utc>> {
        if !minutes_after_midnight.is_finite() {
            return None;
        }
        let total_seconds = (minutes_after_midnight * 60.0).round() as i64;
        let day_offset = total_seconds.div_euclid(86_400);
        let seconds_in_day = total_seconds.rem_euclid(86_400);
        let event_day = date + Duration::days(day_offset);
        let time = NaiveTime::from_num_seconds_from_midnight_opt(seconds_in_day as u32, 0)?;
        let naive = event_day.and_time(time);
        match tz.from_local_datetime(&naive) {
            chrono::LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
            chrono::LocalResult::Ambiguous(earlier, _later) => Some(earlier.with_timezone(&Utc)),
            // A wall-clock time skipped by DST cannot be a solar event in
            // practice; fall back to the UTC instant that would have been used.
            chrono::LocalResult::None => tz.from_utc_datetime(&naive).with_timezone(&Utc).into(),
        }
    }

    /// System local time zone, falling back to UTC when it cannot be resolved.
    pub fn local_timezone() -> Tz {
        iana_time_zone::get_timezone()
            .ok()
            .and_then(|name| name.parse::<Tz>().ok())
            .unwrap_or(chrono_tz::UTC)
    }

    /// Formats the next solar event in local time for UI copy.
    pub fn format_event(event: &SolarEvent, tz: Tz) -> String {
        let local = event.date.with_timezone(&tz);
        let suffix = local.format("%H:%M").to_string();
        format!("{} at {}", event.kind.label(), suffix)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Timelike};

    fn utc(year: i32, month: u32, day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(year, month, day, hour, 0, 0).unwrap()
    }

    #[test]
    fn coordinate_rounding_and_validation() {
        let c = SolarCoordinate::new(52.52, 13.405).rounded(1);
        assert_eq!(c.latitude, 52.5);
        assert_eq!(c.longitude, 13.4);
        assert!(!SolarCoordinate::new(f64::NAN, 0.0).is_valid());
        assert!(SolarCoordinate::new(100.0, 200.0).is_valid());
        assert_eq!(SolarCoordinate::new(100.0, 200.0).latitude, 90.0);
    }

    #[test]
    fn berlin_autumn_sunset_is_plausible() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let date = utc(2026, 11, 8, 17);
        let schedule = SolarCalculator::schedule(date, SolarCoordinate::new(52.5, 13.4), tz);
        assert!(schedule.is_night, "17:00 UTC in November is after sunset");
        let next = schedule.next_event.expect("sunrise must exist");
        assert_eq!(next.kind, SolarEventKind::Sunrise);
        assert!(next.date > date);
        let local = next.date.with_timezone(&tz);
        assert!((6..=8).contains(&local.hour()), "sunrise hour {local}");
    }

    #[test]
    fn midday_summer_is_day_and_next_event_is_sunset() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let date = utc(2026, 6, 21, 10);
        let schedule = SolarCalculator::schedule(date, SolarCoordinate::new(52.5, 13.4), tz);
        assert!(!schedule.is_night);
        assert_eq!(schedule.next_event.unwrap().kind, SolarEventKind::Sunset);
    }

    #[test]
    fn dst_boundary_keeps_events_on_the_local_day() {
        let tz: Tz = "America/New_York".parse().unwrap();
        let date = tz
            .with_ymd_and_hms(2026, 3, 8, 12, 0, 0)
            .unwrap()
            .with_timezone(&Utc);
        let condition = SolarCalculator::day_condition(date, SolarCoordinate::new(40.7, -74.0), tz);
        match condition {
            SolarDayCondition::Normal { sunrise, sunset } => {
                assert_eq!(sunrise.with_timezone(&tz).day(), 8);
                assert_eq!(sunset.with_timezone(&tz).day(), 8);
            }
            other => panic!("expected normal day, got {other:?}"),
        }
    }

    #[test]
    fn polar_conditions_do_not_fabricate_events() {
        let tz: Tz = "Arctic/Longyearbyen".parse().unwrap();
        let arctic = SolarCoordinate::new(78.2, 15.6);
        let summer = utc(2026, 6, 21, 12);
        let winter = utc(2026, 12, 21, 12);
        assert_eq!(
            SolarCalculator::day_condition(summer, arctic, tz),
            SolarDayCondition::SunAlwaysAboveHorizon
        );
        assert_eq!(
            SolarCalculator::day_condition(winter, arctic, tz),
            SolarDayCondition::SunAlwaysBelowHorizon
        );
        let schedule = SolarCalculator::schedule(winter, arctic, tz);
        assert!(schedule.is_night);
        // The next actual sunrise is months away; the polar night itself must
        // not fabricate an event on the same day.
        if let Some(event) = schedule.next_event {
            assert!(event.date > winter + Duration::days(30));
        }
    }

    #[test]
    fn exact_pole_is_finite() {
        let tz: Tz = "UTC".parse().unwrap();
        let north = SolarCoordinate::new(90.0, 0.0);
        let schedule = SolarCalculator::schedule(utc(2026, 6, 21, 12), north, tz);
        assert!(!schedule.is_night);
    }

    #[test]
    fn cached_location_freshness() {
        let now = Utc::now();
        let fresh =
            CachedSolarLocation::new(SolarCoordinate::new(1.0, 2.0), now - Duration::minutes(4));
        assert!(fresh.is_fresh(now, Duration::minutes(5)));
        let stale =
            CachedSolarLocation::new(SolarCoordinate::new(1.0, 2.0), now - Duration::minutes(6));
        assert!(!stale.is_fresh(now, Duration::minutes(5)));
    }

    #[test]
    fn next_event_is_always_in_the_future() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let coord = SolarCoordinate::new(52.5, 13.4);
        for hour in [0, 3, 6, 9, 12, 15, 18, 21, 23] {
            let at = utc(2026, 11, 8, hour);
            let schedule = SolarCalculator::schedule(at, coord, tz);
            if let Some(event) = schedule.next_event {
                assert!(event.date > at);
                assert!(event.date < at + Duration::days(2));
            }
        }
    }

    #[test]
    fn event_format_uses_local_time() {
        let tz: Tz = "Europe/Berlin".parse().unwrap();
        let event = SolarEvent::new(
            SolarEventKind::Sunset,
            tz.with_ymd_and_hms(2026, 11, 8, 16, 22, 0)
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(SolarCalculator::format_event(&event, tz), "Sunset at 16:22");
    }
}
