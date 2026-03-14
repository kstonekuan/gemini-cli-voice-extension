# @kstonekuan/audio-capture

Native Node.js addon for cross-platform microphone capture. Built with Rust ([cpal](https://github.com/RustAudio/cpal)) and exposed to JavaScript via [NAPI-RS](https://napi.rs/).

Audio is captured from the system microphone, normalized to **16kHz 16-bit PCM mono**, and delivered to a JavaScript callback in real time. The architecture uses a lock-free ring buffer to decouple the OS audio thread from Node.js, so the real-time callback never blocks on JS execution.

## Installation

```bash
npm install @kstonekuan/audio-capture
```

Pre-built native binaries are included for:

| Platform       | Architecture |
| -------------- | ------------ |
| macOS          | arm64, x64   |
| Linux (glibc)  | x64          |
| Windows (MSVC) | x64          |

No Rust toolchain is needed for end users.

## Usage

### Capture audio

```js
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
const { Recorder } = require("@kstonekuan/audio-capture");

const recorder = new Recorder();

recorder.start((error, samples) => {
  if (error) {
    console.error("Capture error:", error.message);
    return;
  }
  // `samples` is an Int16Array of 16kHz 16-bit PCM mono audio
  console.log(`Received ${samples.length} samples at ${recorder.sampleRate}Hz`);
});

// Stop capturing when done
setTimeout(() => {
  recorder.stop();
}, 5000);
```

### List audio devices

```js
const devices = Recorder.getAudioDevices();
for (const device of devices) {
  console.log(`${device.index}: ${device.name}`);
}
```

### Select a specific device

```js
const recorder = new Recorder(1); // use device at index 1
```

## API

### `new Recorder(deviceIndex?: number)`

Creates a new recorder instance. Pass a device index to capture from a specific input device, or omit it to use the system default.

### `recorder.start(callback: (error: Error | null, samples: Int16Array) => void): void`

Start capturing audio. The callback is invoked on a dedicated drain thread each time new samples are available from the microphone. Samples are 16kHz 16-bit signed PCM, mono.

### `recorder.stop(): void`

Stop capturing and release the audio stream.

### `recorder.sampleRate: number`

The output sample rate in Hz (always `16000`).

### `Recorder.getAudioDevices(): AudioDevice[]`

Returns a list of available audio input devices.

### `AudioDevice`

```ts
interface AudioDevice {
  index: number;
  name: string;
}
```

## Architecture

```
Microphone
  -> cpal (OS audio thread)
  -> normalize to 16kHz mono
  -> lock-free SPSC ring buffer
  -> drain thread
  -> NAPI ThreadsafeFunction
  -> JavaScript callback (Int16Array)
```

Three threads are involved:

1. **Audio thread** -- owns the cpal stream lifecycle (start/stop/shutdown)
2. **cpal callback** (OS real-time thread) -- normalizes samples (channel downmix + resample) and pushes `i16` into the ring buffer. Never blocks.
3. **Drain thread** -- waits on a condvar, reads all available samples from the ring buffer, and invokes the JavaScript callback via a NAPI ThreadsafeFunction (non-blocking)

The ring buffer decouples the real-time cpal callback from the drain thread, so the callback never contends with JS-bound callback invocation.

## Usage example

[gemini-cli-voice-extension](https://github.com/kstonekuan/gemini-cli-voice-extension) uses `@kstonekuan/audio-capture` to stream microphone audio to the Gemini Live API for real-time speech transcription:

```ts
const recorder = new Recorder(deviceIndex ?? null);

recorder.start((error, samples) => {
  if (error) return;

  // Base64-encode the PCM samples and stream to Gemini Live API
  const base64Audio = int16ArrayToBase64(samples);
  session.sendRealtimeInput({
    audio: { data: base64Audio, mimeType: "audio/pcm;rate=16000" },
  });
});
```

The 16kHz PCM output matches what the Gemini Live API expects, so no additional conversion is needed.

## Building from source

Requires a [Rust toolchain](https://rustup.rs/) and system audio libraries (ALSA dev headers on Linux).

```bash
pnpm install
pnpm build   # runs: napi build --platform --release
```

## License

MIT
