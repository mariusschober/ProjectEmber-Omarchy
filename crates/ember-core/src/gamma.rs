use serde::{Deserialize, Serialize};

use crate::models::ColorGains;

/// Maximum accepted ramp length. Wayland compositors commonly expose 256–4096
/// samples; the protocol allows arbitrary sizes, so cap parsing to avoid memory
/// abuse from a corrupted journal.
pub const MAX_SAMPLES: usize = 65_536;
/// Minimum meaningful ramp length.
pub const MIN_SAMPLES: usize = 2;

/// A per-channel transfer table with values in the display's encoded space.
///
/// Values are `0.0..=1.0` floats internally; platform backends convert to the
/// native representation (u16 ramps on Wayland/X11). Original tables are kept
/// immutable and every transform derives from them, so slider changes never
/// compound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GammaTable {
    pub red: Vec<f32>,
    pub green: Vec<f32>,
    pub blue: Vec<f32>,
}

impl GammaTable {
    pub fn new(red: Vec<f32>, green: Vec<f32>, blue: Vec<f32>) -> Self {
        Self { red, green, blue }
    }

    pub fn sample_count(&self) -> usize {
        self.red.len()
    }

    pub fn is_valid(&self) -> bool {
        let count = self.red.len();
        (MIN_SAMPLES..=MAX_SAMPLES).contains(&count)
            && self.green.len() == count
            && self.blue.len() == count
            && self.red.iter().all(|v| v.is_finite())
            && self.green.iter().all(|v| v.is_finite())
            && self.blue.iter().all(|v| v.is_finite())
    }

    /// Linear identity ramp of `sample_count` entries (0, 1/(n-1), …, 1).
    pub fn identity(sample_count: usize) -> Self {
        let count = sample_count.clamp(MIN_SAMPLES, MAX_SAMPLES);
        let values: Vec<f32> = (0..count).map(|i| i as f32 / (count - 1) as f32).collect();
        Self {
            red: values.clone(),
            green: values.clone(),
            blue: values,
        }
    }

    /// Applies per-channel gains and software brightness. Output is clamped to
    /// `0.0..=1.0`; brightness is clamped to the same 10% floor as the UI.
    pub fn applying(&self, gains: ColorGains, apparent_brightness: f64) -> Self {
        if !self.is_valid() {
            return self.clone();
        }
        let brightness = apparent_brightness.clamp(0.10, 1.0) as f32;
        let map = |values: &[f32], gain: f32| {
            values
                .iter()
                .map(|v| (v * gain * brightness).clamp(0.0, 1.0))
                .collect::<Vec<f32>>()
        };
        Self {
            red: map(&self.red, gains.red),
            green: map(&self.green, gains.green),
            blue: map(&self.blue, gains.blue),
        }
    }

    /// Linearly resamples each ramp to `target_count`.
    pub fn resampled(&self, target_count: usize) -> Self {
        let count = target_count.clamp(MIN_SAMPLES, MAX_SAMPLES);
        if !self.is_valid() || count == self.sample_count() {
            return self.clone();
        }
        Self {
            red: resample(&self.red, count),
            green: resample(&self.green, count),
            blue: resample(&self.blue, count),
        }
    }

    /// Largest absolute per-channel difference, resampling the other table to
    /// this table's length. Returns infinity when either table is invalid.
    pub fn maximum_absolute_difference(&self, other: &GammaTable) -> f32 {
        if !self.is_valid() || !other.is_valid() {
            return f32::INFINITY;
        }
        let rhs = other.resampled(self.sample_count());
        let mut difference: f32 = 0.0;
        for index in 0..self.sample_count() {
            difference = difference.max((self.red[index] - rhs.red[index]).abs());
            difference = difference.max((self.green[index] - rhs.green[index]).abs());
            difference = difference.max((self.blue[index] - rhs.blue[index]).abs());
        }
        difference
    }

    /// Converts to interleaved u16 ramps as required by Wayland's
    /// `zwlr_gamma_control_v1.set_gamma` (red block, green block, blue block).
    pub fn to_u16_blocks(&self, gamma_size: usize) -> Vec<u16> {
        let size = gamma_size.clamp(MIN_SAMPLES, MAX_SAMPLES);
        let table = self.resampled(size);
        let mut out = Vec::with_capacity(size * 3);
        for values in [&table.red, &table.green, &table.blue] {
            out.extend(
                values
                    .iter()
                    .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16),
            );
        }
        out
    }

    /// Converts to the separate channel arrays used by XRandR.
    pub fn to_u16_channels(&self, gamma_size: usize) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
        let size = gamma_size.clamp(MIN_SAMPLES, MAX_SAMPLES);
        let table = self.resampled(size);
        let convert = |values: &[f32]| {
            values
                .iter()
                .map(|v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
                .collect::<Vec<u16>>()
        };
        (
            convert(&table.red),
            convert(&table.green),
            convert(&table.blue),
        )
    }

    /// Builds a table from interleaved u16 blocks (wayland layout).
    pub fn from_u16_blocks(blocks: &[u16], gamma_size: usize) -> Option<Self> {
        let size = gamma_size;
        if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&size) || blocks.len() != size * 3 {
            return None;
        }
        let convert = |slice: &[u16]| {
            slice
                .iter()
                .map(|v| *v as f32 / 65535.0)
                .collect::<Vec<f32>>()
        };
        Some(Self {
            red: convert(&blocks[0..size]),
            green: convert(&blocks[size..size * 2]),
            blue: convert(&blocks[size * 2..size * 3]),
        })
    }

    /// Builds a table from separate u16 channel arrays (XRandR layout).
    pub fn from_u16_channels(red: &[u16], green: &[u16], blue: &[u16]) -> Option<Self> {
        let size = red.len();
        if !(MIN_SAMPLES..=MAX_SAMPLES).contains(&size) || green.len() != size || blue.len() != size
        {
            return None;
        }
        let convert = |slice: &[u16]| {
            slice
                .iter()
                .map(|v| *v as f32 / 65535.0)
                .collect::<Vec<f32>>()
        };
        Some(Self {
            red: convert(red),
            green: convert(green),
            blue: convert(blue),
        })
    }

    pub fn all_finite(&self) -> bool {
        self.red
            .iter()
            .chain(self.green.iter())
            .chain(self.blue.iter())
            .all(|v| v.is_finite())
    }
}

fn resample(values: &[f32], count: usize) -> Vec<f32> {
    if values.len() < 2 {
        return vec![values.first().copied().unwrap_or(0.0); count];
    }
    (0..count)
        .map(|index| {
            let position = index as f64 * (values.len() - 1) as f64 / (count - 1) as f64;
            let lower = position.floor() as usize;
            let upper = (lower + 1).min(values.len() - 1);
            let fraction = (position - lower as f64) as f32;
            values[lower] + ((values[upper] - values[lower]) * fraction)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_has_exact_endpoints() {
        for count in [2, 3, 8, 256, 4096] {
            let table = GammaTable::identity(count);
            assert!(table.is_valid());
            assert_eq!(table.sample_count(), count);
            assert_eq!(table.red[0], 0.0);
            assert_eq!(table.red[count - 1], 1.0);
        }
    }

    #[test]
    fn neutral_full_brightness_preserves_baseline() {
        let baseline = GammaTable::identity(256);
        let transformed = baseline.applying(ColorGains::neutral(), 1.0);
        assert_eq!(transformed, baseline);
    }

    #[test]
    fn gains_and_brightness_compose() {
        let tiny = GammaTable::identity(3);
        let transformed = tiny.applying(ColorGains::new(1.0, 0.5, 0.25), 0.5);
        assert_eq!(transformed.red, vec![0.0, 0.25, 0.5]);
        assert_eq!(transformed.green, vec![0.0, 0.125, 0.25]);
        assert_eq!(transformed.blue, vec![0.0, 0.0625, 0.125]);
    }

    #[test]
    fn brightness_floor_is_ten_percent() {
        let tiny = GammaTable::identity(2);
        let transformed = tiny.applying(ColorGains::neutral(), 0.0);
        assert_eq!(transformed.red, vec![0.0, 0.10]);
    }

    #[test]
    fn resampling_preserves_endpoints() {
        let resampled = GammaTable::identity(4).resampled(257);
        assert_eq!(resampled.sample_count(), 257);
        assert_eq!(resampled.red[0], 0.0);
        assert_eq!(resampled.red[256], 1.0);
    }

    #[test]
    fn difference_detects_changes() {
        let base = GammaTable::identity(8);
        let warm = base.applying(ColorGains::new(1.0, 0.5, 0.2), 0.75);
        assert!(base.maximum_absolute_difference(&warm) > 0.004);
        assert!(warm.maximum_absolute_difference(&warm) < 0.0001);
    }

    #[test]
    fn u16_round_trip_is_lossless_enough() {
        let table = GammaTable::identity(256);
        let blocks = table.to_u16_blocks(4096);
        let back = GammaTable::from_u16_blocks(&blocks, 4096).unwrap();
        assert!(back.sample_count() == 4096);
        // Resample back to 256 and compare: quantization plus interpolation.
        let delta = table.maximum_absolute_difference(&back);
        assert!(delta < 0.0002, "delta={delta}");
    }

    #[test]
    fn invalid_tables_are_rejected() {
        assert!(!GammaTable::new(vec![0.0], vec![0.0], vec![0.0]).is_valid());
        assert!(!GammaTable::new(vec![0.0, f32::NAN], vec![0.0, 1.0], vec![0.0, 1.0]).is_valid());
        assert!(!GammaTable::new(vec![0.0, 1.0], vec![0.0], vec![0.0, 1.0]).is_valid());
    }

    #[test]
    fn repeated_apply_from_baseline_has_zero_drift() {
        let baseline = GammaTable::identity(256);
        let gains = crate::ColorCurve::gains_for_warmth(0.73);
        let mut last = baseline.clone();
        for _ in 0..100 {
            last = baseline.applying(gains, 0.42);
        }
        assert_eq!(
            last.maximum_absolute_difference(&baseline.clone().applying(gains, 0.42)),
            0.0
        );
    }
}
