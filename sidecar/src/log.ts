// Logs go to stderr: stdout carries the JSON-RPC stream and nothing else.

export type LogLevel = "debug" | "info" | "warn" | "error";

export interface Logger {
  debug(message: string, ...details: unknown[]): void;
  info(message: string, ...details: unknown[]): void;
  warn(message: string, ...details: unknown[]): void;
  error(message: string, ...details: unknown[]): void;
  child(scope: string): Logger;
}

const ORDER: Record<LogLevel, number> = { debug: 0, info: 1, warn: 2, error: 3 };

function parseLevel(value: string | undefined): LogLevel {
  return value === "debug" || value === "info" || value === "warn" || value === "error" ? value : "info";
}

export function createLogger(
  scope = "sidecar",
  level: LogLevel = parseLevel(process.env.CHAUFFEUR_SIDECAR_LOG),
  write: (line: string) => void = (line) => process.stderr.write(line),
): Logger {
  const log = (at: LogLevel, message: string, details: unknown[]) => {
    if (ORDER[at] < ORDER[level]) return;
    const suffix = details.map(formatDetail).join(" ");
    write(`${new Date().toISOString()} ${at.toUpperCase()} [${scope}] ${message}${suffix ? ` ${suffix}` : ""}\n`);
  };
  return {
    debug: (message, ...details) => log("debug", message, details),
    info: (message, ...details) => log("info", message, details),
    warn: (message, ...details) => log("warn", message, details),
    error: (message, ...details) => log("error", message, details),
    child: (child) => createLogger(`${scope}:${child}`, level, write),
  };
}

function formatDetail(detail: unknown): string {
  if (detail instanceof Error) return detail.stack ?? `${detail.name}: ${detail.message}`;
  if (typeof detail === "string") return detail;
  try {
    return JSON.stringify(detail);
  } catch {
    return String(detail);
  }
}

/** A logger that drops everything, for tests. */
export const silentLogger: Logger = {
  debug: () => {},
  info: () => {},
  warn: () => {},
  error: () => {},
  child: () => silentLogger,
};

export function errorMessage(error: unknown): string {
  if (error instanceof Error) return error.message;
  return typeof error === "string" ? error : JSON.stringify(error);
}
