import { spawn } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync } from "node:fs";
import { delimiter, dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { homedir } from "node:os";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const { version } = JSON.parse(readFileSync(resolve(root, "package.json"), "utf8"));
const [command, ...args] = process.argv.slice(2);
const env = { ...process.env };
const pathKey = Object.keys(env).find((key) => key.toLowerCase() === "path") || "PATH";
env[pathKey] = [
  resolve(env.CARGO_HOME || resolve(homedir(), ".cargo"), "bin"),
  dirname(process.execPath),
  env[pathKey] || "",
].join(delimiter);
const savedPath = resolve(root, ".local/game-path.txt");
if (command === "dev" && !env.WAW_ROOT && existsSync(savedPath)) {
  env.WAW_ROOT = readFileSync(savedPath, "utf8").trim();
}
const child = spawn(process.execPath, [resolve(root, "node_modules/@tauri-apps/cli/tauri.js"), command, ...args], {
  cwd: root,
  env,
  stdio: "inherit",
});
child.on("error", (error) => {
  console.error(error.message);
  process.exitCode = 1;
});
child.on("exit", (code) => {
  process.exitCode = code ?? 1;
  if (code === 0 && command === "build") {
    const release = resolve(root, "release");
    mkdirSync(resolve(release, `v${version}`), { recursive: true });
    const binary = resolve(root, "src-tauri/target/release/waw-sound-player.exe");
    copyFileSync(binary, resolve(release, "waw-sound-player.exe"));
    copyFileSync(binary, resolve(release, `v${version}/waw-sound-player.exe`));
    for (const notice of ["LICENCE", "THIRD_PARTY_NOTICES.txt"]) {
      copyFileSync(resolve(root, notice), resolve(release, `v${version}`, notice));
    }
    console.log("Ready: release/waw-sound-player.exe");
  }
});
