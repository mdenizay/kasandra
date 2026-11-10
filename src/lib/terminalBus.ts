// One listener for the whole window, routed by session id.
//
// A listener per tab would wake every terminal for every other terminal's output and
// have it filter the chunk away. On a busy session that is a lot of wasted work on the
// hot path, so the single listener does the routing instead.

import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type DataHandler = (bytes: Uint8Array) => void;
type ExitHandler = (code: number) => void;

const dataHandlers = new Map<string, DataHandler>();
const exitHandlers = new Map<string, ExitHandler>();
let started: Promise<UnlistenFn[]> | null = null;

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i += 1) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

function ensureStarted(): Promise<UnlistenFn[]> {
  started ??= Promise.all([
    listen<{ id: string; data: string }>("terminal:data", (event) => {
      dataHandlers.get(event.payload.id)?.(decodeBase64(event.payload.data));
    }),
    listen<{ id: string; code: number }>("terminal:exit", (event) => {
      exitHandlers.get(event.payload.id)?.(event.payload.code);
    }),
  ]);
  return started;
}

/**
 * Resolves once the window is listening. A tab must wait for this before opening its
 * session: `listen` is asynchronous, and output emitted before it resolves is lost — which
 * shows up as a terminal missing its first prompt.
 */
export function ready(): Promise<unknown> {
  return ensureStarted();
}

export function subscribe(id: string, onData: DataHandler, onExit: ExitHandler): () => void {
  dataHandlers.set(id, onData);
  exitHandlers.set(id, onExit);
  void ensureStarted();
  return () => {
    dataHandlers.delete(id);
    exitHandlers.delete(id);
  };
}
