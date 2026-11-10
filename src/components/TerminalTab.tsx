import { useEffect, useRef, useState } from "react";
import { Terminal, type ITheme } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { openUrl } from "@tauri-apps/plugin-opener";
import "@xterm/xterm/css/xterm.css";
import { api, type TerminalTarget } from "../lib/api";
import { ready, subscribe } from "../lib/terminalBus";
import type { Translate } from "../lib/i18n";

interface Props {
  /** A new id is a new session: the parent changes it to reconnect. */
  sessionId: string;
  target: TerminalTarget;
  active: boolean;
  fontSize: number;
  onTitle: (title: string) => void;
  onReconnect: () => void;
  t: Translate;
}

// The 16 ANSI colours read on both the light and the dark app theme; background,
// foreground and cursor come from the app's own tokens so the terminal sits in the
// window rather than on top of it.
const ANSI: ITheme = {
  black: "#1e1e1e", red: "#e06c75", green: "#98c379", yellow: "#d19a66",
  blue: "#61afef", magenta: "#c678dd", cyan: "#56b6c2", white: "#abb2bf",
  brightBlack: "#5c6370", brightRed: "#ef7b84", brightGreen: "#a9d48a", brightYellow: "#e5c07b",
  brightBlue: "#74baf5", brightMagenta: "#d48aea", brightCyan: "#66c7d3", brightWhite: "#ffffff",
};

function themeFromCss(): ITheme {
  const css = getComputedStyle(document.documentElement);
  const token = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    ...ANSI,
    background: token("--bg", "#1e1f22"),
    foreground: token("--text", "#d4d4d4"),
    cursor: token("--accent", "#61afef"),
    selectionBackground: "rgba(97, 175, 239, 0.3)",
  };
}

export function TerminalTab({ sessionId, target, active, fontSize, onTitle, onReconnect, t }: Props) {
  const mountRef = useRef<HTMLDivElement>(null);
  const termRef = useRef<Terminal | null>(null);
  const fitRef = useRef<FitAddon | null>(null);
  const [exitCode, setExitCode] = useState<number | null>(null);

  // Built once per session. Switching tabs only hides it, so the session survives.
  useEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;
    setExitCode(null);

    const terminal = new Terminal({
      fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono").trim() || "monospace",
      fontSize,
      cursorBlink: true,
      scrollback: 10000,
      theme: themeFromCss(),
      macOptionIsMeta: true,
      allowProposedApi: true,
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    // Links open in the real browser, never inside the app window.
    terminal.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri)));
    terminal.open(mount);
    // The GPU renderer is much faster on a busy terminal but missing or flaky on some
    // Linux setups; the DOM renderer is the fallback rather than a blank pane.
    try {
      const webgl = new WebglAddon();
      webgl.onContextLoss(() => webgl.dispose());
      terminal.loadAddon(webgl);
    } catch {
      /* DOM renderer */
    }
    termRef.current = terminal;
    fitRef.current = fit;
    if (mount.clientWidth > 0) fit.fit();

    terminal.onTitleChange((title) => title.trim() && onTitle(title.trim()));
    terminal.onData((data) => void api.terminalWrite(sessionId, data).catch(() => undefined));

    const unsubscribe = subscribe(
      sessionId,
      (bytes) => terminal.write(bytes),
      (code) => {
        setExitCode(code);
        terminal.writeln(`\r\n\x1b[90m[${t("terminal.exited", code)}]\x1b[0m`);
      },
    );

    let disposed = false;
    void (async () => {
      await ready();
      if (disposed) return;
      try {
        const title = await api.terminalOpen(sessionId, target, terminal.cols, terminal.rows);
        onTitle(title);
      } catch (e) {
        const error = e as { message?: string; detail?: string | null };
        terminal.writeln(`\x1b[31m${error.message ?? String(e)}\x1b[0m`);
        if (error.detail) terminal.writeln(`\x1b[90m${error.detail}\x1b[0m`);
        setExitCode(1);
      }
    })();

    // Catches window resizes and the tab becoming visible again; a hidden tab has no
    // size, and fitting it then would shrink the session to one column.
    const observer = new ResizeObserver(() => {
      if (mount.clientWidth === 0 || mount.clientHeight === 0) return;
      try {
        fit.fit();
        void api.terminalResize(sessionId, terminal.cols, terminal.rows);
      } catch {
        /* mid-teardown */
      }
    });
    observer.observe(mount);

    return () => {
      disposed = true;
      observer.disconnect();
      unsubscribe();
      terminal.dispose();
      void api.terminalClose(sessionId);
    };
    // One terminal per session; the other props are read once or applied below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  useEffect(() => {
    const terminal = termRef.current;
    if (!terminal) return;
    terminal.options.fontSize = fontSize;
    if (mountRef.current?.clientWidth) fitRef.current?.fit();
  }, [fontSize]);

  useEffect(() => {
    if (!active) return;
    // The theme follows the app's light/dark switch the next time the tab is shown.
    if (termRef.current) termRef.current.options.theme = themeFromCss();
    termRef.current?.focus();
  }, [active]);

  return (
    <div className="terminal-host" style={{ display: active ? "flex" : "none" }}>
      <div ref={mountRef} className="terminal-mount" />
      {exitCode !== null && (
        <div className="terminal-ended">
          <span>{t("terminal.exited", exitCode)}</span>
          <button className="primary" onClick={onReconnect}>
            {t("terminal.reconnect")}
          </button>
        </div>
      )}
    </div>
  );
}
