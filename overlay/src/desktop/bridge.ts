/**
 * Bridge to the Tauri shell.
 *
 * The overlay runs inside the *same* page as the upstream frontend, so it can talk to the
 * API through the loopback origin exactly like the app does — same `/api` prefix, same
 * `token` cookie. Only the desktop-only capabilities go over IPC.
 *
 * Everything here degrades to a no-op when the bundle is served in a normal browser, so
 * the same build still works as a plain web app.
 */

interface TauriGlobal {
  core: { invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> };
  event: {
    listen<T>(
      name: string,
      handler: (event: { payload: T }) => void,
    ): Promise<() => void>;
  };
}

function tauri(): TauriGlobal | null {
  const candidate = (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
  return candidate ?? null;
}

export function isDesktop(): boolean {
  return tauri() !== null;
}

export async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const shell = tauri();
  if (!shell) throw new Error('不在桌面客户端中运行');
  return shell.core.invoke<T>(command, args);
}

/**
 * Turn whatever Tauri rejected with into something a person can act on.
 *
 * A denied IPC call surfaces as an opaque ACL error string. Reporting it verbatim beats
 * the alternative — a button that appears to do nothing.
 */
export function describeError(error: unknown): string {
  const text = String((error as Error)?.message ?? error);
  if (/not allowed|denied|capability|forbidden/i.test(text)) {
    return `桌面功能被拒绝（IPC 权限）：${text}`;
  }
  return text;
}

export async function listen<T>(
  eventName: string,
  handler: (payload: T) => void,
): Promise<() => void> {
  const shell = tauri();
  if (!shell) return () => {};
  return shell.event.listen<T>(eventName, (event) => handler(event.payload));
}

/** The frontend stores the session token in a readable cookie (`utils/cookie.ts`). */
export function sessionToken(): string {
  const match = document.cookie.match(/(?:^|;\s*)token=([^;]*)/);
  return match ? decodeURIComponent(match[1]) : '';
}

async function apiGet<T>(path: string): Promise<T> {
  const response = await fetch(`/api/${path.replace(/^\/+/, '')}`, {
    headers: { Authorization: `Bearer ${sessionToken()}` },
  });
  if (!response.ok) {
    throw new Error(`请求 ${path} 失败：HTTP ${response.status}`);
  }
  return (await response.json()) as T;
}

export interface ApiLanguage {
  code: string;
  i18n_name?: string;
  i18nName?: string;
}

export interface ApiTarget {
  id: string;
  language: ApiLanguage;
}

export interface ApiProjectSummary {
  id: string;
  name: string;
}

export interface Profile {
  id: string;
  name: string;
  /** Human-facing site URL, for display and for opening in a browser. */
  site_url: string;
  /** Where `/api/<rest>` is forwarded. */
  api_base: string;
  /** Loopback port for this profile — distinct per instance, so logins stay separate. */
  port: number;
  allow_invalid_certs: boolean;
  media_origins: string[];
}

export interface CacheStats {
  entries: number;
  bytes: number;
  limit_bytes: number;
}

export interface ProxySettings {
  mode: 'direct' | 'system' | 'manual';
  url: string;
}

export interface BootPayload {
  version: string;
  proxy: ProxySettings;
  profiles: Profile[];
  active: string;
  active_port: number;
  cache: CacheStats;
}

export interface ProbeResult {
  ok: boolean;
  api_base: string | null;
  matched: string | null;
  message: string;
  tried: string[];
}

export const shell = {
  boot: () => invoke<BootPayload>('boot_payload'),
  probe: (input: string) => invoke<ProbeResult>('probe_server', { input }),
  setActive: (id: string) => invoke<number>('set_active_profile', { id }),
  /** Open the instance picker window — the app's landing page. */
  openLauncher: () => invoke<void>('open_launcher'),
  openSettings: () => invoke<void>('open_settings'),
  openExternal: (url: string) => invoke<void>('open_external', { url }),
};

export interface ExportReport {
  path: string;
  file_count: number;
  image_count: number;
  skipped_images: string[];
  warnings: string[];
  diverged: boolean;
}

export interface ExportProgress {
  stage: string;
  progress: number;
}

export const api = {
  listProjects: () => apiGet<ApiProjectSummary[]>('v1/user/projects'),
  /**
   * A project's translation targets.
   *
   * These are **not** on the project detail — that carries only `target_count`, so reading
   * `detail.targets` yields undefined and the language list comes up empty. They live at
   * `/v1/projects/{id}/targets`, unpaginated in practice: the upstream list component asks
   * for `limit: 100000` and reads the count from the `x-pagination-count` header.
   */
  projectTargets: (id: string) =>
    apiGet<ApiTarget[] | { list?: ApiTarget[] }>(`v1/projects/${id}/targets?limit=100000`),
};

/** Tolerate either a bare array or a wrapped list; the endpoint has been both. */
export function asTargetList(payload: ApiTarget[] | { list?: ApiTarget[] } | null | undefined): ApiTarget[] {
  if (Array.isArray(payload)) return payload;
  return Array.isArray(payload?.list) ? payload.list : [];
}

/** Pull the project id out of `/projects/<uuid>` so the dialog can prefill. */
export function currentProjectId(): string | null {
  const match = window.location.pathname.match(/\/projects\/([0-9a-fA-F-]{36})/);
  return match ? match[1] : null;
}

export function targetLabel(target: ApiTarget): string {
  return target.language?.i18n_name ?? target.language?.i18nName ?? target.language?.code ?? target.id;
}
