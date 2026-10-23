/* An image of a message over the page: which list, which one, and where the focus goes back to. */
import { signal } from '@preact/signals';
import type { SavedFile } from './attach';

export interface LightboxState { list: SavedFile[]; at: number; from: HTMLElement | null }

export const lightbox = signal<LightboxState | null>(null);

export function openLightbox(list: SavedFile[], at: number, from: HTMLElement | null): void {
  if (!list.length) return;
  lightbox.value = { list, at: Math.max(0, Math.min(at, list.length - 1)), from };
}

export function closeLightbox(): void {
  const from = lightbox.value?.from ?? null;
  lightbox.value = null;
  if (from && from.isConnected) from.focus();
}

export function stepLightbox(delta: number): void {
  const s = lightbox.value;
  if (!s) return;
  lightbox.value = { ...s, at: (s.at + delta + s.list.length) % s.list.length };
}
