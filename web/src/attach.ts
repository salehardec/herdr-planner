/* Files for the Coordinator's chat and for the todo list: text, images and sound.
   The browser sends the bytes over HTTP(S) to the server, which decides by the bytes what a file is
   (the browser's own type is only a first look), keeps it in the planner's folder and gives back an id. */
import { signal, type Signal } from '@preact/signals';
import { toast } from './toast';
import { t } from './i18n';

export type FileClass = 'image' | 'audio' | 'text';

export const FILE_MAX_COUNT = 8;
const LIMITS: Record<FileClass, number> = { image: 10 * 1024 * 1024, audio: 25 * 1024 * 1024, text: 2 * 1024 * 1024 };
const IMAGE = /^image\/(png|jpeg|webp|gif)$/;
const TEXT_EXT = /\.(txt|md|markdown|log|json|ya?ml|toml|csv|tsv|xml|ini|conf|cfg|rs|ts|tsx|js|jsx|mjs|py|go|sh|c|h|cpp|hpp|java|kt|rb|php|sql|css|html?|diff|patch)$/i;
const AUDIO_EXT = /\.(mp3|wav|ogg|oga|flac|m4a|aac)$/i;

/** What the browser says a file is (by its type, then its name); `null`: the server will not take it. */
export function classOf(file: { name: string; type: string }): FileClass | null {
  const type = String(file.type || '').toLowerCase();
  if (IMAGE.test(type)) return 'image';
  if (type.startsWith('image/')) return null;
  if (type.startsWith('audio/')) return 'audio';
  if (type.startsWith('text/') || /^application\/(json|xml|x-yaml|yaml|toml|x-sh|javascript|x-ndjson)/.test(type)) return 'text';
  if (type && type !== 'application/octet-stream') return null;
  if (AUDIO_EXT.test(file.name)) return 'audio';
  if (TEXT_EXT.test(file.name)) return 'text';
  return null;
}

export interface PendingFile {
  key: number;
  name: string;
  size: number;
  cls: FileClass;
  state: 'uploading' | 'ready' | 'error';
  /** An address the page can show right now: an image's thumbnail, a sound's player. */
  local: string;
  /** The beginning of a text file, for the preview. */
  excerpt: string;
  id: string;
  error: string;
  task: Promise<void> | null;
}

/** A saved file as the server reports it. */
/** `url`: where the bytes are when not in the planner's attachments folder (an image out of the Coordinator's transcript). */
export interface SavedFile { id: string; name: string; mime?: string; size?: number; url?: string }

let seq = 0;

export const classOfSaved = (a: { mime?: string; id: string }): FileClass => {
  const mime = a.mime ?? '';
  if (mime.startsWith('image/') || /\.(png|jpg|gif|webp)$/.test(a.id)) return 'image';
  if (mime.startsWith('audio/') || /\.(mp3|wav|ogg|flac|m4a|aac)$/.test(a.id)) return 'audio';
  return 'text';
};

async function upload(item: PendingFile, file: File, onChange: () => void): Promise<void> {
  const form = new FormData();
  form.append('file', file, item.name);
  try {
    const r = await fetch('/api/chat/attach', { method: 'POST', credentials: 'same-origin', body: form });
    if (r.status === 401) { location.href = '/login?next=' + encodeURIComponent(location.pathname + location.search); throw new Error('unauthorized'); }
    const data = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(data.error || `HTTP ${r.status}`);
    item.id = data.attachment?.id || '';
    item.name = data.attachment?.name || item.name;
    if (!item.id) throw new Error(t('chat.noFile'));
    item.state = 'ready';
  } catch (e) {
    item.state = 'error';
    item.error = (e as Error).message;
    toast(t('chat.imgFailed', { why: item.error }));
  }
  onChange();
}

/** The files of a shelf (the chat's line, the todo composer): checks, previews, uploads. */
export function makeShelf(): {
  files: Signal<PendingFile[]>;
  add: (list: FileList | File[] | null | undefined) => void;
  remove: (key: number) => void;
  clear: () => void;
  /** Waits for the uploads; the saved files, or the first error's text. */
  settled: () => Promise<{ ok: true; saved: SavedFile[] } | { ok: false; why: string }>;
  /** Adopts files already on the server (a todo item's, when it becomes a draft). */
  adopt: (saved: SavedFile[]) => void;
} {
  const files = signal<PendingFile[]>([]);
  const touch = (): void => { files.value = [...files.value]; };
  const add = (list: FileList | File[] | null | undefined): void => {
    for (const file of [...(list ?? [])]) {
      if (files.value.length >= FILE_MAX_COUNT) { toast(t('chat.imgMax', { n: FILE_MAX_COUNT })); break; }
      const cls = classOf(file);
      if (!cls) { toast(t('attach.type')); continue; }
      if (!file.size) { toast(t('chat.imgEmpty')); continue; }
      if (file.size > LIMITS[cls]) { toast(t(`attach.big.${cls}`)); continue; }
      const item: PendingFile = {
        key: ++seq, name: file.name || t(`attach.default.${cls}`), size: file.size, cls, state: 'uploading',
        local: cls === 'text' ? '' : URL.createObjectURL(file), excerpt: '', id: '', error: '', task: null,
      };
      files.value = [...files.value, item];
      if (cls === 'text') {
        void file.slice(0, 600).text().then((x) => { item.excerpt = x.slice(0, 220); touch(); }).catch(() => { /* no preview */ });
      }
      item.task = upload(item, file, touch);
    }
  };
  const revoke = (p: PendingFile): void => { if (p.local.startsWith('blob:')) URL.revokeObjectURL(p.local); };
  return {
    files,
    add,
    remove: (key) => {
      const p = files.value.find((x) => x.key === key);
      if (!p) return;
      revoke(p);
      files.value = files.value.filter((x) => x.key !== key);
    },
    clear: () => { files.value.forEach(revoke); files.value = []; },
    settled: async () => {
      const list = [...files.value];
      await Promise.all(list.map((p) => p.task).filter(Boolean));
      const failed = list.find((p) => p.state !== 'ready');
      if (failed) return { ok: false, why: failed.error || t('chat.tryAgain') };
      return { ok: true, saved: list.map((p) => ({ id: p.id, name: p.name })) };
    },
    adopt: (saved) => {
      const have = new Set(files.value.map((p) => p.id));
      const more: PendingFile[] = saved.filter((a) => !have.has(a.id)).slice(0, Math.max(0, FILE_MAX_COUNT - files.value.length)).map((a) => {
        const cls = classOfSaved(a);
        return {
          key: ++seq, name: a.name, size: a.size ?? 0, cls, state: 'ready' as const, local: cls === 'text' ? '' : fileUrl(a), excerpt: '', id: a.id, error: '', task: null,
        };
      });
      files.value = [...files.value, ...more];
    },
  };
}

export const fileUrl = (a: string | { id: string; url?: string }): string =>
  (typeof a !== 'string' && a.url) ? a.url : '/api/attachments/' + encodeURIComponent(typeof a === 'string' ? a : a.id);

/** Files in a paste event (a screenshot from the clipboard, files copied in a file manager). */
export function pastedFiles(e: ClipboardEvent): File[] {
  return [...(e.clipboardData?.items ?? [])].filter((i) => i.kind === 'file').map((i) => i.getAsFile()).filter((f): f is File => !!f);
}

export const hasFiles = (dt: DataTransfer | null | undefined): boolean => !!dt && [...dt.types].includes('Files');

/**
 * Backspace in an empty field takes the last file off the shelf (then the one before it, and so on).
 * Returns true when it did, so the caller leaves the key alone.
 */
export function backspaceTakesFile(
  e: { key: string; shiftKey?: boolean; ctrlKey?: boolean; metaKey?: boolean; altKey?: boolean; isComposing?: boolean; preventDefault: () => void },
  field: { value: string },
  shelf: { files: { value: { key: number }[] }; remove: (key: number) => void },
): boolean {
  if (e.key !== 'Backspace' || e.shiftKey || e.ctrlKey || e.metaKey || e.altKey || e.isComposing) return false;
  if (field.value !== '') return false;
  const last = shelf.files.value[shelf.files.value.length - 1];
  if (!last) return false;
  e.preventDefault();
  shelf.remove(last.key);
  return true;
}
