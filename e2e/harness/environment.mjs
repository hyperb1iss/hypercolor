import fs from "node:fs";

/**
 * Variables that would point a harness daemon or CLI at the invoking user's
 * own daemon, config, or session bus.
 */
const AMBIENT_VARIABLES = new Set([
  "HYPERCOLOR_HOST",
  "HYPERCOLOR_PORT",
  "HYPERCOLOR_PROFILE",
  "HYPERCOLOR_API_KEY",
  "HYPERCOLOR_CLI_CONFIG",
  "HYPERCOLOR_CONFIG",
  "DBUS_SESSION_BUS_ADDRESS",
]);

const LOOPBACK_HOSTS = ["127.0.0.1", "localhost", "::1"];

/**
 * Fail before anything starts on a platform where HOME and the XDG roots do
 * not relocate the daemon's directories. Windows resolves them through
 * known-folder APIs, so a harness daemon there would share the user's state.
 */
export function assertIsolationSupported(platform = process.platform) {
  if (platform === "win32") {
    throw new Error(
      "The e2e harness isolates the daemon through HOME and XDG variables, " +
        "which Windows ignores; run the suite on Linux or macOS.",
    );
  }
}

/**
 * The environment every harness daemon and CLI runs in: the invoking shell's,
 * minus anything that names the user's own daemon or directories, with HOME,
 * every XDG root, and the runtime directory inside the run directory. The
 * daemon keeps its runtime session and audit trail in the XDG state home, so
 * inheriting the user's would write their live daemon's state. Loopback
 * requests bypass any inherited proxy.
 */
export function isolatedEnv(stack) {
  const env = {};
  for (const [key, value] of Object.entries(process.env)) {
    if (!key.startsWith("XDG_") && !AMBIENT_VARIABLES.has(key)) {
      env[key] = value;
    }
  }
  const noProxy = [env.NO_PROXY ?? env.no_proxy, ...LOOPBACK_HOSTS]
    .filter(Boolean)
    .join(",");
  return {
    ...env,
    NO_PROXY: noProxy,
    no_proxy: noProxy,
    HYPERCOLOR_E2E: "1",
    NO_COLOR: "1",
    HOME: stack.homeDir,
    XDG_CONFIG_HOME: stack.xdgConfigHome,
    XDG_DATA_HOME: stack.xdgDataHome,
    XDG_STATE_HOME: stack.xdgStateHome,
    XDG_CACHE_HOME: stack.xdgCacheHome,
    XDG_RUNTIME_DIR: stack.xdgRuntimeDir,
  };
}

/**
 * Fail unless the daemon answering at `apiOrigin` is the one this harness
 * started, identified by the config file it was launched with.
 */
export async function assertHarnessDaemon(apiOrigin, configPath) {
  const response = await fetch(`${apiOrigin}/api/v1/system`, {
    headers: { accept: "application/json" },
  });
  if (!response.ok) {
    throw new Error(
      `GET ${apiOrigin}/api/v1/system answered HTTP ${response.status}; ` +
        "cannot confirm it is the harness daemon",
    );
  }
  const body = await response.json();
  const answeringConfig = body?.data?.status?.config_path;
  if (!answeringConfig || realPath(answeringConfig) !== realPath(configPath)) {
    throw new Error(
      `the daemon at ${apiOrigin} runs ${answeringConfig ?? "an unreported config"}, ` +
        `not the harness config ${configPath}`,
    );
  }
}

function realPath(filePath) {
  try {
    return fs.realpathSync(filePath);
  } catch {
    return filePath;
  }
}
