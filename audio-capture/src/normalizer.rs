//! Audio normalization: channel downmix + resample to 16kHz mono.
//!
//! Adapted from tambourine-voice's `AudioStreamNormalizer` with the target
//! sample rate changed from 48kHz to 16kHz (what the Gemini Live API expects).

use cpal::Sample;

const TARGET_OUTPUT_SAMPLE_RATE_HZ: u32 = 16_000;

/// Returns the fixed output sample rate (16kHz).
pub const fn output_sample_rate_hz() -> u32 {
    TARGET_OUTPUT_SAMPLE_RATE_HZ
}

/// Normalizes native microphone input into mono 16kHz float samples.
///
/// Handles:
/// - Channel downmixing (N channels -> mono)
/// - Sample-rate conversion (device rate -> 16kHz) using linear interpolation
pub struct AudioStreamNormalizer {
    input_channel_count: usize,
    input_sample_period_seconds: f64,
    output_sample_period_seconds: f64,
    current_input_time_seconds: f64,
    next_output_time_seconds: f64,
    previous_input_sample: Option<f32>,
}

impl AudioStreamNormalizer {
    pub fn new(input_channel_count: usize, input_sample_rate_hz: u32) -> Self {
        Self {
            input_channel_count,
            input_sample_period_seconds: 1.0 / f64::from(input_sample_rate_hz),
            output_sample_period_seconds: 1.0 / f64::from(TARGET_OUTPUT_SAMPLE_RATE_HZ),
            current_input_time_seconds: 0.0,
            next_output_time_seconds: 0.0,
            previous_input_sample: None,
        }
    }

    #[allow(clippy::cast_possible_truncation)]
    fn push_mono_sample(&mut self, mono_sample: f32, normalized_output: &mut Vec<f32>) {
        if self.previous_input_sample.is_none() {
            // Emit first sample immediately so startup has deterministic, non-silent output.
            normalized_output.push(mono_sample);
            self.previous_input_sample = Some(mono_sample);
            self.next_output_time_seconds =
                self.current_input_time_seconds + self.output_sample_period_seconds;
            self.current_input_time_seconds += self.input_sample_period_seconds;
            return;
        }

        let previous_input_sample = self.previous_input_sample.unwrap_or(mono_sample);
        let previous_input_time_seconds =
            self.current_input_time_seconds - self.input_sample_period_seconds;

        while self.next_output_time_seconds <= self.current_input_time_seconds {
            let interpolation_position = ((self.next_output_time_seconds
                - previous_input_time_seconds)
                / self.input_sample_period_seconds)
                .clamp(0.0, 1.0) as f32;

            let interpolated_sample = previous_input_sample
                + (mono_sample - previous_input_sample) * interpolation_position;
            normalized_output.push(interpolated_sample);
            self.next_output_time_seconds += self.output_sample_period_seconds;
        }

        self.previous_input_sample = Some(mono_sample);
        self.current_input_time_seconds += self.input_sample_period_seconds;
    }
}

/// Normalize an interleaved multi-channel input chunk to mono resampled output.
pub fn normalize_interleaved_input_chunk<T, F>(
    interleaved_input_samples: &[T],
    normalizer: &mut AudioStreamNormalizer,
    mut convert_sample_to_f32: F,
) -> Vec<f32>
where
    T: Copy,
    F: FnMut(T) -> f32,
{
    let mut normalized_output = Vec::new();
    let channel_count_as_f32 =
        f32::from(u16::try_from(normalizer.input_channel_count).unwrap_or(u16::MAX));

    for frame_samples in interleaved_input_samples.chunks_exact(normalizer.input_channel_count) {
        let mono_sample = frame_samples
            .iter()
            .copied()
            .map(&mut convert_sample_to_f32)
            .sum::<f32>()
            / channel_count_as_f32;

        normalizer.push_mono_sample(mono_sample, &mut normalized_output);
    }

    normalized_output
}

/// Convert normalized f32 sample ([-1.0, 1.0]) to i16 PCM.
pub fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// Generic sample-to-f32 converter using cpal's `FromSample` trait.
/// Handles all sample formats (i8, i16, i24, i32, i64, u8, u16, u24, u32, u64, f32, f64)
/// correctly, including edge cases like `i16::MIN` mapping to `-1.0`.
pub fn convert_sample_to_normalized_f32<T>(sample: T) -> f32
where
    f32: cpal::FromSample<T>,
{
    f32::from_sample(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_samples_are_close(actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "sample lengths differ: actual={}, expected={}",
            actual.len(),
            expected.len()
        );
        for (index, (actual_sample, expected_sample)) in
            actual.iter().zip(expected.iter()).enumerate()
        {
            let delta = (actual_sample - expected_sample).abs();
            assert!(
                delta <= tolerance,
                "sample mismatch at index {index}: actual={actual_sample}, expected={expected_sample}, delta={delta}"
            );
        }
    }

    #[test]
    fn passthrough_16khz_mono_keeps_data_stable() {
        let mut normalizer = AudioStreamNormalizer::new(1, 16_000);

        let first_chunk =
            normalize_interleaved_input_chunk(&[0.0_f32, 0.5_f32], &mut normalizer, |s| s);
        let second_chunk =
            normalize_interleaved_input_chunk(&[-0.25_f32, 1.0_f32], &mut normalizer, |s| s);

        let mut combined = first_chunk;
        combined.extend(second_chunk);

        assert_samples_are_close(&combined, &[0.0, 0.5, -0.25, 1.0], 1e-6);
    }

    #[test]
    fn downmixes_stereo_to_mono() {
        let mut normalizer = AudioStreamNormalizer::new(2, 16_000);
        let output = normalize_interleaved_input_chunk(
            &[1.0_f32, -1.0_f32, 0.25_f32, 0.75_f32],
            &mut normalizer,
            |s| s,
        );

        assert_samples_are_close(&output, &[0.0, 0.5], 1e-6);
    }

    #[test]
    fn downsamples_48khz_to_16khz() {
        let mut normalizer = AudioStreamNormalizer::new(1, 48_000);
        // 6 input samples at 48kHz → 3:1 ratio → 2 output samples at 16kHz
        // First sample emitted immediately (0.0), next at input index 3
        let output = normalize_interleaved_input_chunk(
            &[0.0_f32, 0.25_f32, 0.5_f32, 0.75_f32, 1.0_f32, 0.5_f32],
            &mut normalizer,
            |s| s,
        );
        assert_eq!(output.len(), 2);
        assert_samples_are_close(&output, &[0.0, 0.75], 1e-5);
    }

    #[test]
    fn f32_to_i16_clamps_edges() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(1.5), 32767); // clamps
        assert_eq!(f32_to_i16(-1.5), -32767); // clamps
    }

    #[test]
    fn generic_converter_handles_i16_edge_cases() {
        let converted = [
            convert_sample_to_normalized_f32(i16::MIN),
            convert_sample_to_normalized_f32(0_i16),
            convert_sample_to_normalized_f32(i16::MAX),
        ];
        assert_samples_are_close(&converted, &[-1.0, 0.0, 1.0], 1e-4);
    }

    #[test]
    fn generic_converter_handles_u16_edge_cases() {
        let converted = [
            convert_sample_to_normalized_f32(u16::MIN),
            convert_sample_to_normalized_f32(32_768_u16),
            convert_sample_to_normalized_f32(u16::MAX),
        ];
        assert_samples_are_close(&converted, &[-1.0, 0.0, 1.0], 1e-4);
    }

    #[test]
    fn generic_converter_handles_f32_passthrough() {
        let converted = [
            convert_sample_to_normalized_f32(-1.0_f32),
            convert_sample_to_normalized_f32(0.0_f32),
            convert_sample_to_normalized_f32(0.5_f32),
            convert_sample_to_normalized_f32(1.0_f32),
        ];
        assert_samples_are_close(&converted, &[-1.0, 0.0, 0.5, 1.0], 0.0);
    }
}
