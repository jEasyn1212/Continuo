import { spawn } from "node:child_process";
import { access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
if (process.platform !== "darwin") {
  console.error(
    "This App demo launcher currently targets macOS. Use tauri dev on other platforms.",
  );
  process.exitCode = 1;
} else {
  const binary = fileURLToPath(
    new URL(
      "src-tauri/target/release/bundle/macos/Continuo.app/Contents/MacOS/continuo-desktop",
      import.meta.url,
    ),
  );
  try {
    await access(binary);
    const data = fileURLToPath(new URL("../../.local-demo", import.meta.url));
    console.log(
      "Opening Continuo App with the isolated project demo vault, shared with demo:web.",
    );
    const child = spawn(binary, [], {
      stdio: "inherit",
      env: { ...process.env, CONTINUO_DATA_DIR: data },
    });
    child.on("error", () => {
      console.error(
        "App could not start. Build it locally with npm run app:build.",
      );
      process.exitCode = 1;
    });
    child.on("exit", (code) => {
      process.exitCode = code ?? 0;
    });
    process.on("SIGINT", () => child.kill("SIGINT"));
    process.on("SIGTERM", () => child.kill("SIGTERM"));
  } catch {
    console.error("Build the App first: npm run app:build");
    process.exitCode = 1;
  }
}
