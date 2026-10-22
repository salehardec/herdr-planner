/* A sparkline: a thin line through samples of a percentage, a break where the value is not known (the yard was silent).
   No axes and no legend: the numbers beside it are the current value and the peak of the window. */
import type { JSX } from 'preact';

export interface SparkPoint { at: number; v: number | null }

/** The `d` of a path through the points, on a 0 … 100 × 0 … `h` box; an unknown value starts a new piece. */
export function sparkPath(points: SparkPoint[], from: number, to: number, top = 100, h = 100, pad = 4): string {
  const span = Math.max(1, to - from);
  const max = Math.max(1, top);
  const x = (at: number): number => Math.max(0, Math.min(100, ((at - from) / span) * 100));
  const y = (v: number): number => h - pad - (Math.max(0, Math.min(max, v)) / max) * (h - pad * 2);
  let d = '';
  let open = false;
  for (const p of points) {
    if (p.v == null || !Number.isFinite(p.v)) {
      open = false;
      continue;
    }
    d += `${open ? ' L' : ' M'}${x(p.at).toFixed(2)} ${y(p.v).toFixed(2)}`;
    open = true;
  }
  return d.trim();
}

/** The highest value of the window, never below 100: an overloaded CPU goes above a full line. */
export function sparkTop(points: SparkPoint[]): number {
  let top = 100;
  for (const p of points) if (p.v != null && Number.isFinite(p.v)) top = Math.max(top, p.v);
  return top;
}

/** The samples of the window and their peak, so the line and the number beside it are of the same window. */
export function sparkStats(points: SparkPoint[], from: number): { shown: SparkPoint[]; peak: number } {
  const shown = points.filter((p) => p.at >= from);
  let peak = 0;
  for (const p of shown) {
    if (p.v == null || !Number.isFinite(p.v)) continue;
    peak = Math.max(peak, p.v);
  }
  return { shown, peak };
}

export function Spark(props: { points: SparkPoint[]; from: number; to: number; top?: number; label: string; cls?: string; id?: string }): JSX.Element {
  const top = props.top ?? sparkTop(props.points);
  const d = sparkPath(props.points, props.from, props.to, top);
  const known = props.points.filter((p) => p.v != null).length;
  const yAt = (v: number): number => 96 - (Math.min(v, top) / top) * 92;
  const grid = yAt(top > 100 ? 100 : 50);
  return (
    <svg
      class={`spark ${props.cls ?? ''}`} id={props.id} viewBox="0 0 100 100" preserveAspectRatio="none"
      role="img" aria-label={props.label} data-points={known} data-top={Math.round(top)}
    >
      <line class="grid" x1="0" y1={grid} x2="100" y2={grid} vector-effect="non-scaling-stroke" />
      <path class="line" d={d} vector-effect="non-scaling-stroke" />
    </svg>
  );
}
