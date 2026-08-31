import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export const isNativeRuntime = typeof window !== "undefined" && Boolean(window.__TAURI_INTERNALS__);

export function invokeNative<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!isNativeRuntime) {
    return Promise.reject(new Error("Native runtime is not available."));
  }

  return invoke<T>(command, args);
}

export function startNativeDragging(): Promise<void> {
  if (!isNativeRuntime) return Promise.resolve();
  return getCurrentWindow().startDragging();
}

export function listenNative<T>(event: string, handler: (payload: T) => void): Promise<UnlistenFn> {
  if (!isNativeRuntime) {
    return Promise.reject(new Error("Native runtime is not available."));
  }

  return listen<T>(event, (eventPayload) => handler(eventPayload.payload));
}
