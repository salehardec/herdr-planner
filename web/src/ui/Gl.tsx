import type { JSX } from 'preact';

/** An icon from the sprite (index.html injects it). */
export function Gl(props: { n: string; cls?: string }): JSX.Element {
  return <svg class={`gl ${props.cls ?? ''}`} aria-hidden="true"><use href={`#g-${props.n}`} /></svg>;
}

export function Badge(props: { status: string; label: string }): JSX.Element {
  return (
    <span class={`badge s-${props.status}`}>
      <Gl n={props.status} cls={props.status === 'working' ? 'spin' : ''} />
      {props.label}
    </span>
  );
}
