import { createInterface } from "node:readline/promises";
import type { CommandModule } from "yargs";
import {
	clearApiKey,
	getConfigFilePath,
	getStoredApiKey,
	storeApiKey,
} from "../config.js";

async function promptForApiKey(): Promise<string> {
	const rl = createInterface({
		input: process.stdin,
		output: process.stderr,
	});
	try {
		const apiKey = await rl.question(
			"Enter your Gemini API key (from https://aistudio.google.com/apikey): ",
		);
		return apiKey.trim();
	} finally {
		rl.close();
	}
}

interface AuthArgs {
	clear?: boolean;
}

export const authCommand: CommandModule<object, AuthArgs> = {
	command: "auth",
	describe: "Set up or clear your Gemini API key",
	builder: (argv) =>
		argv.option("clear", {
			type: "boolean",
			default: false,
			describe: "Remove the stored API key",
		}),
	handler: async (argv) => {
		if (argv.clear) {
			clearApiKey();
			process.stderr.write("API key removed.\n");
			return;
		}

		const existing = getStoredApiKey();
		if (existing) {
			process.stderr.write(
				`API key already configured (stored in ${getConfigFilePath()}).\n`,
			);
			process.stderr.write("Run 'gemini-voice auth --clear' to remove it.\n");
			return;
		}

		const apiKey = await promptForApiKey();
		if (!apiKey) {
			process.stderr.write("No API key provided.\n");
			process.exit(1);
		}

		storeApiKey(apiKey);
		process.stderr.write(
			`API key saved to ${getConfigFilePath()} (mode 600).\n`,
		);
	},
};
