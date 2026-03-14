import { appendFileSync } from "node:fs";
import { createRequire } from "node:module";
import type {
	LiveConnectConfig,
	LiveServerMessage,
	Session,
} from "@google/genai";
import { GoogleGenAI, Modality } from "@google/genai";
import { render } from "ink";
import React from "react";
import type { CommandModule } from "yargs";
import type { TranscribeState } from "../components/TranscribeUI.js";
import { TranscribeUI } from "../components/TranscribeUI.js";
import { getStoredApiKey } from "../config.js";

const require = createRequire(import.meta.url);
const { Recorder } =
	require("@kstonekuan/audio-capture") as typeof import("@kstonekuan/audio-capture");

const GEMINI_MODEL = "gemini-2.5-flash-native-audio-preview-12-2025";
const AUDIO_MIME_TYPE = "audio/pcm;rate=16000";

// After the first transcription arrives, wait this long for a `finished: true`
// signal or additional fragments before closing (ms).
const TRANSCRIPT_SETTLE_TIMEOUT_MS = 2000;

const DEBUG_LOG_PATH = "/tmp/gemini-debug.log";

function createDebugLogger(enabled: boolean): (message: string) => void {
	if (!enabled) return () => {};
	return (message: string) => appendFileSync(DEBUG_LOG_PATH, message);
}

function getApiKey(): string {
	const apiKey = getStoredApiKey();
	if (!apiKey) {
		process.stderr.write(
			"Error: No API key found. Run 'gemini-voice auth' to set one up.\n",
		);
		process.exit(1);
	}
	return apiKey;
}

function int16ArrayToBase64(samples: Int16Array): string {
	const bytes = new Uint8Array(
		samples.buffer,
		samples.byteOffset,
		samples.byteLength,
	);
	return Buffer.from(bytes).toString("base64");
}

function computeRmsLevel(samples: Int16Array): number {
	if (samples.length === 0) return 0;
	let sumOfSquares = 0;
	for (const sample of samples) {
		const normalized = sample / 32768;
		sumOfSquares += normalized * normalized;
	}
	const rms = Math.sqrt(sumOfSquares / samples.length);
	// Logarithmic (dB) scale for perceptually responsive metering.
	// Floor at -50dB to ignore background noise, ceiling at -6dB.
	const minDb = -50;
	const maxDb = -6;
	const db = 20 * Math.log10(rms || Number.MIN_VALUE);
	if (db < minDb) return 0;
	return Math.min(1, (db - minDb) / (maxDb - minDb));
}

type SessionPhase =
	| { phase: "connecting" }
	| { phase: "active"; session: Session }
	| { phase: "shutting_down" };

type RecorderState = {
	sessionPhase: SessionPhase;
	recorder: InstanceType<typeof Recorder>;
	transcriptParts: string[];
	settleTimeoutId: ReturnType<typeof setTimeout> | undefined;
	debugLog: (message: string) => void;
	updateUI: (
		state: TranscribeState,
		audioLevel: number,
		transcript: string,
	) => void;
};

const UI_UPDATE_INTERVAL_MS = 66; // ~15fps

function startMicrophoneCapture(state: RecorderState): void {
	let lastUiUpdateTime = 0;

	state.recorder.start((error: Error | null, samples: Int16Array) => {
		if (error) {
			process.stderr.write(`Audio capture error: ${error.message}\n`);
			return;
		}
		if (state.sessionPhase.phase !== "active") return;

		const audioLevel = computeRmsLevel(samples);

		const now = Date.now();
		if (now - lastUiUpdateTime >= UI_UPDATE_INTERVAL_MS) {
			lastUiUpdateTime = now;
			const runningTranscript = state.transcriptParts.join("").trim();
			state.updateUI("listening", audioLevel, runningTranscript);
		}

		const base64Audio = int16ArrayToBase64(samples);
		state.debugLog(
			`[AUDIO] len=${samples.length} rms=${audioLevel.toFixed(3)}\n`,
		);
		state.sessionPhase.session.sendRealtimeInput({
			audio: { data: base64Audio, mimeType: AUDIO_MIME_TYPE },
		});
	});
}

function handleServerMessage(
	message: LiveServerMessage,
	state: RecorderState,
): void {
	const serverContent = message.serverContent;
	if (!serverContent) return;

	if (serverContent.inputTranscription?.text) {
		state.transcriptParts.push(serverContent.inputTranscription.text);
		const runningTranscript = state.transcriptParts.join("").trim();
		state.updateUI("listening", 0, runningTranscript);

		if (state.settleTimeoutId) {
			clearTimeout(state.settleTimeoutId);
		}

		if (serverContent.inputTranscription.finished) {
			shutdownGracefully(state);
			return;
		}

		state.settleTimeoutId = setTimeout(() => {
			shutdownGracefully(state);
		}, TRANSCRIPT_SETTLE_TIMEOUT_MS);
	}

	if (serverContent.turnComplete && !serverContent.modelTurn) {
		if (!state.settleTimeoutId && state.transcriptParts.length === 0) {
			state.settleTimeoutId = setTimeout(() => {
				shutdownGracefully(state);
			}, TRANSCRIPT_SETTLE_TIMEOUT_MS);
		}
	}
}

function shutdownGracefully(state: RecorderState): void {
	if (state.sessionPhase.phase === "shutting_down") return;
	const session =
		state.sessionPhase.phase === "active" ? state.sessionPhase.session : null;
	state.sessionPhase = { phase: "shutting_down" };

	if (state.settleTimeoutId) {
		clearTimeout(state.settleTimeoutId);
		state.settleTimeoutId = undefined;
	}

	state.recorder.stop();

	const transcript = state.transcriptParts.join("").trim();
	if (transcript.length > 0) {
		state.updateUI("done", 0, transcript);
		process.stdout.write(transcript);
		process.stdout.write("\n");
	} else {
		state.updateUI("done", 0, "No speech detected.");
		process.stderr.write("No speech detected.\n");
	}

	session?.close();
}

interface TranscribeArgs {
	device?: number;
	debug?: boolean;
	quiet?: boolean;
}

async function runTranscribe(
	deviceIndex?: number,
	debug?: boolean,
	quiet?: boolean,
): Promise<void> {
	const debugLog = createDebugLogger(debug ?? false);
	const apiKey = getApiKey();
	const ai = new GoogleGenAI({ apiKey });

	const liveConnectConfig: LiveConnectConfig = {
		responseModalities: [Modality.AUDIO],
		inputAudioTranscription: {},
		realtimeInputConfig: {
			automaticActivityDetection: {
				disabled: false,
				prefixPaddingMs: 100,
				silenceDurationMs: 500,
			},
		},
		systemInstruction: {
			parts: [
				{
					text: "You are a transcription assistant. Respond with a single word 'ok' after each user message. Keep responses minimal.",
				},
			],
		},
	};

	const recorder = new Recorder(deviceIndex ?? null);

	let updateUI: (
		state: TranscribeState,
		audioLevel: number,
		transcript: string,
	) => void;

	if (quiet) {
		updateUI = () => {};
	} else {
		let currentState: TranscribeState = "connecting";
		let currentAudioLevel = 0;
		let currentTranscript = "";

		const { rerender, unmount } = render(
			React.createElement(TranscribeUI, {
				state: currentState,
				audioLevel: currentAudioLevel,
				transcript: currentTranscript,
			}),
			{ stdout: process.stderr },
		);

		updateUI = (
			newState: TranscribeState,
			audioLevel: number,
			transcript: string,
		): void => {
			currentState = newState;
			currentAudioLevel = audioLevel;
			currentTranscript = transcript;
			rerender(
				React.createElement(TranscribeUI, {
					state: currentState,
					audioLevel: currentAudioLevel,
					transcript: currentTranscript,
				}),
			);
			if (newState === "done") {
				unmount();
			}
		};
	}

	// connect() resolves after the WebSocket opens and the setup message is
	// sent. The official example starts streaming audio immediately after
	// connect() — no need to wait for setupComplete.
	process.on("unhandledRejection", (reason) => {
		debugLog(`[UNHANDLED] ${String(reason)}\n`);
	});

	debugLog("[START] connecting...\n");
	let resolveSetupComplete: () => void;
	const setupCompletePromise = new Promise<void>((resolve) => {
		resolveSetupComplete = resolve;
	});

	const state: RecorderState = {
		sessionPhase: { phase: "connecting" },
		recorder,
		transcriptParts: [],
		settleTimeoutId: undefined,
		debugLog,
		updateUI,
	};

	const session = await ai.live.connect({
		model: GEMINI_MODEL,
		config: liveConnectConfig,
		callbacks: {
			onopen: () => {},
			onmessage: (message: LiveServerMessage) => {
				debugLog(`[MSG] keys: ${JSON.stringify(Object.keys(message))}\n`);
				if (message.setupComplete) {
					debugLog("[MSG] setupComplete\n");
					resolveSetupComplete();
					return;
				}
				if (message.serverContent) {
					debugLog(
						`[MSG] serverContent: ${JSON.stringify(message.serverContent)}\n`,
					);
				}
				handleServerMessage(message, state);
			},
			onerror: (error: ErrorEvent) => {
				debugLog(`[ERR] ${error.message}\n`);
				process.stderr.write(`Live API error: ${error.message}\n`);
				shutdownGracefully(state);
			},
			onclose: (event: CloseEvent) => {
				debugLog(`[CLOSE] code=${event.code} reason=${event.reason}\n`);
			},
		},
	});

	state.sessionPhase = { phase: "active", session };

	debugLog("[START] connected, waiting for setupComplete\n");
	await setupCompletePromise;
	debugLog("[START] setupComplete received, starting mic\n");
	updateUI("listening", 0, "");
	startMicrophoneCapture(state);

	process.on("SIGINT", () => {
		process.stderr.write("\nInterrupted.\n");
		shutdownGracefully(state);
	});
}

export const transcribeCommand: CommandModule<object, TranscribeArgs> = {
	command: "transcribe",
	describe: "Capture microphone audio and transcribe via Gemini Live API",
	builder: (argv) =>
		argv
			.option("device", {
				alias: "d",
				type: "number",
				describe: "Audio input device index (use 'devices' command to list)",
			})
			.option("debug", {
				type: "boolean",
				default: false,
				describe: "Write debug logs to /tmp/gemini-debug.log",
			})
			.option("quiet", {
				alias: "q",
				type: "boolean",
				default: false,
				describe: "Suppress all UI output, only write transcript to stdout",
			}),
	handler: async (argv) => {
		await runTranscribe(argv.device, argv.debug, argv.quiet);
	},
};
