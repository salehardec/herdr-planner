import type { JSX } from 'preact';
import { RefText } from './TaskChip';

export { goToTag } from './TaskChip';

/** Text where a task key (`PLN-9`) is a chip that flies to the task. */
export function RichText(props: { text: string; class?: string }): JSX.Element {
  return <span class={props.class}><RefText text={props.text ?? ''} /></span>;
}
