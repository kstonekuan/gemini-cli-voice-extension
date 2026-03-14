import { createRequire } from "node:module";
import type { CommandModule } from "yargs";

const require = createRequire(import.meta.url);
const { Recorder } =
	require("@kstonekuan/audio-capture") as typeof import("@kstonekuan/audio-capture");

export const devicesCommand: CommandModule = {
	command: "devices",
	describe: "List available audio input devices",
	handler: () => {
		const devices = Recorder.getAudioDevices();
		if (devices.length === 0) {
			process.stderr.write("No audio input devices found.\n");
			process.exit(1);
		}
		for (const device of devices) {
			process.stdout.write(`${device.index}: ${device.name}\n`);
		}
	},
};
