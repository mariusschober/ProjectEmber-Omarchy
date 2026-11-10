use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Stable per-display identity.
///
/// Linux does not expose a CoreGraphics UUID. We derive `uuid` from the
/// connector's EDID (manufacturer/product/serial or a hash of the EDID bytes)
/// and keep the connector name as a fallback. `vendor_number`/`model_number`/
/// `serial_number` mirror the macOS model so journals remain structurally
/// comparable, while `legacy_display_id` supports v1 macOS journals.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayIdentity {
    pub uuid: Option<String>,
    pub vendor_number: u32,
    pub model_number: u32,
    pub serial_number: u32,
    pub unit_number: u32,
    pub is_built_in: bool,
    #[serde(rename = "legacyDisplayID")]
    pub legacy_display_id: Option<u32>,
    /// Connector name such as `eDP-1` or `DP-3`. Not part of identity matching
    /// when EDID identity is available; exposed for diagnostics and display
    /// mapping.
    #[serde(default)]
    pub connector: String,
}

impl DisplayIdentity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        uuid: Option<String>,
        vendor_number: u32,
        model_number: u32,
        serial_number: u32,
        unit_number: u32,
        is_built_in: bool,
        legacy_display_id: Option<u32>,
        connector: String,
    ) -> Self {
        Self {
            uuid: uuid
                .map(|value| value.trim().to_ascii_lowercase())
                .filter(|value| !value.is_empty()),
            vendor_number,
            model_number,
            serial_number,
            unit_number,
            is_built_in,
            legacy_display_id,
            connector,
        }
    }

    /// v1 journal entry: display id only, assumed built-in like the macOS app.
    pub fn legacy(display_id: u32) -> Self {
        Self::new(None, 0, 0, 0, 0, true, Some(display_id), String::new())
    }

    /// Matching follows the macOS rules: UUID if both sides have one, then
    /// nonzero serial equality, then vendor/model/unit equality. A zero-serial
    /// fallback can collide, which callers must treat as ambiguous.
    pub fn matches(&self, other: &DisplayIdentity) -> bool {
        if let (Some(uuid), Some(other_uuid)) = (&self.uuid, &other.uuid) {
            if uuid.eq_ignore_ascii_case(other_uuid) {
                return true;
            }
            if self.serial_number != 0 && other.serial_number != 0 {
                return self.vendor_number == other.vendor_number
                    && self.model_number == other.model_number
                    && self.serial_number == other.serial_number
                    && self.is_built_in == other.is_built_in;
            }
            return false;
        }
        if self.serial_number != 0 && other.serial_number != 0 {
            return self.vendor_number == other.vendor_number
                && self.model_number == other.model_number
                && self.serial_number == other.serial_number
                && self.is_built_in == other.is_built_in;
        }
        self.vendor_number == other.vendor_number
            && self.model_number == other.model_number
            && self.unit_number == other.unit_number
            && self.is_built_in == other.is_built_in
    }

    /// True when this identity cannot uniquely distinguish a physical display.
    /// Callers must not mutate on ambiguity.
    pub fn is_ambiguous_without_uuid(&self) -> bool {
        if self.uuid.as_deref().is_some_and(|uuid| !uuid.is_empty()) {
            return false;
        }
        self.serial_number == 0
    }

    /// Stable key used for reconciliation sets and journal lookups.
    pub fn key(&self) -> String {
        if let Some(uuid) = &self.uuid {
            if !uuid.is_empty() {
                return format!("uuid:{}", uuid.to_ascii_lowercase());
            }
        }
        format!(
            "hw:{}:{}:{}:{}:{}",
            self.vendor_number,
            self.model_number,
            self.serial_number,
            self.unit_number,
            self.is_built_in
        )
    }

    /// Sanitized identifier for logs/exports: raw serials and EDID bytes never
    /// appear. The salt is a per-install random value kept at 0600.
    pub fn diagnostic_tag(&self, salt: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(salt.as_bytes());
        hasher.update(self.key().as_bytes());
        hasher.update(self.connector.as_bytes());
        let digest = hasher.finalize();
        digest[..6].iter().map(|b| format!("{b:02x}")).collect()
    }

    pub fn label(&self) -> String {
        if self.connector.is_empty() {
            format!(
                "{}:{:04x}:{:04x}",
                if self.is_built_in {
                    "built-in"
                } else {
                    "display"
                },
                self.vendor_number,
                self.model_number
            )
        } else {
            format!(
                "{} ({})",
                self.connector,
                if self.is_built_in {
                    "built-in"
                } else {
                    "external"
                }
            )
        }
    }

    pub fn is_built_in_connector(name: &str) -> bool {
        let upper = name.to_ascii_uppercase();
        ["EDP", "LVDS", "DSI", "DPI", "DSI-", "E-DP"]
            .iter()
            .any(|prefix| upper.starts_with(prefix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(uuid: Option<&str>, serial: u32, built_in: bool) -> DisplayIdentity {
        DisplayIdentity::new(
            uuid.map(str::to_string),
            100,
            200,
            serial,
            1,
            built_in,
            None,
            "DP-1".into(),
        )
    }

    #[test]
    fn uuid_matches_case_insensitively() {
        assert!(id(Some("ABC-DEF"), 0, false).matches(&id(Some("abc-def"), 0, false)));
    }

    #[test]
    fn uuid_mismatch_with_serial_falls_back_to_serial() {
        assert!(id(Some("A"), 10, false).matches(&id(Some("B"), 10, false)));
        assert!(!id(Some("A"), 10, false).matches(&id(Some("B"), 11, false)));
    }

    #[test]
    fn zero_serial_uses_connector_unit_fallback() {
        assert!(id(None, 0, false).matches(&id(None, 0, false)));
    }

    #[test]
    fn built_in_flag_matters() {
        assert!(!id(None, 10, true).matches(&id(None, 10, false)));
    }

    #[test]
    fn ambiguity_detection() {
        assert!(id(None, 0, false).is_ambiguous_without_uuid());
        assert!(!id(Some("A"), 0, false).is_ambiguous_without_uuid());
        assert!(!id(None, 9, false).is_ambiguous_without_uuid());
    }

    #[test]
    fn keys_prefer_uuid() {
        assert_eq!(id(Some("ABC"), 5, false).key(), "uuid:abc");
        assert_eq!(id(None, 5, false).key(), "hw:100:200:5:1:false");
    }

    #[test]
    fn diagnostic_tags_are_stable_and_salted() {
        let a = id(Some("ABC"), 5, false);
        assert_eq!(a.diagnostic_tag("s1"), a.diagnostic_tag("s1"));
        assert_ne!(a.diagnostic_tag("s1"), a.diagnostic_tag("s2"));
        assert_eq!(a.diagnostic_tag("s1").len(), 12);
        assert!(!a.diagnostic_tag("s1").contains("abc"));
    }

    #[test]
    fn built_in_connector_detection() {
        assert!(DisplayIdentity::is_built_in_connector("eDP-1"));
        assert!(DisplayIdentity::is_built_in_connector("LVDS-1"));
        assert!(DisplayIdentity::is_built_in_connector("DSI-1"));
        assert!(!DisplayIdentity::is_built_in_connector("DP-1"));
        assert!(!DisplayIdentity::is_built_in_connector("HDMI-A-1"));
    }

    #[test]
    fn v1_legacy_is_built_in() {
        let legacy = DisplayIdentity::legacy(42);
        assert!(legacy.is_built_in);
        assert_eq!(legacy.legacy_display_id, Some(42));
    }
}
