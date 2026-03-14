import { Box, Text } from "ink";
import Spinner from "ink-spinner";
import type React from "react";
import { useRef } from "react";
import { match } from "ts-pattern";

export type TranscribeState = "connecting" | "listening" | "done";

interface TranscribeUIProps {
	state: TranscribeState;
	audioLevel: number;
	transcript: string;
}

const BAR_COUNT = 16;

// Block elements ▁▂▃▄▅▆▇█ growing upward from the bottom of the cell.
const BLOCKS = [
	" ",
	"\u2581",
	"\u2582",
	"\u2583",
	"\u2584",
	"\u2585",
	"\u2586",
	"\u2587",
	"\u2588",
];

const MIN_BLOCK_INDEX = 1;

function levelToBlock(level: number): string {
	const clamped = Math.max(0, Math.min(1, level));
	const index =
		MIN_BLOCK_INDEX +
		Math.round(clamped * (BLOCKS.length - 1 - MIN_BLOCK_INDEX));
	return BLOCKS[index];
}

// Exponential smoothing factors: bars rise fast but fall slowly.
const ATTACK_FACTOR = 0.85;
const DECAY_FACTOR = 0.15;

function AudioLevelMeter({ level }: { level: number }): React.ReactElement {
	const smoothedRef = useRef<number[]>(new Array(BAR_COUNT).fill(0));
	const phaseRef = useRef(Math.random() * Math.PI * 2);

	// Advance phase based on level so the wave moves when there's audio.
	phaseRef.current += level * 0.6 + 0.02;

	const smoothedLevels = Array.from({ length: BAR_COUNT }, (_, i) => {
		// Two overlapping sine waves at different frequencies create organic,
		// spatially correlated variation — adjacent bars have similar heights.
		const wave1 = Math.sin(phaseRef.current + i * 0.7) * 0.3;
		const wave2 = Math.sin(phaseRef.current * 1.3 + i * 1.1) * 0.2;
		const targetLevel = level + (wave1 + wave2) * level;

		const previous = smoothedRef.current[i];
		const factor = targetLevel > previous ? ATTACK_FACTOR : DECAY_FACTOR;
		const smoothed = previous + factor * (targetLevel - previous);
		smoothedRef.current[i] = smoothed;
		return smoothed;
	});

	const row = smoothedLevels.map((l) => levelToBlock(l)).join(" ");

	return <Text>{row}</Text>;
}

export function TranscribeUI({
	state,
	audioLevel,
	transcript,
}: TranscribeUIProps): React.ReactElement {
	return match(state)
		.with("connecting", () => (
			<Box>
				<Text color="yellow">
					<Spinner type="dots" /> Connecting to Gemini Live API...
				</Text>
			</Box>
		))
		.with("listening", () => (
			<Box flexDirection="column">
				<Box>
					<Text color="green">
						<Spinner type="dots" /> Listening... (silence ends recording)
					</Text>
				</Box>
				<Box>
					<Text> </Text>
					<AudioLevelMeter level={audioLevel} />
				</Box>
				{transcript.length > 0 && (
					<Box>
						<Text dimColor>{transcript}</Text>
					</Box>
				)}
			</Box>
		))
		.with("done", () => (
			<Box>
				<Text color="green">
					{"✓ "}
					{transcript}
				</Text>
			</Box>
		))
		.exhaustive();
}
