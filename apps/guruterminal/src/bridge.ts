import type { GuruTerminalBridge } from "./types";
import { TauriGuruTerminalBridge } from "./bridge/tauriBridge";

export { MockGuruTerminalBridge } from "./bridge/mockBridge";
export { TAURI_COMMANDS, TAURI_STREAM_CHANNEL_ARGUMENT } from "./bridge/commands";

export const createGuruTerminalBridge = (): GuruTerminalBridge => {
  return new TauriGuruTerminalBridge();
};
