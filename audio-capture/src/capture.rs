//! cpal-based microphone capture with a lock-free ring buffer and push-based
//! sample delivery.
//!
//! Architecture (three threads):
//!   1. **Audio thread** — owns the cpal stream lifecycle (Start/Stop/Shutdown).
//!   2. **cpal callback** (OS real-time thread) — normalizes samples and pushes
//!      i16 into the lock-free SPSC ring buffer. Never blocks.
//!   3. **Drain thread** — waits on a condvar, reads all available samples from
//!      the ring buffer, and invokes the caller-supplied callback. This is the
//!      only thread that touches the consumer side of the ring buffer.
//!
//! The ring buffer decouples the real-time cpal callback from the drain thread,
//! so the callback never contends with the JS-bound callback invocation.

use cpal::SampleFormat;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use crate::normalizer::{
    AudioStreamNormalizer, convert_sample_to_normalized_f32, f32_to_i16,
    normalize_interleaved_input_chunk,
};

/// Maximum ring buffer capacity in samples (~2 seconds at 16kHz).
const RING_BUFFER_CAPACITY: usize = 32_000;

/// Callback invoked from the drain thread with a batch of i16 PCM samples.
/// Kept generic (no napi dependency) so capture.rs stays pure Rust.
pub type AudioSampleCallback = Box<dyn Fn(Vec<i16>) + Send + 'static>;

/// Which audio input device to capture from.
#[derive(Clone)]
pub enum DeviceSelection {
    Default,
    ByIndex(u32),
}

/// Errors that can occur during audio capture operations.
#[derive(Debug, Clone)]
pub enum CaptureError {
    DeviceNotFound(String),
    StreamCreationFailed(String),
    StreamStartFailed(String),
    AudioThreadUnreachable(String),
}

impl fmt::Display for CaptureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DeviceNotFound(msg) => write!(f, "Audio device not found: {msg}"),
            Self::StreamCreationFailed(msg) => write!(f, "Failed to create audio stream: {msg}"),
            Self::StreamStartFailed(msg) => write!(f, "Failed to start audio stream: {msg}"),
            Self::AudioThreadUnreachable(msg) => write!(f, "Audio thread unreachable: {msg}"),
        }
    }
}

/// Commands sent to the audio capture thread.
enum AudioCommand {
    Start(DeviceSelection),
    Stop,
    Shutdown,
}

/// Response from the audio capture thread.
enum AudioResponse {
    Started,
    Error(CaptureError),
}

/// Information about an audio input device.
///
/// `index` is `i32` rather than `u32` because napi exposes it to JavaScript,
/// which has no unsigned integer type. The value is always non-negative.
#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub index: i32,
    pub name: String,
}

/// Notification mechanism: the producer (cpal callback) notifies the drain
/// thread that new samples are available, without the callback ever blocking.
struct SampleNotifier {
    condvar: Condvar,
    mutex: Mutex<()>,
}

impl SampleNotifier {
    fn new() -> Self {
        Self {
            condvar: Condvar::new(),
            mutex: Mutex::new(()),
        }
    }

    /// Called by the cpal callback after pushing samples. Never blocks.
    fn notify(&self) {
        self.condvar.notify_all();
    }

    /// Called by the drain thread to wait for new samples.
    /// Spurious wakeups are fine — the caller rechecks the ring buffer.
    fn wait(&self) {
        let guard = self.mutex.lock().unwrap();
        let _guard = self.condvar.wait(guard).unwrap();
    }
}

/// Handle to the drain thread, used to stop it on `stop()` or `drop()`.
struct DrainThreadHandle {
    stop_signal: Arc<AtomicBool>,
    thread_handle: Option<thread::JoinHandle<()>>,
}

/// Audio capture engine managing the cpal audio thread, ring buffer, and
/// drain thread.
pub struct CaptureEngine {
    command_tx: Sender<AudioCommand>,
    response_rx: Mutex<Receiver<AudioResponse>>,
    audio_thread_handle: Mutex<Option<thread::JoinHandle<()>>>,
    consumer: Arc<Mutex<HeapCons<i16>>>,
    sample_notifier: Arc<SampleNotifier>,
    drain_thread: Mutex<Option<DrainThreadHandle>>,
}

impl CaptureEngine {
    pub fn new() -> Self {
        let ring_buffer = HeapRb::<i16>::new(RING_BUFFER_CAPACITY);
        let (producer, consumer) = ring_buffer.split();

        let (command_tx, command_rx) = channel::<AudioCommand>();
        let (response_tx, response_rx) = channel::<AudioResponse>();

        let sample_notifier = Arc::new(SampleNotifier::new());
        let notifier_for_audio_thread = Arc::clone(&sample_notifier);

        let audio_thread_handle = thread::spawn(move || {
            run_audio_thread(command_rx, response_tx, producer, notifier_for_audio_thread);
        });

        Self {
            command_tx,
            response_rx: Mutex::new(response_rx),
            audio_thread_handle: Mutex::new(Some(audio_thread_handle)),
            consumer: Arc::new(Mutex::new(consumer)),
            sample_notifier,
            drain_thread: Mutex::new(None),
        }
    }

    /// Start capturing audio and push samples to `on_audio_samples` via the
    /// drain thread. The callback is invoked on a dedicated thread (not the
    /// cpal real-time thread) with whatever samples are available in the ring
    /// buffer each time the producer signals.
    pub fn start(
        &self,
        device_selection: DeviceSelection,
        on_audio_samples: AudioSampleCallback,
    ) -> Result<(), CaptureError> {
        // Stop any existing drain thread before starting a new one.
        self.stop_drain_thread();

        // Clear any stale samples from the ring buffer.
        {
            let mut consumer = self.consumer.lock().unwrap();
            let stale_count = consumer.occupied_len();
            consumer.skip(stale_count);
        }

        // Tell the audio thread to open the cpal stream.
        self.command_tx
            .send(AudioCommand::Start(device_selection))
            .map_err(|e| CaptureError::AudioThreadUnreachable(e.to_string()))?;

        let rx = self.response_rx.lock().unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(AudioResponse::Started) => {}
            Ok(AudioResponse::Error(e)) => return Err(e),
            Err(_) => {
                return Err(CaptureError::AudioThreadUnreachable(
                    "Timeout waiting for audio thread to start".into(),
                ));
            }
        }

        // Spawn the drain thread.
        let stop_signal = Arc::new(AtomicBool::new(false));
        let drain_stop = Arc::clone(&stop_signal);
        let drain_consumer = Arc::clone(&self.consumer);
        let drain_notifier = Arc::clone(&self.sample_notifier);

        let drain_handle = thread::spawn(move || {
            run_drain_thread(drain_consumer, drain_notifier, drain_stop, on_audio_samples);
        });

        *self.drain_thread.lock().unwrap() = Some(DrainThreadHandle {
            stop_signal,
            thread_handle: Some(drain_handle),
        });

        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.command_tx.send(AudioCommand::Stop);
        self.stop_drain_thread();

        // Clear buffer on stop.
        let mut consumer = self.consumer.lock().unwrap();
        let stale_count = consumer.occupied_len();
        consumer.skip(stale_count);
    }

    pub fn list_devices() -> Vec<AudioDeviceInfo> {
        let host = cpal::default_host();
        host.input_devices()
            .map(|devices| {
                devices
                    .enumerate()
                    .filter_map(|(index, device)| {
                        let name = device.description().ok()?.name().to_string();
                        Some(AudioDeviceInfo {
                            index: index as i32,
                            name,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Signal the drain thread to stop and wait for it to exit.
    fn stop_drain_thread(&self) {
        if let Some(mut drain) = self.drain_thread.lock().unwrap().take() {
            drain.stop_signal.store(true, Ordering::Relaxed);
            // Wake the drain thread in case it's blocked on the condvar.
            self.sample_notifier.notify();
            if let Some(handle) = drain.thread_handle.take() {
                let _ = handle.join();
            }
        }
    }
}

impl Drop for CaptureEngine {
    fn drop(&mut self) {
        // Stop drain thread first (it reads from the ring buffer).
        self.stop_drain_thread();

        // Then shut down the audio thread (it writes to the ring buffer).
        let _ = self.command_tx.send(AudioCommand::Shutdown);
        if let Some(handle) = self.audio_thread_handle.lock().unwrap().take() {
            let _ = handle.join();
        }
    }
}

/// Drain thread: reads all available samples from the ring buffer whenever
/// the cpal callback signals, and delivers them to the JS callback.
fn run_drain_thread(
    consumer: Arc<Mutex<HeapCons<i16>>>,
    sample_notifier: Arc<SampleNotifier>,
    stop_signal: Arc<AtomicBool>,
    on_audio_samples: AudioSampleCallback,
) {
    loop {
        sample_notifier.wait();

        if stop_signal.load(Ordering::Relaxed) {
            break;
        }

        let samples = {
            let mut consumer = consumer.lock().unwrap();
            let available = consumer.occupied_len();
            if available == 0 {
                continue;
            }
            consumer.pop_iter().take(available).collect::<Vec<i16>>()
        };

        if !samples.is_empty() {
            on_audio_samples(samples);
        }
    }
}

/// Main loop for the dedicated audio thread.
fn run_audio_thread(
    command_rx: Receiver<AudioCommand>,
    response_tx: Sender<AudioResponse>,
    producer: HeapProd<i16>,
    sample_notifier: Arc<SampleNotifier>,
) {
    let mut current_stream: Option<cpal::Stream> = None;

    // Wrap producer in Arc<Mutex> so it can be shared with cpal callbacks.
    // The mutex is only contended between successive cpal callbacks (same thread)
    // and `start()`/`stop()` stream teardown — never with the consumer.
    let producer = Arc::new(Mutex::new(producer));

    loop {
        match command_rx.recv() {
            Ok(AudioCommand::Start(device_selection)) => {
                // Stop any existing stream.
                current_stream.take();

                match create_stream(
                    &device_selection,
                    Arc::clone(&producer),
                    Arc::clone(&sample_notifier),
                ) {
                    Ok(stream) => {
                        current_stream = Some(stream);
                        let _ = response_tx.send(AudioResponse::Started);
                    }
                    Err(e) => {
                        let _ = response_tx.send(AudioResponse::Error(e));
                    }
                }
            }
            Ok(AudioCommand::Stop) => {
                current_stream.take();
            }
            Ok(AudioCommand::Shutdown) | Err(_) => {
                current_stream.take();
                break;
            }
        }
    }
}

/// Build a normalized input stream for a specific sample type, pushing i16
/// samples into the lock-free ring buffer.
fn build_normalized_input_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    input_channel_count: usize,
    input_sample_rate_hz: u32,
    producer: &Arc<Mutex<HeapProd<i16>>>,
    sample_notifier: &Arc<SampleNotifier>,
) -> Result<cpal::Stream, CaptureError>
where
    T: cpal::SizedSample + Copy + Send + 'static,
    f32: cpal::FromSample<T>,
{
    let producer_for_callback = Arc::clone(producer);
    let notifier_for_callback = Arc::clone(sample_notifier);
    let mut audio_stream_normalizer =
        AudioStreamNormalizer::new(input_channel_count, input_sample_rate_hz);

    device
        .build_input_stream(
            config,
            move |data: &[T], _: &cpal::InputCallbackInfo| {
                let normalized_f32 = normalize_interleaved_input_chunk(
                    data,
                    &mut audio_stream_normalizer,
                    convert_sample_to_normalized_f32::<T>,
                );

                if normalized_f32.is_empty() {
                    return;
                }

                let i16_samples: Vec<i16> =
                    normalized_f32.iter().copied().map(f32_to_i16).collect();

                // push_slice writes as many samples as fit; excess newest samples are dropped.
                let mut producer = producer_for_callback.lock().unwrap();
                producer.push_slice(&i16_samples);

                notifier_for_callback.notify();
            },
            |err| eprintln!("Audio stream error: {err}"),
            None,
        )
        .map_err(|e| CaptureError::StreamCreationFailed(e.to_string()))
}

/// Dispatch macro: calls `build_normalized_input_stream::<$T>` with shared args.
macro_rules! dispatch_sample_format {
    ($sample_format:expr, $device:expr, $config:expr, $channels:expr, $rate:expr, $producer:expr, $notifier:expr, $($format:pat => $type:ty),+ $(,)?) => {
        match $sample_format {
            $(
                $format => build_normalized_input_stream::<$type>(
                    $device, $config, $channels, $rate, $producer, $notifier,
                )?,
            )+
            other => {
                return Err(CaptureError::StreamCreationFailed(
                    format!("Unsupported input sample format: {other:?}")
                ));
            }
        }
    };
}

/// Select device and create a normalized input stream.
fn create_stream(
    device_selection: &DeviceSelection,
    producer: Arc<Mutex<HeapProd<i16>>>,
    sample_notifier: Arc<SampleNotifier>,
) -> Result<cpal::Stream, CaptureError> {
    let host = cpal::default_host();

    let device = match device_selection {
        DeviceSelection::ByIndex(index) => {
            let mut devices = host
                .input_devices()
                .map_err(|e| CaptureError::DeviceNotFound(e.to_string()))?;
            devices.nth(*index as usize).ok_or_else(|| {
                CaptureError::DeviceNotFound(format!("No device at index {index}"))
            })?
        }
        DeviceSelection::Default => host
            .default_input_device()
            .ok_or_else(|| CaptureError::DeviceNotFound("No default input device".into()))?,
    };

    let default_config = device
        .default_input_config()
        .map_err(|e| CaptureError::StreamCreationFailed(e.to_string()))?;
    let config = default_config.config();
    let input_channel_count = usize::from(config.channels);
    let input_sample_rate_hz = config.sample_rate;

    let input_sample_format = default_config.sample_format();

    let stream = dispatch_sample_format!(
        input_sample_format, &device, &config, input_channel_count, input_sample_rate_hz, &producer, &sample_notifier,
        SampleFormat::I8  => i8,
        SampleFormat::I16 => i16,
        SampleFormat::I24 => cpal::I24,
        SampleFormat::I32 => i32,
        SampleFormat::I64 => i64,
        SampleFormat::U8  => u8,
        SampleFormat::U16 => u16,
        SampleFormat::U24 => cpal::U24,
        SampleFormat::U32 => u32,
        SampleFormat::U64 => u64,
        SampleFormat::F32 => f32,
        SampleFormat::F64 => f64,
    );

    stream
        .play()
        .map_err(|e| CaptureError::StreamStartFailed(e.to_string()))?;

    Ok(stream)
}
