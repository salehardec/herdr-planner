import type { JSX } from 'preact';
import { dismissToast, toasts } from '../toast';
import { Gl } from './Gl';

export function Toasts(): JSX.Element {
  return (
    <div class="toasts" id="toasts" role="status">
      {toasts.value.map((x) => (
        <div class="toast" key={x.id}>
          <Gl n={x.icon} />
          <span>{x.msg}</span>
          {x.action && (
            <button class="undo" type="button" onClick={() => { dismissToast(x.id); x.action?.(); }}>{x.label}</button>
          )}
        </div>
      ))}
    </div>
  );
}
