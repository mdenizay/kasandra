import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { TrashIcon, ServerIcon, PlusIcon } from "@heroicons/react/24/outline";
import { api, type FileProtocol, type FileSite, type FileTarget } from "../lib/api";
import type { Translate } from "../lib/i18n";

interface Props {
  t: Translate;
  onOpen: (target: FileTarget, title: string) => void;
  onCancel: () => void;
}

function blankSite(): FileSite {
  return {
    id: crypto.randomUUID(),
    name: "",
    protocol: "sftp",
    host: "",
    port: 0,
    username: "",
    keyPath: "",
    initialPath: "",
    savePassword: true,
    activeMode: false,
  };
}

function siteTitle(site: FileSite): string {
  if (site.name.trim()) return site.name.trim();
  return site.username ? `${site.username}@${site.host}` : site.host;
}

const DEFAULT_PORT: Record<FileProtocol, number> = { ftp: 21, ftps: 21, sftp: 22 };

/** The site manager: saved FTP, FTPS and SFTP servers, and a form to connect to one. */
export function FilesDialog({ t, onOpen, onCancel }: Props) {
  const [sites, setSites] = useState<FileSite[]>([]);
  const [site, setSite] = useState<FileSite>(blankSite);
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const saved = sites.some((s) => s.id === site.id);

  useEffect(() => {
    void api.filesListSites().then((list) => {
      setSites(list);
      if (list.length > 0) setSite(list[0]);
    });
  }, []);

  const patch = (changes: Partial<FileSite>) => setSite((current) => ({ ...current, ...changes }));

  const save = async () => {
    try {
      setError(null);
      const list = await api.filesSaveSite(
        { ...site, name: site.name.trim() || siteTitle(site) },
        password || null,
      );
      setSites(list);
      setSite(list.find((s) => s.id === site.id) ?? site);
      setPassword("");
      return true;
    } catch (e) {
      setError((e as { message?: string }).message ?? String(e));
      return false;
    }
  };

  const connect = async () => {
    if (!site.host.trim()) return;
    // A saved site stays in sync with the form, so what connects is what is listed.
    if (saved && !(await save())) return;
    onOpen({ kind: "site", site, password: password || null }, siteTitle(site));
  };

  const remove = async () => {
    setSites(await api.filesDeleteSite(site.id));
    setSite(blankSite());
    setPassword("");
  };

  return (
    <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && onCancel()}>
      <div className="dialog files-dialog">
        <h2>{t("files.siteManager")}</h2>
        <div className="files-dialog-body">
          <div className="site-list">
            {sites.map((s) => (
              <button
                key={s.id}
                className={`site-row${s.id === site.id ? " selected" : ""}`}
                onClick={() => {
                  setSite(s);
                  setPassword("");
                  setError(null);
                }}
                onDoubleClick={() => onOpen({ kind: "site", site: s, password: null }, siteTitle(s))}
              >
                <ServerIcon className="icon" />
                <span className="label">{siteTitle(s)}</span>
                <span className="sub">{s.protocol.toUpperCase()}</span>
              </button>
            ))}
            {sites.length === 0 && <div className="hint" style={{ padding: 8 }}>{t("files.noSites")}</div>}
            <div className="spacer" />
            <button
              className="site-new"
              onClick={() => {
                setSite(blankSite());
                setPassword("");
                setError(null);
              }}
            >
              <PlusIcon className="icon" />
              {t("files.newSite")}
            </button>
          </div>

          <div
            className="dialog-body"
            style={{ flex: 1 }}
            onKeyDown={(e) => {
              if (e.key === "Enter") void connect();
              if (e.key === "Escape") onCancel();
            }}
          >
            <div className="field">
              <label>{t("connection.name")}</label>
              <input value={site.name} placeholder={siteTitle(site) || t("files.namePlaceholder")} onChange={(e) => patch({ name: e.target.value })} />
            </div>
            <div className="field">
              <label>{t("files.protocol")}</label>
              <select value={site.protocol} onChange={(e) => patch({ protocol: e.target.value as FileProtocol })}>
                <option value="sftp">SFTP — SSH</option>
                <option value="ftp">FTP</option>
                <option value="ftps">FTPS — FTP over TLS</option>
              </select>
            </div>
            <div className="row">
              <div className="field" style={{ flex: 1 }}>
                <label>{t("connection.host")}</label>
                <input autoFocus value={site.host} placeholder="files.example.com" onChange={(e) => patch({ host: e.target.value })} />
              </div>
              <div className="field" style={{ width: 80 }}>
                <label>{t("connection.port")}</label>
                <input
                  value={site.port || ""}
                  placeholder={String(DEFAULT_PORT[site.protocol])}
                  inputMode="numeric"
                  onChange={(e) => patch({ port: Number.parseInt(e.target.value, 10) || 0 })}
                />
              </div>
            </div>
            <div className="row">
              <div className="field" style={{ flex: 1 }}>
                <label>{t("connection.user")}</label>
                <input
                  value={site.username}
                  placeholder={site.protocol === "sftp" ? "" : "anonymous"}
                  onChange={(e) => patch({ username: e.target.value })}
                />
              </div>
              <div className="field" style={{ flex: 1 }}>
                <label>{t("connection.password")}</label>
                <input
                  type="password"
                  value={password}
                  placeholder={saved && site.savePassword ? t("files.passwordSaved") : ""}
                  onChange={(e) => setPassword(e.target.value)}
                />
              </div>
            </div>
            <label className="check">
              <input type="checkbox" checked={site.savePassword} onChange={(e) => patch({ savePassword: e.target.checked })} />
              {t("connection.savePassword")}
            </label>
            {site.protocol === "sftp" && (
              <div className="field">
                <label>{t("terminal.keyFile")}</label>
                <div className="row">
                  <input value={site.keyPath} placeholder={t("terminal.keyHint")} onChange={(e) => patch({ keyPath: e.target.value })} />
                  <button
                    onClick={async () => {
                      const picked = await open({ multiple: false, directory: false });
                      if (typeof picked === "string") patch({ keyPath: picked });
                    }}
                  >
                    …
                  </button>
                </div>
              </div>
            )}
            <div className="field">
              <label>{t("files.initialPath")}</label>
              <input value={site.initialPath} placeholder={t("files.initialPathHint")} onChange={(e) => patch({ initialPath: e.target.value })} />
            </div>
            {site.protocol !== "sftp" && (
              <label className="check">
                <input type="checkbox" checked={site.activeMode} onChange={(e) => patch({ activeMode: e.target.checked })} />
                {t("files.activeMode")}
              </label>
            )}
            <div className="hint">
              {site.protocol === "sftp"
                ? t("files.sftpHint")
                : site.protocol === "ftps"
                  ? t("files.ftpsHint")
                  : t("files.ftpHint")}
            </div>
            {error && <div className="bad">{error}</div>}
          </div>
        </div>
        <div className="dialog-footer">
          {saved && (
            <button className="danger" onClick={() => void remove()}>
              <TrashIcon className="icon" />
              {t("general.delete")}
            </button>
          )}
          <div className="spacer" />
          <button onClick={onCancel}>{t("general.cancel")}</button>
          <button disabled={!site.host.trim()} onClick={() => void save()}>
            {t("general.save")}
          </button>
          <button className="primary" disabled={!site.host.trim()} onClick={() => void connect()}>
            {t("terminal.connect")}
          </button>
        </div>
      </div>
    </div>
  );
}
