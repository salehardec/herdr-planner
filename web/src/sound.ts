/* A short sound for the warp and for a new request, if the operator switched sound on (the bell in the menu). */
import { notifyOn } from './store';

let ctx: AudioContext | null = null;

function audio(): AudioContext | null {
  try {
    ctx = ctx ?? new (window.AudioContext || (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext)();
    return ctx;
  } catch { return null; }
}

export function warpSound(): void {
  if (!notifyOn.value) return;
  const a = audio();
  if (!a) return;
  const o = a.createOscillator();
  const g = a.createGain();
  o.type = 'sawtooth';
  o.frequency.setValueAtTime(70, a.currentTime);
  o.frequency.exponentialRampToValueAtTime(520, a.currentTime + 0.7);
  g.gain.setValueAtTime(0.0001, a.currentTime);
  g.gain.exponentialRampToValueAtTime(0.03, a.currentTime + 0.2);
  g.gain.exponentialRampToValueAtTime(0.0001, a.currentTime + 0.9);
  o.connect(g).connect(a.destination);
  o.start();
  o.stop(a.currentTime + 0.95);
}
