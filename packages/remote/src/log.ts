import type { ComponentLogger, Logger } from "@libp2p/interface";

// Noise's upstream debug mode can dump session keys. Never route its logger to
// console, even when DEBUG or DUMP_SESSION_KEYS is enabled in the parent process.
const noop = () => {};
export const silentLog: Logger = Object.assign(noop, {
  enabled: false,
  error: noop,
  trace: noop,
  info: noop,
  warn: noop,
  newScope: (): Logger => silentLog,
});
export const silentLogger: ComponentLogger = { forComponent: () => silentLog };
