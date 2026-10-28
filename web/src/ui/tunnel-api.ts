/* The gate (Шлюз): the tunnel to the outside, its devices and its settings, and the calls the plaque makes.
   The calls mirror src/tunnel.rs and src/devices.rs. Only this machine may ask: from anywhere else the server says
   no, and then the plaque is not shown at all. */
import { signal } from '@preact/signals';
import { api, ApiError, onTunnelEvent, post } from '../api';
import { t } from '../i18n';
import { toast } from '../toast';

export type TunnelState = 'closed' | 'starting' | 'open' | 'reconnecting' | 'closing' | 'failed' | 'external';
export type TunnelMode = 'quick' | 'named';

export interface TunnelProblem { kind: string; message: string; log_tail: string[] }

/** One line of «Check» for a named tunnel's config; `id` picks the words, `have` and `want` fill them. */
export interface TunnelCheck { id: string; ok: boolean; warn: boolean; have: string | null; want: string | null }

export interface TunnelConfigCheck {
  ok: boolean;
  problem: TunnelProblem | null;
  hosts: string[];
  host: string | null;
  checks: TunnelCheck[];
}

export interface TunnelDevice {
  id: string;
  label: string;
  scope: 'lite' | 'full';
  state: 'pending' | 'active';
  country: string | null;
  touch: boolean;
  since_ms: number;
  last_ms: number;
  ends_ms: number | null;
  answer_by_ms: number | null;
}

export interface TunnelSettings { full_allowed: boolean; confirm_devices: boolean; idle_close_h: number }

export interface TunnelView {
  installed: boolean;
  bin: string | null;
  version: string | null;
  mode: TunnelMode;
  config: string | null;
  hostname: string | null;
  config_check: TunnelConfigCheck | null;
  state: TunnelState;
  since_ms: number;
  host: string | null;
  link: string | null;
  link_state: 'fresh' | 'used' | 'expired' | 'none';
  link_expires_ms: number | null;
  settings: TunnelSettings;
  password_set: boolean;
  sessions: TunnelDevice[];
  idle_closes_ms: number | null;
  error: TunnelProblem | null;
  external_pid: number | null;
  sse: boolean;
  qr_svg: string | null;
  port: number;
  home: string;
}

/** What the server says; null while it has not answered, or when this page may not ask (another computer, a gate session). */
export const tunnelView = signal<TunnelView | null>(null);
/** The plaque's card is open. */
export const tunnelCard = signal(false);
/** The step-by-step guide: which of the two ways it opens on. */
export const tunnelGuide = signal<TunnelMode | null>(null);
/** What is typed in the fields of a named tunnel (null: the field shows what the server has). The guide reads them to fill its commands. */
export const draftConfig = signal<string | null>(null);
export const draftHost = signal<string | null>(null);
/** The tunnel's id typed in the guide (null: the one in the config, if it has one). */
export const draftUuid = signal<string | null>(null);
/** The answer of the last «Check», and whether it is out of date because a field was typed in after it. */
export const checked = signal<{ at: number } | null>(null);

export const shownConfig = (): string => draftConfig.value ?? tunnelView.value?.config ?? '';
export const shownHost = (): string => draftHost.value ?? tunnelView.value?.hostname ?? '';

export function openTunnelGuide(mode?: TunnelMode): void {
  tunnelCard.value = false;
  tunnelGuide.value = mode ?? tunnelView.value?.mode ?? 'quick';
}

export function closeTunnelCard(): void {
  tunnelCard.value = false;
  draftConfig.value = null;
  draftHost.value = null;
  checked.value = null;
}

let loading = false;
let again = false;

/** Asks the server for the gate as it is now. A server that refuses (this is not the owner's machine) hides the plaque. */
export async function loadTunnel(): Promise<void> {
  if (loading) { again = true; return; }
  loading = true;
  try {
    tunnelView.value = await api<TunnelView>('/api/tunnel');
  } catch (e) {
    if (e instanceof ApiError && (e.status === 403 || e.status === 404 || e.status === 421)) tunnelView.value = null;
  } finally {
    loading = false;
    if (again) { again = false; void loadTunnel(); }
  }
}

onTunnelEvent(() => { void loadTunnel(); });

/** A POST of the gate; the answer is the gate as it is after it. Returns whether it worked. */
async function act(path: string, body?: unknown): Promise<boolean> {
  try {
    tunnelView.value = await post<TunnelView>(path, body);
    return true;
  } catch (e) {
    toast(t('tun.failed', { why: (e as Error).message }), { icon: 'failed' });
    void loadTunnel();
    return false;
  }
}

export const openGate = (): Promise<boolean> => act('/api/tunnel/open');
export const closeGate = (): Promise<boolean> => act('/api/tunnel/close');
export const rotateLink = (): Promise<boolean> => act('/api/tunnel/rotate');
export const decideDevice = (id: string, op: 'allow' | 'deny' | 'revoke'): Promise<boolean> => act('/api/tunnel/session', { id, op });
export const setGateSettings = (patch: Partial<TunnelSettings>): Promise<boolean> => act('/api/tunnel/settings', patch);

/** Saves the mode and (for a named tunnel) the config and the address; what is not given stays. */
export async function saveGateConfig(patch: { mode?: TunnelMode; config?: string; hostname?: string }): Promise<boolean> {
  const ok = await act('/api/tunnel/config', patch);
  if (ok) {
    if (patch.config !== undefined) draftConfig.value = null;
    if (patch.hostname !== undefined) draftHost.value = null;
  }
  return ok;
}

/** The fields typed in and not saved yet, as a patch for the server. */
export function draftPatch(): { config?: string; hostname?: string } {
  const v = tunnelView.value;
  const patch: { config?: string; hostname?: string } = {};
  if (draftConfig.value !== null && draftConfig.value.trim() !== (v?.config ?? '')) patch.config = draftConfig.value.trim();
  if (draftHost.value !== null && draftHost.value.trim() !== (v?.hostname ?? '')) patch.hostname = draftHost.value.trim();
  return patch;
}

/** Saves what is typed in the fields (when it differs from what the server has). */
export async function saveDrafts(): Promise<boolean> {
  const patch = draftPatch();
  return Object.keys(patch).length === 0 || saveGateConfig(patch);
}

/** «Check»: the fields are saved, then the server reads the config again. */
export async function checkGate(): Promise<boolean> {
  if (!(await saveDrafts())) return false;
  await loadTunnel();
  checked.value = { at: Date.now() };
  return true;
}

/** The words for a line of «Check». */
export function checkText(c: TunnelCheck): string {
  const dash = '—';
  const vars = { have: c.have ?? dash, want: c.want ?? dash };
  if (c.id === 'hostname' && !c.ok && !c.want) return t('tun.chk.hostname.none');
  if (c.id === 'credentials' && !c.ok && !c.have) return t('tun.chk.credentials.none');
  if (c.id === 'service' && !c.ok && !c.have) return t('tun.chk.service.none', vars);
  return t(`tun.chk.${c.id}.${c.ok ? 'ok' : 'bad'}`, vars);
}
