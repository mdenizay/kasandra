// Typed wrappers over the Tauri command surface. Every call the UI makes goes through
// here, so the invoke names live in one place.

import { invoke } from "@tauri-apps/api/core";

/** What a terminal tab connects to; mirrors `terminal::TerminalTarget`. */
export type TerminalTarget =
  | { kind: "local" }
  | { kind: "profile"; profileId: string }
  | { kind: "host"; host: string; port: number | null; username: string; keyPath: string };
export type FileProtocol = "ftp" | "ftps" | "sftp";

/** A saved server in the file site manager; mirrors `files::FileSite`. */
export interface FileSite {
  id: string;
  name: string;
  protocol: FileProtocol;
  host: string;
  /** 0 means the protocol's usual port. */
  port: number;
  username: string;
  keyPath: string;
  initialPath: string;
  savePassword: boolean;
  activeMode: boolean;
}

/** What a file browser tab connects to; mirrors `files::FileTarget`. */
export type FileTarget =
  | { kind: "site"; site: FileSite; password: string | null }
  | { kind: "profile"; profileId: string };

export interface RemoteEntry {
  name: string;
  path: string;
  kind: "dir" | "file" | "link";
  link: boolean;
  size: number;
  modified: number | null;
  permissions: string | null;
}

export interface OpenedFileSession {
  home: string;
  title: string;
  protocol: FileProtocol;
}

export interface FileProgress {
  transferId: string;
  done: number;
  total: number;
  file: string;
}

import type {
  AppSettings,
  ColumnInfo,
  ConnectionProfile,
  CsvOptions,
  CsvPreview,
  DumpOptions,
  DumpSummary,
  ExportFormat,
  HistoryEntry,
  PlannedStatement,
  QueryResult,
  ResultRow,
  RowEdit,
  ScriptSummary,
  ServerInfo,
  Snippet,
  TableDetails,
  TableRef,
} from "./types";

export const api = {
  listProfiles: () => invoke<ConnectionProfile[]>("list_profiles"),
  saveProfile: (profile: ConnectionProfile, password?: string, sshPassword?: string) =>
    invoke<ConnectionProfile[]>("save_profile", {
      profile,
      password: password ?? null,
      sshPassword: sshPassword ?? null,
    }),
  deleteProfile: (id: string) => invoke<ConnectionProfile[]>("delete_profile", { id }),

  parseConnectionUrl: (url: string) =>
    invoke<{ profile: ConnectionProfile; password: string | null; warnings: string[] }>(
      "parse_connection_url",
      { url },
    ),
  looksLikeConnectionUrl: (text: string) =>
    invoke<boolean>("looks_like_connection_url", { text }),
  connectionUrlForProfile: (profile: ConnectionProfile) =>
    invoke<string>("connection_url_for_profile", { profile }),

  connect: (profile: ConnectionProfile, password?: string, sshPassword?: string) =>
    invoke<ServerInfo>("connect", {
      profile,
      password: password ?? null,
      sshPassword: sshPassword ?? null,
    }),
  testConnection: (profile: ConnectionProfile, password?: string, sshPassword?: string) =>
    invoke<ServerInfo>("test_connection", {
      profile,
      password: password ?? null,
      sshPassword: sshPassword ?? null,
    }),
  disconnect: (id: string) => invoke<void>("disconnect", { id }),

  executeSql: (id: string, sql: string, maxRows: number) =>
    invoke<QueryResult[]>("execute_sql", { id, sql, maxRows }),
  cancelQuery: (id: string) => invoke<void>("cancel_query", { id }),
  useDatabase: (id: string, database: string) => invoke<void>("use_database", { id, database }),
  listDatabases: (id: string) => invoke<string[]>("list_databases", { id }),
  listSchemas: (id: string, database: string) =>
    invoke<string[]>("list_schemas", { id, database }),
  listTables: (id: string, database: string, schema: string) =>
    invoke<TableRef[]>("list_tables", { id, database, schema }),
  schemaColumns: (id: string, database: string, schema: string) =>
    invoke<Record<string, string[]>>("schema_columns", { id, database, schema }),
  describeTable: (id: string, table: TableRef) =>
    invoke<TableDetails>("describe_table", { id, table }),
  createStatement: (id: string, table: TableRef) =>
    invoke<string>("create_statement", { id, table }),
  fetchRows: (
    id: string,
    table: TableRef,
    whereClause: string,
    orderBy: [string, boolean][],
    limit: number,
    offset: number,
  ) => invoke<QueryResult>("fetch_rows", { id, args: { table, whereClause, orderBy, limit, offset } }),
  countRows: (id: string, table: TableRef, whereClause: string) =>
    invoke<number>("count_rows", { id, table, whereClause }),

  getSettings: () => invoke<AppSettings>("get_settings"),
  setSettings: (settings: AppSettings) => invoke<void>("set_settings", { settings }),
  getHistory: () => invoke<HistoryEntry[]>("get_history"),
  addHistory: (entry: HistoryEntry, limit: number) =>
    invoke<HistoryEntry[]>("add_history", { entry, limit }),
  clearHistory: () => invoke<void>("clear_history"),
  dataDirectory: () => invoke<string>("data_directory"),
  credentialStoreAvailable: () => invoke<boolean>("credential_store_available"),

  getSnippets: () => invoke<Snippet[]>("get_snippets"),
  setSnippets: (snippets: Snippet[]) => invoke<void>("set_snippets", { snippets }),

  planRowEdits: (id: string, table: TableRef, edits: RowEdit[]) =>
    invoke<PlannedStatement[]>("plan_row_edits", { id, table, edits }),
  applyStatements: (id: string, statements: string[]) =>
    invoke<number>("apply_statements", { id, statements }),

  exportRows: (
    columns: ColumnInfo[],
    rows: ResultRow[],
    format: ExportFormat,
    tableName: string,
    kind: string,
    includeHeader = true,
  ) =>
    invoke<string>("export_rows", {
      columns,
      rows,
      options: { format, includeHeader, nullPlaceholder: "", tableName, kind },
    }),
  writeTextFile: (path: string, contents: string) =>
    invoke<void>("write_text_file", { path, contents }),

  dumpDatabase: (id: string, tables: TableRef[], options: DumpOptions, path: string) =>
    invoke<DumpSummary>("dump_database", { id, tables, options, path }),
  runScriptFile: (id: string, path: string, stopOnError: boolean) =>
    invoke<ScriptSummary>("run_script_file", { id, path, stopOnError }),
  csvPreview: (path: string, options: CsvOptions) =>
    invoke<CsvPreview>("csv_preview", { path, options }),
  csvImport: (id: string, table: TableRef, path: string, options: CsvOptions) =>
    invoke<number>("csv_import", { id, table, path, options }),

  terminalOpen: (id: string, target: TerminalTarget, cols: number, rows: number) =>
    invoke<string>("terminal_open", { id, target, cols, rows }),
  terminalWrite: (id: string, data: string) => invoke<void>("terminal_write", { id, data }),
  terminalResize: (id: string, cols: number, rows: number) =>
    invoke<void>("terminal_resize", { id, cols, rows }),
  terminalClose: (id: string) => invoke<void>("terminal_close", { id }),

  filesListSites: () => invoke<FileSite[]>("files_list_sites"),
  filesSaveSite: (site: FileSite, password: string | null) =>
    invoke<FileSite[]>("files_save_site", { site, password }),
  filesDeleteSite: (id: string) => invoke<FileSite[]>("files_delete_site", { id }),
  filesOpen: (id: string, target: FileTarget) => invoke<OpenedFileSession>("files_open", { id, target }),
  filesList: (id: string, path: string) => invoke<RemoteEntry[]>("files_list", { id, path }),
  filesMkdir: (id: string, path: string) => invoke<void>("files_mkdir", { id, path }),
  filesRename: (id: string, from: string, to: string) => invoke<void>("files_rename", { id, from, to }),
  filesDelete: (id: string, path: string, isDir: boolean) => invoke<void>("files_delete", { id, path, isDir }),
  filesDownload: (id: string, transferId: string, remotePath: string, isDir: boolean, localPath: string) =>
    invoke<void>("files_download", { id, transferId, remotePath, isDir, localPath }),
  filesUpload: (id: string, transferId: string, localPath: string, remoteDir: string) =>
    invoke<string>("files_upload", { id, transferId, localPath, remoteDir }),
  filesCancel: (transferId: string) => invoke<void>("files_cancel", { transferId }),
  filesClose: (id: string) => invoke<void>("files_close", { id }),
};
