// PostToolUse: rustfmt the .rs file Claude just edited, so `cargo fmt --check` in CI stays green.
import { readFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';

const input = JSON.parse(readFileSync(0, 'utf8') || '{}');
const file = input.tool_input?.file_path ?? '';
if (file.endsWith('.rs')) {
	spawnSync('rustfmt', ['--edition', '2021', file], { stdio: 'ignore', timeout: 20000 });
}
