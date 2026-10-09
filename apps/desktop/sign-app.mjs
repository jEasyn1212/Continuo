import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
if (process.platform !== "darwin") {
  console.error("The .app build/sign workflow targets macOS.");
  process.exitCode = 1;
} else {
  const app = fileURLToPath(
    new URL(
      "src-tauri/target/release/bundle/macos/Continuo.app",
      import.meta.url,
    ),
  );
  // Ad-hoc local signature only: '-' does not use a certificate or a keychain identity.
  for (const args of [
    ["--force", "--deep", "--sign", "-", app],
    ["--verify", "--deep", "--strict", app],
  ]) {
    const result = spawnSync("/usr/bin/codesign", args, { stdio: "inherit" });
    if (result.error || result.status !== 0) {
      console.error("Local App signature verification failed.");
      process.exitCode = 1;
      break;
    }
  }
}
