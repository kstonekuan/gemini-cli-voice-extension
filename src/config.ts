import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const CONFIG_DIR = join(homedir(), ".config", "gemini-voice");
const CONFIG_FILE = join(CONFIG_DIR, "config.json");

interface Config {
	apiKey?: string;
}

function readConfig(): Config {
	if (!existsSync(CONFIG_FILE)) return {};
	try {
		return JSON.parse(readFileSync(CONFIG_FILE, "utf-8")) as Config;
	} catch {
		return {};
	}
}

function writeConfig(config: Config): void {
	mkdirSync(CONFIG_DIR, { recursive: true });
	writeFileSync(CONFIG_FILE, `${JSON.stringify(config, null, "\t")}\n`, {
		mode: 0o600,
	});
}

export function getStoredApiKey(): string | undefined {
	return readConfig().apiKey;
}

export function storeApiKey(apiKey: string): void {
	const config = readConfig();
	config.apiKey = apiKey;
	writeConfig(config);
}

export function clearApiKey(): void {
	const config = readConfig();
	delete config.apiKey;
	writeConfig(config);
}

export function getConfigFilePath(): string {
	return CONFIG_FILE;
}
