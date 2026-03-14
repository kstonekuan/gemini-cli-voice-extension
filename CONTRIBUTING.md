# Contributing

## Project structure

The Gemini CLI extension files (`gemini-extension.json`, `commands/`) live at the repo root.

### `audio-capture/` (`@kstonekuan/audio-capture`)

Native Node.js addon for cross-platform microphone capture, built with [NAPI-RS](https://napi.rs/). Pre-built binaries are published for macOS (arm64, x64), Linux (x64), and Windows (x64). Published separately to npm.

| File                              | Purpose                                                              |
| --------------------------------- | -------------------------------------------------------------------- |
| `audio-capture/src/lib.rs`        | NAPI bindings -- exposes `Recorder` class to JavaScript              |
| `audio-capture/src/capture.rs`    | Audio capture engine using cpal with ring buffer                     |
| `audio-capture/src/normalizer.rs` | Sample rate and format normalization (output: 16kHz 16-bit PCM mono) |

### `src/` (`@kstonekuan/gemini-voice`)

CLI and Gemini CLI extension. Connects to the Gemini Live API, streams microphone audio for transcription, and renders an Ink-based terminal UI.

| File                              | Purpose                                                                                 |
| --------------------------------- | --------------------------------------------------------------------------------------- |
| `src/cli.ts`                      | Yargs CLI entry point                                                                   |
| `src/config.ts`                   | API key storage (~/.config/gemini-voice/config.json)                                    |
| `src/commands/auth.ts`            | Auth command -- set up or clear the stored API key                                      |
| `src/commands/transcribe.ts`      | Transcription command -- connects to Gemini Live API, captures audio, handles lifecycle |
| `src/commands/devices.ts`         | Lists available audio input devices                                                     |
| `src/components/TranscribeUI.tsx` | Ink component -- spinner, audio level meter, transcript display                         |
| `src/index.ts`                    | Package entry point                                                                     |

## Setup

```bash
git clone https://github.com/kstonekuan/gemini-cli-voice-extension
cd gemini-cli-voice-extension
pnpm install
pnpm build               # builds native addon + TypeScript
pnpm link --global        # makes `gemini-voice` CLI available on PATH
gemini extensions link .  # links the extension to Gemini CLI
```

## Code quality

### All checks

```bash
pnpm check           # Lint + typecheck + clippy + fmt
```

### TypeScript

```bash
pnpm lint            # Biome linting with auto-fix
pnpm typecheck       # TypeScript type checking
```

### Rust

```bash
pnpm cargo:clippy    # Clippy linting
pnpm cargo:fmt       # Formatting
```

## Releasing

Publishing is automated via GitHub Actions. Pushing a version tag triggers the publish workflow, which builds native binaries on all platforms and publishes everything to npm.

### Prerequisites

- An npm account with access to the `@kstonekuan` scope
- An `NPM_TOKEN` secret set in the repo (Settings > Secrets > Actions) -- use an "Automation" type token

### Steps

1. Update the version in all `package.json` files:
   ```bash
   # Root, audio-capture/, and audio-capture/npm/*/package.json all need to match
   ```

2. Tag and push:
   ```bash
   git tag v0.0.1
   git push origin v0.0.1
   ```

3. The [publish workflow](.github/workflows/publish.yml) will:
   - Build the native addon on macOS (arm64, x64), Linux (x64), and Windows (x64)
   - Publish the 4 platform packages (`@kstonekuan/audio-capture-darwin-arm64`, etc.)
   - Publish `@kstonekuan/audio-capture`
   - Build TypeScript and publish `@kstonekuan/gemini-voice`

### Packages published

| Package | Description |
| ------- | ----------- |
| `@kstonekuan/audio-capture` | Native audio capture addon (main package) |
| `@kstonekuan/audio-capture-darwin-arm64` | macOS arm64 binary |
| `@kstonekuan/audio-capture-darwin-x64` | macOS x64 binary |
| `@kstonekuan/audio-capture-linux-x64-gnu` | Linux x64 binary |
| `@kstonekuan/audio-capture-win32-x64-msvc` | Windows x64 binary |
| `@kstonekuan/gemini-voice` | CLI and Gemini CLI extension |

## Code style

### Typing & pattern matching

- Prefer **explicit types** over raw dicts -- make invalid states unrepresentable where practical
- Prefer **typed variants over string literals** when the set of valid values is known
- Use **exhaustive pattern matching** (`match` in Python and Rust, `ts-pattern` in TypeScript) so the type checker can verify all cases are handled
- Structure types to enable exhaustive matching when handling variants
- Prefer **shared internal functions over factory patterns** when extracting common logic from hooks or functions -- keep each export explicitly defined for better IDE navigation and readability

### Self-documenting code

- **Verbose naming**: Variable and function naming should read like documentation
- **Strategic comments**: Only for non-obvious logic or architectural decisions; avoid restating what code shows
