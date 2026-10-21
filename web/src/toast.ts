import { signal } from '@preact/signals';

export interface Toast {
  id: number;
  msg: string;
  icon: string;
  action?: () => void;
  label?: string;
}

export const toasts = signal<Toast[]>([]);
let seq = 0;

export function toast(msg: string, opts: { icon?: string; action?: () => void; label?: string; timeout?: number } = {}): void {
  const id = ++seq;
  toasts.value = [...toasts.value, { id, msg, icon: opts.icon ?? 'done', action: opts.action, label: opts.label }];
  setTimeout(() => { toasts.value = toasts.value.filter((x) => x.id !== id); }, opts.timeout ?? (opts.action ? 7000 : 3600));
}

export const dismissToast = (id: number): void => { toasts.value = toasts.value.filter((x) => x.id !== id); };
