import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import type { TerminalTarget } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Props {
  t: Translate;
  onOpen: (target: TerminalTarget) => void;
  onCancel: () => void;
}

/** Opens a terminal: an SSH session to any host, or a shell on this computer. */
export function SshDialog({ t, onOpen, onCancel }: Props) {
  const [host, setHost] = useState("");
  const [port, setPort] = useState("");
  const [keyPath, setKeyPath] = useState("");

  const connect = () => {
    if (!host.trim()) return;
    const parsedPort = Number.parseInt(port, 10);
    onOpen({
      kind: "host",
      host: host.trim(),
      port: Number.isFinite(parsedPort) ? parsedPort : null,
      username: "",
      keyPath: keyPath.trim(),
    });
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="dialog" style={{ width: 440 }}>
        <h2>{t("terminal.newSession")}</h2>
        <div
          className="dialog-body"
          onKeyDown={(e) => {
            if (e.key === "Enter") connect();
            if (e.key === "Escape") onCancel();
          }}
        >
          <div className="row">
            <div className="field" style={{ flex: 1 }}>
              <label>{t("terminal.destination")}</label>
              <input autoFocus value={host} placeholder="user@host" onChange={(e) => setHost(e.target.value)} />
            </div>
            <div className="field" style={{ width: 80 }}>
              <label>{t("connection.port")}</label>
              <input value={port} placeholder="22" inputMode="numeric" onChange={(e) => setPort(e.target.value)} />
            </div>
          </div>
          <div className="field">
            <label>{t("terminal.keyFile")}</label>
            <div className="row">
              <input value={keyPath} placeholder={t("terminal.keyHint")} onChange={(e) => setKeyPath(e.target.value)} />
              <button
                onClick={async () => {
                  const picked = await open({ multiple: false, directory: false });
                  if (typeof picked === "string") setKeyPath(picked);
                }}
              >
                …
              </button>
            </div>
          </div>
          <div className="hint">{t("terminal.configHint")}</div>
        </div>
        <div className="dialog-footer">
          <button onClick={() => onOpen({ kind: "local" })}>{t("terminal.localShell")}</button>
          <div className="spacer" />
          <button onClick={onCancel}>{t("general.cancel")}</button>
          <button className="primary" disabled={!host.trim()} onClick={connect}>
            {t("terminal.connect")}
          </button>
        </div>
      </div>
    </div>
  );
}
