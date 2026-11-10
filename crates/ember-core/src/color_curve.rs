use crate::models::ColorGains;

/// Warmth → per-channel gain mapping.
///
/// This is a line-for-line port of the macOS implementation so both products
/// render identical colors: the first 82% of the slider follows a 6500 K →
/// 2000 K black-body approximation; the final 18% fades to red-channel-only
/// "Pure Red". Pure Red is a transfer-table mode, not a color temperature.
pub struct ColorCurve;

impl ColorCurve {
    pub const RED_TAIL_START: f64 = 0.82;

    /// Maps a user-facing `0.0..=1.0` warmth to neutral-to-red gains.
    pub fn gains_for_warmth(warmth: f64) -> ColorGains {
        let amount = warmth.clamp(0.0, 1.0);
        if amount == 0.0 {
            return ColorGains::neutral();
        }
        if amount <= Self::RED_TAIL_START {
            let progress = smoothstep(amount / Self::RED_TAIL_START);
            let kelvin = 6500.0 - (4500.0 * progress);
            return Self::gains_for_kelvin(kelvin);
        }
        let base = Self::gains_for_kelvin(2000.0);
        let progress = smoothstep((amount - Self::RED_TAIL_START) / (1.0 - Self::RED_TAIL_START));
        ColorGains::new(
            lerp(base.red as f64, 1.0, progress) as f32,
            lerp(base.green as f64, 0.0, progress) as f32,
            lerp(base.blue as f64, 0.0, progress) as f32,
        )
    }

    /// Kelvin value shown in the UI, or `None` for the Pure Red tail.
    pub fn approximate_kelvin(warmth: f64) -> Option<f64> {
        let amount = warmth.clamp(0.0, 1.0);
        if amount > Self::RED_TAIL_START {
            return None;
        }
        Some(6500.0 - (4500.0 * smoothstep(amount / Self::RED_TAIL_START)))
    }

    /// CIE 1931 xy approximation of a black-body / daylight locus, converted to
    /// linear sRGB gains and normalized so the brightest channel is 1.0.
    pub fn gains_for_kelvin(kelvin: f64) -> ColorGains {
        let temperature = kelvin.clamp(1667.0, 25000.0);
        if (temperature - 6500.0).abs() < 0.5 {
            return ColorGains::neutral();
        }

        let t = temperature;
        let x = if t <= 4000.0 {
            (-0.2661239e9 / t.powi(3)) - (0.2343580e6 / t.powi(2)) + (0.8776956e3 / t) + 0.179910
        } else {
            (-3.0258469e9 / t.powi(3)) + (2.1070379e6 / t.powi(2)) + (0.2226347e3 / t) + 0.240390
        };

        let y = if t <= 2222.0 {
            (-1.1063814 * x.powi(3)) - (1.34811020 * x.powi(2)) + (2.18555832 * x) - 0.20219683
        } else if t <= 4000.0 {
            (-0.9549476 * x.powi(3)) - (1.37418593 * x.powi(2)) + (2.09137015 * x) - 0.16748867
        } else {
            (3.0817580 * x.powi(3)) - (5.87338670 * x.powi(2)) + (3.75112997 * x) - 0.37001483
        };

        let luminance = 1.0;
        let capital_x = x / y;
        let capital_z = (1.0 - x - y) / y;

        let red = (3.2404542 * capital_x) - (1.5371385 * luminance) - (0.4985314 * capital_z);
        let green = (-0.9692660 * capital_x) + (1.8760108 * luminance) + (0.0415560 * capital_z);
        let blue = (0.0556434 * capital_x) - (0.2040259 * luminance) + (1.0572252 * capital_z);
        let maximum = red.max(green).max(blue).max(0.000_001);

        ColorGains::new(
            (red / maximum).max(0.0) as f32,
            (green / maximum).max(0.0) as f32,
            (blue / maximum).max(0.0) as f32,
        )
    }

    /// Human label for the warmth slider value, matching the macOS wording.
    pub fn warmth_description(warmth: f64) -> String {
        if warmth < 0.01 {
            return "Neutral".to_string();
        }
        match Self::approximate_kelvin(warmth) {
            Some(kelvin) => format!("~{} K", (kelvin / 50.0).round() as i64 * 50),
            None => "Pure Red".to_string(),
        }
    }
}

fn smoothstep(value: f64) -> f64 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - (2.0 * t))
}

fn lerp(start: f64, end: f64, progress: f64) -> f64 {
    start + ((end - start) * progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_is_exact() {
        assert_eq!(ColorCurve::gains_for_warmth(0.0), ColorGains::neutral());
        assert_eq!(ColorCurve::gains_for_kelvin(6500.0), ColorGains::neutral());
    }

    #[test]
    fn pure_red_endpoint_removes_green_and_blue() {
        let gains = ColorCurve::gains_for_warmth(1.0);
        assert_eq!(gains.red, 1.0);
        assert_eq!(gains.green, 0.0);
        assert_eq!(gains.blue, 0.0);
        assert!(ColorCurve::approximate_kelvin(1.0).is_none());
    }

    #[test]
    fn clamps_out_of_range() {
        assert_eq!(ColorCurve::gains_for_warmth(-5.0), ColorGains::neutral());
        assert_eq!(
            ColorCurve::gains_for_warmth(5.0),
            ColorGains::new(1.0, 0.0, 0.0)
        );
    }

    #[test]
    fn warmth_removes_blue_and_green_monotonically() {
        let samples: Vec<ColorGains> = (0..=20)
            .map(|i| ColorCurve::gains_for_warmth(i as f64 / 20.0))
            .collect();
        for window in samples.windows(2) {
            assert!(window[1].blue <= window[0].blue + 0.0001);
            assert!(window[1].green <= window[0].green + 0.0001);
            assert!(window[1].red >= window[0].red - 0.0001);
        }
    }

    #[test]
    fn warm_labels_match_macos() {
        assert_eq!(ColorCurve::warmth_description(0.0), "Neutral");
        assert_eq!(ColorCurve::warmth_description(1.0), "Pure Red");
        assert_eq!(ColorCurve::warmth_description(0.62), "~2650 K");
    }

    #[test]
    fn kelvin_gains_are_normalized() {
        for kelvin in [2000.0, 2700.0, 3400.0, 4500.0, 5500.0, 6500.0] {
            let g = ColorCurve::gains_for_kelvin(kelvin);
            let max = g.red.max(g.green).max(g.blue);
            assert!((max - 1.0).abs() < 0.001, "{kelvin} K max {max}");
            assert!(g.red >= g.green - 0.001, "warmth should keep red >= green");
            assert!(
                g.green >= g.blue - 0.001,
                "warmth should keep green >= blue"
            );
        }
    }
}
