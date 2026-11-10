use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use ember_core::{CachedSolarLocation, SolarCoordinate};
use log::debug;
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::OwnedObjectPath;

pub const MAX_FIX_AGE: Duration = Duration::from_secs(5 * 60);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocationError {
    /// The desktop location service refused access.
    Denied,
    /// GeoClue is not installed or not running.
    Unavailable(String),
    /// A temporary failure; retry with backoff.
    Transient(String),
}

impl LocationError {
    pub fn message(&self) -> String {
        match self {
            Self::Denied => {
                "Location access was denied. Set your coordinates manually or allow the desktop \
                 location service."
                    .into()
            }
            Self::Unavailable(reason) => {
                format!("The desktop location service is unavailable: {reason}")
            }
            Self::Transient(reason) => format!("Location is temporarily unavailable: {reason}"),
        }
    }

    pub fn is_denied(&self) -> bool {
        matches!(self, Self::Denied)
    }
}

pub trait LocationProvider: Send {
    fn request(&mut self) -> Result<CachedSolarLocation, LocationError>;
    fn name(&self) -> &'static str;
}

/// GeoClue2 client over the system bus. Ember itself performs no network I/O;
/// the system location service may use network services depending on its
/// configuration, which is disclosed in the UI.
pub struct GeoClueProvider {
    timeout: Duration,
}

impl GeoClueProvider {
    pub fn new() -> Self {
        Self {
            timeout: Duration::from_secs(10),
        }
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub fn available() -> bool {
        let Ok(connection) = Connection::system() else {
            return false;
        };
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&connection) else {
            return false;
        };
        dbus.name_has_owner(
            "org.freedesktop.GeoClue2"
                .try_into()
                .unwrap_or_else(|_| "org.freedesktop.GeoClue2".try_into().unwrap()),
        )
        .unwrap_or(false)
    }

    fn request_inner(&self) -> Result<CachedSolarLocation, LocationError> {
        let connection =
            Connection::system().map_err(|error| LocationError::Unavailable(error.to_string()))?;
        let manager = Proxy::new(
            &connection,
            "org.freedesktop.GeoClue2",
            "/org/freedesktop/GeoClue2/Manager",
            "org.freedesktop.GeoClue2.Manager",
        )
        .map_err(|error| LocationError::Unavailable(error.to_string()))?;

        let session: OwnedObjectPath = manager
            .call::<_, _, OwnedObjectPath>("CreateSession", &())
            .map_err(map_zbus_error)?;
        let session_proxy = Proxy::new(
            &connection,
            "org.freedesktop.GeoClue2",
            session.as_str(),
            "org.freedesktop.GeoClue2.Session",
        )
        .map_err(map_zbus_error)?;

        // These properties are best-effort: older GeoClue versions may not have
        // all of them. Failure to set accuracy must not block a fix attempt.
        let _ = session_proxy.set_property("DesktopId", "project-ember");
        let _ = session_proxy.set_property("RequestedAccuracyLevel", 4u32);

        session_proxy
            .call::<_, _, ()>("Start", &())
            .map_err(map_zbus_error)?;

        let location_path = self.wait_for_location(&connection, &session_proxy)?;
        let _ = session_proxy.call::<_, _, ()>("Stop", &());
        drop(session_proxy);
        let _ = manager.call::<_, _, ()>("DeleteSession", &(session,));

        let location_proxy = Proxy::new(
            &connection,
            "org.freedesktop.GeoClue2",
            location_path.as_str(),
            "org.freedesktop.GeoClue2.Location",
        )
        .map_err(map_zbus_error)?;

        let latitude: f64 = location_proxy
            .get_property("Latitude")
            .map_err(map_zbus_error)?;
        let longitude: f64 = location_proxy
            .get_property("Longitude")
            .map_err(map_zbus_error)?;
        let timestamp: u64 = location_proxy.get_property("Timestamp").unwrap_or_default();

        let captured_at = normalize_timestamp(timestamp).unwrap_or_else(Utc::now);
        let age = Utc::now().signed_duration_since(captured_at);
        if age > chrono::Duration::from_std(MAX_FIX_AGE).unwrap_or_default() {
            return Err(LocationError::Transient(
                "the location fix is older than five minutes".into(),
            ));
        }

        let coordinate = SolarCoordinate::new(latitude, longitude).rounded(1);
        if !coordinate.is_valid() {
            return Err(LocationError::Transient("invalid coordinates".into()));
        }
        Ok(CachedSolarLocation::new(coordinate, captured_at))
    }

    fn wait_for_location(
        &self,
        connection: &Connection,
        session: &Proxy<'_>,
    ) -> Result<OwnedObjectPath, LocationError> {
        let deadline = Instant::now() + self.timeout;
        let mut last_error = None;
        while Instant::now() < deadline {
            let path: Result<OwnedObjectPath, _> = session.get_property("Location");
            match path {
                Ok(path) if !path.as_str().is_empty() && path.as_str() != "/" => return Ok(path),
                Ok(_) => {}
                Err(error) => last_error = Some(map_zbus_error(error)),
            }

            // Some GeoClue versions expose the fix only through the Client.
            if let Ok(client_path) = session.get_property::<OwnedObjectPath>("Client") {
                if !client_path.as_str().is_empty() {
                    if let Ok(client) = Proxy::new(
                        connection,
                        "org.freedesktop.GeoClue2",
                        client_path.as_str(),
                        "org.freedesktop.GeoClue2.Client",
                    ) {
                        if let Ok(path) = client.get_property::<OwnedObjectPath>("Location") {
                            if !path.as_str().is_empty() && path.as_str() != "/" {
                                return Ok(path);
                            }
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(match last_error {
            Some(error) => error,
            None => LocationError::Transient("no location fix arrived in time".into()),
        })
    }
}

impl Default for GeoClueProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LocationProvider for GeoClueProvider {
    fn request(&mut self) -> Result<CachedSolarLocation, LocationError> {
        let result = self.request_inner();
        if let Err(error) = &result {
            debug!("geoclue request failed: {error:?}");
        }
        result
    }

    fn name(&self) -> &'static str {
        "geoclue"
    }
}

/// GeoClue timestamps are `t` seconds since epoch in current versions; older
/// builds used microseconds. Normalize both, and treat zero as "now".
pub fn normalize_timestamp(raw: u64) -> Option<DateTime<Utc>> {
    if raw == 0 {
        return None;
    }
    let seconds = if raw > 10_000_000_000_000 {
        raw / 1_000_000
    } else if raw > 10_000_000_000 {
        raw / 1_000
    } else {
        raw
    };
    DateTime::from_timestamp(seconds as i64, 0)
}

fn map_zbus_error(error: zbus::Error) -> LocationError {
    let text = error.to_string();
    let lower = text.to_ascii_lowercase();
    if lower.contains("denied")
        || lower.contains("not authorized")
        || lower.contains("permission")
        || lower.contains("cancelled")
    {
        LocationError::Denied
    } else if lower.contains("serviceunknown")
        || lower.contains("name has no owner")
        || lower.contains("not provided")
        || lower.contains("unknown service")
    {
        LocationError::Unavailable(text)
    } else {
        LocationError::Transient(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_normalization_handles_seconds_and_microseconds() {
        let seconds = 1_794_168_000u64;
        assert_eq!(
            normalize_timestamp(seconds).unwrap().timestamp(),
            seconds as i64
        );
        assert_eq!(
            normalize_timestamp(seconds * 1_000_000)
                .unwrap()
                .timestamp(),
            seconds as i64
        );
        assert!(normalize_timestamp(0).is_none());
    }

    #[test]
    fn errors_are_classified_truthfully() {
        let denied = map_zbus_error(zbus::Error::Failure(
            "org.freedesktop.DBus.Error.AccessDenied: denied".into(),
        ));
        assert!(denied.is_denied());
        let unavailable = map_zbus_error(zbus::Error::Failure(
            "org.freedesktop.DBus.Error.ServiceUnknown: no owner".into(),
        ));
        assert!(matches!(unavailable, LocationError::Unavailable(_)));
        let transient = map_zbus_error(zbus::Error::Failure("timeout".into()));
        assert!(matches!(transient, LocationError::Transient(_)));
    }
}
