#!/usr/bin/env node
import yargs from "yargs";
import { hideBin } from "yargs/helpers";
import { authCommand } from "./commands/auth.js";
import { devicesCommand } from "./commands/devices.js";
import { transcribeCommand } from "./commands/transcribe.js";

yargs(hideBin(process.argv))
	.scriptName("gemini-voice")
	.command(authCommand)
	.command(transcribeCommand)
	.command(devicesCommand)
	.demandCommand(1, "Please specify a command")
	.strict()
	.help()
	.parse();
