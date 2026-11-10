import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ArrowPathIcon,
  ArrowUpIcon,
  ArrowDownTrayIcon,
  ArrowUpTrayIcon,
  ClipboardDocumentIcon,
  DocumentIcon,
  FolderIcon,
  FolderPlusIcon,
  LinkIcon,
  PencilSquareIcon,
  TrashIcon,
  XMarkIcon,
  ExclamationTriangleIcon,
} from "@heroicons/react/24/outline";
import { listen } from "@tauri-apps/api/event";
import { join } from "@tauri-apps/api/path";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, type FileProgress, type FileTarget, type RemoteEntry } from "../lib/api";
import type { Translate } from "../lib/i18n";
import { ContextMenu, type MenuItem } from "./ContextMenu";
import { ConfirmDialog } from "./ConfirmDialog";

interface Props {
  /** A new id is a new session: the parent changes it to reconnect. */
  sessionId: string;
  target: FileTarget;
  active: boolean;
  onTitle: (title: string) => void;
  onReconnect: () => void;
  t: Translate;
}

interface Transfer {
  id: string;
  direction: "up" | "down";
  label: string;
  done: number;
  total: number;
  file: string;
  error: string | null;
  finished: boolean;
}

type Prompt = { kind: "mkdir" } | { kind: "rename"; entry: RemoteEntry };

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

function parentOf(path: string): string {
  const trimmed = path.replace(/\/+$/, "");
  const index = trimmed.lastIndexOf("/");
  if (index <= 0) return "/";
  return trimmed.slice(0, index);
}

function joinRemote(dir: string, name: string): string {
  return dir.endsWith("/") ? `${dir}${name}` : `${dir}/${name}`;
}

function baseName(path: string): string {
  return path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;
}

function message(error: unknown): string {
  const e = error as { message?: string; detail?: string | null };
  return e.message ? (e.detail ? `${e.message}\n${e.detail}` : e.message) : String(error);
}

const isFolder = (entry: RemoteEntry) => entry.kind === "dir";

export function FilesTab({ sessionId, target, active, onTitle, onReconnect, t }: Props) {
  const [connected, setConnected] = useState(false);
  const [connectError, setConnectError] = useState<string | null>(null);
  const [path, setPath] = useState("");
  const [pathInput, setPathInput] = useState("");
  const [entries, setEntries] = useState<RemoteEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [anchor, setAnchor] = useState<string | null>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [prompt, setPrompt] = useState<Prompt | null>(null);
  const [promptValue, setPromptValue] = useState("");
  const [confirmDelete, setConfirmDelete] = useState<RemoteEntry[] | null>(null);
  const [transfers, setTransfers] = useState<Transfer[]>([]);
  const [dropping, setDropping] = useState(false);
  const pathRef = useRef(path);
  pathRef.current = path;

  const load = useCallback(
    async (next: string) => {
      setLoading(true);
      setError(null);
      try {
        const list = await api.filesList(sessionId, next);
        setEntries(list);
        setPath(next);
        setPathInput(next);
        setSelected(new Set());
        setAnchor(null);
      } catch (e) {
        setError(message(e));
        setPathInput(pathRef.current);
      } finally {
        setLoading(false);
      }
    },
    [sessionId],
  );

  // One session per id; switching tabs only hides the browser.
  useEffect(() => {
    let disposed = false;
    setConnected(false);
    setConnectError(null);
    setEntries([]);
    setLoading(true);
    void api
      .filesOpen(sessionId, target)
      .then(async (opened) => {
        if (disposed) return;
        onTitle(opened.title);
        setConnected(true);
        await load(opened.home);
      })
      .catch((e) => {
        if (!disposed) {
          setConnectError(message(e));
          setLoading(false);
        }
      });
    return () => {
      disposed = true;
      void api.filesClose(sessionId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionId]);

  // Progress for this tab's transfers; others' are ignored by id.
  useEffect(() => {
    const unlisten = listen<FileProgress>("files:progress", (event) => {
      const p = event.payload;
      setTransfers((current) =>
        current.map((tr) => (tr.id === p.transferId ? { ...tr, done: p.done, total: p.total, file: p.file } : tr)),
      );
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  const sorted = useMemo(
    () =>
      [...entries].sort((a, b) => {
        if (isFolder(a) !== isFolder(b)) return isFolder(a) ? -1 : 1;
        return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });
      }),
    [entries],
  );

  const runTransfer = useCallback(
    async (direction: "up" | "down", label: string, work: (id: string) => Promise<unknown>) => {
      const id = crypto.randomUUID();
      setTransfers((current) => [
        ...current,
        { id, direction, label, done: 0, total: 0, file: "", error: null, finished: false },
      ]);
      try {
        await work(id);
        setTransfers((current) => current.map((tr) => (tr.id === id ? { ...tr, finished: true } : tr)));
        // A finished transfer lingers a moment so it is seen to have finished.
        setTimeout(() => setTransfers((current) => current.filter((tr) => tr.id !== id)), 2500);
        return true;
      } catch (e) {
        const cancelled = (e as { code?: string }).code === "cancelled";
        setTransfers((current) =>
          cancelled
            ? current.filter((tr) => tr.id !== id)
            : current.map((tr) => (tr.id === id ? { ...tr, error: message(e), finished: true } : tr)),
        );
        return false;
      }
    },
    [],
  );

  const upload = useCallback(
    async (localPaths: string[]) => {
      const dir = pathRef.current;
      await Promise.all(
        localPaths.map((local) =>
          runTransfer("up", baseName(local), (id) => api.filesUpload(sessionId, id, local, dir)),
        ),
      );
      if (pathRef.current === dir) void load(dir);
    },
    [load, runTransfer, sessionId],
  );

  const download = useCallback(
    async (items: RemoteEntry[]) => {
      if (items.length === 0) return;
      if (items.length === 1 && !isFolder(items[0])) {
        const target = await save({ defaultPath: items[0].name });
        if (!target) return;
        void runTransfer("down", items[0].name, (id) =>
          api.filesDownload(sessionId, id, items[0].path, false, target),
        );
        return;
      }
      const folder = await open({ directory: true, multiple: false, title: t("files.downloadTo") });
      if (typeof folder !== "string") return;
      for (const item of items) {
        const local = await join(folder, item.name);
        void runTransfer("down", item.name, (id) =>
          api.filesDownload(sessionId, id, item.path, isFolder(item), local),
        );
      }
    },
    [runTransfer, sessionId, t],
  );

  // Files dropped from Finder or Explorer go into the folder on screen.
  useEffect(() => {
    if (!active || !connected) return;
    const stop = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type === "enter" || event.payload.type === "over") setDropping(true);
      else if (event.payload.type === "leave") setDropping(false);
      else if (event.payload.type === "drop") {
        setDropping(false);
        if (event.payload.paths.length > 0) void upload(event.payload.paths);
      }
    });
    return () => {
      setDropping(false);
      void stop.then((unlisten) => unlisten());
    };
  }, [active, connected, upload]);

  const selectedEntries = sorted.filter((e) => selected.has(e.path));

  const openEntry = async (entry: RemoteEntry) => {
    if (isFolder(entry)) return load(entry.path);
    if (entry.kind === "link") {
      // FTP does not say where a link points; trying it as a folder is how to find out.
      try {
        await api.filesList(sessionId, entry.path);
        return load(entry.path);
      } catch {
        /* a link to a file */
      }
    }
    return download([entry]);
  };

  const select = (entry: RemoteEntry, event: React.MouseEvent) => {
    if (event.shiftKey && anchor) {
      const from = sorted.findIndex((e) => e.path === anchor);
      const to = sorted.findIndex((e) => e.path === entry.path);
      const [lo, hi] = from < to ? [from, to] : [to, from];
      setSelected(new Set(sorted.slice(lo, hi + 1).map((e) => e.path)));
      return;
    }
    if (event.metaKey || event.ctrlKey) {
      setSelected((current) => {
        const next = new Set(current);
        if (next.has(entry.path)) next.delete(entry.path);
        else next.add(entry.path);
        return next;
      });
    } else {
      setSelected(new Set([entry.path]));
    }
    setAnchor(entry.path);
  };

  const entryMenu = (entry: RemoteEntry, x: number, y: number) => {
    const targets = selected.has(entry.path) ? selectedEntries : [entry];
    if (!selected.has(entry.path)) {
      setSelected(new Set([entry.path]));
      setAnchor(entry.path);
    }
    const items: MenuItem[] = [
      { label: t("files.download"), icon: ArrowDownTrayIcon, onSelect: () => void download(targets) },
      {
        label: t("files.rename"),
        icon: PencilSquareIcon,
        disabled: targets.length !== 1,
        onSelect: () => {
          setPromptValue(entry.name);
          setPrompt({ kind: "rename", entry });
        },
      },
      {
        label: t("files.copyPath"),
        icon: ClipboardDocumentIcon,
        onSelect: () => void writeText(targets.map((e) => e.path).join("\n")),
      },
      {
        label: t("general.delete"),
        icon: TrashIcon,
        danger: true,
        separatorBefore: true,
        onSelect: () => setConfirmDelete(targets),
      },
    ];
    setMenu({ x, y, items });
  };

  const submitPrompt = async () => {
    const name = promptValue.trim();
    if (!prompt || !name || name.includes("/")) return;
    const current = prompt;
    setPrompt(null);
    try {
      if (current.kind === "mkdir") await api.filesMkdir(sessionId, joinRemote(path, name));
      else if (name !== current.entry.name)
        await api.filesRename(sessionId, current.entry.path, joinRemote(parentOf(current.entry.path), name));
      await load(path);
    } catch (e) {
      setError(message(e));
    }
  };

  const deleteEntries = async (items: RemoteEntry[]) => {
    setConfirmDelete(null);
    setLoading(true);
    try {
      for (const item of items) await api.filesDelete(sessionId, item.path, isFolder(item) && !item.link);
    } catch (e) {
      setError(message(e));
    }
    await load(path);
  };

  const onKeyDown = (event: React.KeyboardEvent) => {
    if ((event.target as HTMLElement).tagName === "INPUT") return;
    if (event.key === "Backspace" && (event.metaKey || selectedEntries.length === 0)) {
      if (event.metaKey && selectedEntries.length > 0) setConfirmDelete(selectedEntries);
      else if (path !== "/") void load(parentOf(path));
      event.preventDefault();
    } else if (event.key === "Delete" && selectedEntries.length > 0) {
      setConfirmDelete(selectedEntries);
    } else if (event.key === "Enter" && selectedEntries.length === 1) {
      void openEntry(selectedEntries[0]);
    } else if (event.key === "a" && (event.metaKey || event.ctrlKey)) {
      setSelected(new Set(sorted.map((e) => e.path)));
      event.preventDefault();
    }
  };

  if (connectError) {
    return (
      <div className="files-host" style={{ display: active ? "flex" : "none" }}>
        <div className="empty">
          <ExclamationTriangleIcon className="icon-xl" />
          <div className="bad" style={{ fontSize: 13, textAlign: "center", maxWidth: 520 }}>{connectError}</div>
          <button className="primary" onClick={onReconnect}>
            <ArrowPathIcon className="icon" />
            {t("terminal.reconnect")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="files-host" style={{ display: active ? "flex" : "none" }} tabIndex={-1} onKeyDown={onKeyDown}>
      <div className="query-toolbar">
        <button title={t("files.up")} disabled={!connected || path === "/"} onClick={() => void load(parentOf(path))}>
          <ArrowUpIcon className="icon" />
        </button>
        <input
          className="files-path"
          value={pathInput}
          disabled={!connected}
          spellCheck={false}
          onChange={(e) => setPathInput(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void load(pathInput.trim() || "/");
            if (e.key === "Escape") setPathInput(path);
          }}
        />
        <button title={t("general.refresh")} disabled={!connected} onClick={() => void load(path)}>
          <ArrowPathIcon className={`icon${loading ? " spin" : ""}`} />
        </button>
        <div className="chrome-divider" style={{ height: 18 }} />
        <button
          disabled={!connected}
          onClick={() => {
            setPromptValue("");
            setPrompt({ kind: "mkdir" });
          }}
        >
          <FolderPlusIcon className="icon" />
          {t("files.newFolder")}
        </button>
        <button
          disabled={!connected}
          onClick={async () => {
            const picked = await open({ multiple: true, directory: false, title: t("files.upload") });
            if (Array.isArray(picked) && picked.length > 0) void upload(picked);
            else if (typeof picked === "string") void upload([picked]);
          }}
        >
          <ArrowUpTrayIcon className="icon" />
          {t("files.upload")}
        </button>
        <button disabled={!connected || selectedEntries.length === 0} onClick={() => void download(selectedEntries)}>
          <ArrowDownTrayIcon className="icon" />
          {t("files.download")}
        </button>
      </div>

      {error && (
        <div className="error-banner">
          <ExclamationTriangleIcon className="icon" />
          <div style={{ flex: 1, whiteSpace: "pre-wrap" }}>{error}</div>
          <span className="close" onClick={() => setError(null)}>
            <XMarkIcon className="icon" />
          </span>
        </div>
      )}

      <div
        className={`files-list${dropping ? " dropping" : ""}`}
        onMouseDown={(e) => {
          if (e.target === e.currentTarget) setSelected(new Set());
        }}
      >
        <div className="files-row files-head">
          <span className="name">{t("files.name")}</span>
          <span className="size">{t("files.size")}</span>
          <span className="modified">{t("files.modified")}</span>
          <span className="perm">{t("files.permissions")}</span>
        </div>
        {sorted.map((entry) => (
          <div
            key={entry.path}
            className={`files-row${selected.has(entry.path) ? " selected" : ""}`}
            onMouseDown={(e) => e.button === 0 && select(entry, e)}
            onDoubleClick={() => void openEntry(entry)}
            onContextMenu={(e) => {
              e.preventDefault();
              entryMenu(entry, e.clientX, e.clientY);
            }}
          >
            <span className="name">
              {entry.kind === "dir" ? (
                <FolderIcon className="icon folder" />
              ) : entry.kind === "link" ? (
                <LinkIcon className="icon" />
              ) : (
                <DocumentIcon className="icon" />
              )}
              <span className="label">{entry.name}</span>
              {entry.link && entry.kind !== "link" && <LinkIcon className="icon faint" />}
            </span>
            <span className="size">{entry.kind === "dir" ? "—" : formatSize(entry.size)}</span>
            <span className="modified">{entry.modified ? new Date(entry.modified).toLocaleString() : ""}</span>
            <span className="perm">{entry.permissions ?? ""}</span>
          </div>
        ))}
        {connected && !loading && sorted.length === 0 && !error && (
          <div className="hint" style={{ padding: 16, textAlign: "center" }}>{t("files.emptyFolder")}</div>
        )}
        {!connected && <div className="hint" style={{ padding: 16, textAlign: "center" }}>{t("files.connecting")}</div>}
        {dropping && <div className="files-drop">{t("files.dropHere", path)}</div>}
      </div>

      {transfers.length > 0 && (
        <div className="transfers">
          {transfers.map((tr) => {
            const pct = tr.total > 0 ? Math.min(100, (tr.done / tr.total) * 100) : tr.finished ? 100 : 0;
            return (
              <div key={tr.id} className="transfer">
                {tr.direction === "up" ? <ArrowUpTrayIcon className="icon" /> : <ArrowDownTrayIcon className="icon" />}
                <span className="label" title={tr.file}>
                  {tr.label}
                  {tr.file && tr.file !== tr.label ? ` — ${tr.file}` : ""}
                </span>
                {tr.error ? (
                  <span className="bad" title={tr.error}>{tr.error.split("\n")[0]}</span>
                ) : (
                  <>
                    <div className="progress">
                      <div style={{ width: `${pct}%` }} />
                    </div>
                    <span className="sub">
                      {formatSize(tr.done)}
                      {tr.total > 0 ? ` / ${formatSize(tr.total)}` : ""}
                    </span>
                  </>
                )}
                <span
                  className="close"
                  title={tr.finished ? "" : t("general.cancel")}
                  onClick={() =>
                    tr.finished
                      ? setTransfers((current) => current.filter((x) => x.id !== tr.id))
                      : void api.filesCancel(tr.id)
                  }
                >
                  <XMarkIcon className="icon" />
                </span>
              </div>
            );
          })}
        </div>
      )}

      {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={() => setMenu(null)} />}

      {prompt && (
        <div className="scrim" onMouseDown={(e) => e.target === e.currentTarget && setPrompt(null)}>
          <div className="dialog" style={{ width: 360 }}>
            <h2>{prompt.kind === "mkdir" ? t("files.newFolder") : t("files.rename")}</h2>
            <div className="dialog-body">
              <input
                autoFocus
                value={promptValue}
                onFocus={(e) => {
                  // Select the name without its extension, as Finder does.
                  const dot = e.target.value.lastIndexOf(".");
                  e.target.setSelectionRange(0, dot > 0 ? dot : e.target.value.length);
                }}
                onChange={(e) => setPromptValue(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") void submitPrompt();
                  if (e.key === "Escape") setPrompt(null);
                }}
              />
            </div>
            <div className="dialog-footer">
              <div className="spacer" />
              <button onClick={() => setPrompt(null)}>{t("general.cancel")}</button>
              <button
                className="primary"
                disabled={!promptValue.trim() || promptValue.includes("/")}
                onClick={() => void submitPrompt()}
              >
                {prompt.kind === "mkdir" ? t("files.create") : t("files.rename")}
              </button>
            </div>
          </div>
        </div>
      )}

      {confirmDelete && (
        <ConfirmDialog
          title={t("files.deleteTitle")}
          message={
            confirmDelete.length === 1
              ? t(isFolder(confirmDelete[0]) ? "files.deleteFolderBody" : "files.deleteFileBody", confirmDelete[0].name)
              : t("files.deleteManyBody", confirmDelete.length)
          }
          confirmLabel={t("general.delete")}
          destructive
          onConfirm={() => void deleteEntries(confirmDelete)}
          onCancel={() => setConfirmDelete(null)}
        />
      )}
    </div>
  );
}
