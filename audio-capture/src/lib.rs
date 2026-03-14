#![deny(clippy::all)]

mod capture;
mod normalizer;

use capture::{AudioSampleCallback, CaptureEngine, DeviceSelection};
use napi::bindgen_prelude::Int16Array;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;

/// Exposed to JavaScript as a plain object.
/// `index` is `i32` because JavaScript has no unsigned integer type.
/// The value is always non-negative.
#[napi(object)]
pub struct AudioDevice {
    pub index: i32,
    pub name: String,
}

#[napi]
pub struct Recorder {
    engine: CaptureEngine,
    device_selection: DeviceSelection,
}

#[napi]
impl Recorder {
    #[napi(constructor)]
    pub fn new(device_index: Option<i32>) -> Self {
        let device_selection = match device_index {
            Some(i) if i >= 0 => DeviceSelection::ByIndex(i as u32),
            _ => DeviceSelection::Default,
        };
        Self {
            engine: CaptureEngine::new(),
            device_selection,
        }
    }

    /// Start capturing audio. The callback is invoked on a dedicated drain
    /// thread each time new samples are available from the microphone.
    #[napi(ts_args_type = "callback: (err: Error | null, samples: Int16Array) => void")]
    pub fn start(&self, callback: ThreadsafeFunction<Int16Array>) -> napi::Result<()> {
        let on_audio_samples: AudioSampleCallback = Box::new(move |samples| {
            // NonBlocking: queues the call onto the Node.js event loop and
            // returns immediately. If the TSFN has been released (e.g. during
            // shutdown), the error is silently ignored.
            callback.call(
                Ok(Int16Array::new(samples)),
                ThreadsafeFunctionCallMode::NonBlocking,
            );
        });

        self.engine
            .start(self.device_selection.clone(), on_audio_samples)
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    }

    #[napi]
    pub fn stop(&self) {
        self.engine.stop();
    }

    #[napi(getter)]
    pub fn sample_rate(&self) -> u32 {
        normalizer::output_sample_rate_hz()
    }

    #[napi]
    pub fn get_audio_devices() -> Vec<AudioDevice> {
        CaptureEngine::list_devices()
            .into_iter()
            .map(|d| AudioDevice {
                index: d.index,
                name: d.name,
            })
            .collect()
    }
}
