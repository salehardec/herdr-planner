/* The pickers of the crew screen: a program and one of ITS models (from `herdr-projects models`, never typed), an effort
   level, and a fallback chain of subscriptions. Used by the role cards, the fallback form and the preset editor. */
import type { JSX } from 'preact';
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { t } from '../i18n';
import { callError, ModelsData, programOf, stationModels, StationProgram } from './station-api';
import { chainOf, chainValue, modelOptions, offerable, programName, untilLabel } from './crew-view';

/** A program's models and efforts; asks again every few seconds while the board is still reading them. */
export function useModels(program: string): { data: ModelsData | null; loading: boolean; error: string } {
  const [data, setData] = useState<ModelsData | null>(null);
  const [error, setError] = useState('');
  useEffect(() => {
    let alive = true;
    let timer = 0;
    setData(null);
    setError('');
    if (!program) return () => { alive = false; };
    const go = (n: number): void => {
      stationModels(program)
        .then((m) => {
          if (!alive) return;
          setData(m);
          if (m.loading && n < 40) timer = window.setTimeout(() => go(n + 1), 1500);
        })
        .catch((e) => { if (alive) setError(callError(e).message); });
    };
    go(0);
    return () => { alive = false; window.clearTimeout(timer); };
  }, [program]);
  return { data, loading: !error && (!data || data.loading), error };
}

/** The mark after a program's name in a picker: why it is a poor choice now. */
export function programMark(p: StationProgram | undefined): string {
  if (!p || !p.installed) return t('st.notInstalledMark');
  const a = p.state.avail;
  if (a.state === 'signed_out') return t('st.notSignedMark');
  if (a.state === 'limited') {
    const until = untilLabel(a);
    return until ? t('st.limitedUntilMark', { until }) : t('st.limitedMark');
  }
  return '';
}

/** The programs a picker lists: the ones installed here, and the one already chosen when it is not. */
function programsFor(programs: StationProgram[], current: string): StationProgram[] {
  const list = programs.filter((p) => p.installed);
  const cur = programs.find((p) => p.name === current);
  return cur && !cur.installed ? [...list, cur] : list;
}

export interface AgentPickProps {
  programs: StationProgram[];
  /** `program/model`, a program alone, or empty. */
  value: string;
  onChange: (value: string) => void;
  /** Ids of the controls: `st-prog-<id>`, `st-model-<id>`. */
  id: string;
  /** The word for «nothing chosen»; with none, a program is always chosen. */
  emptyLabel?: string;
  /** Offer the program alone («whatever model the program has») above its models, for a fallback chain. */
  bare?: boolean;
  /** Programs that may not be chosen (already in the chain). */
  skip?: string[];
}

/** A program, then one of its models. */
export function AgentPick(props: AgentPickProps): JSX.Element {
  const { programs, value, onChange, id } = props;
  const fromValue = programOf(value);
  const [prog, setProg] = useState(fromValue);
  useEffect(() => { if (fromValue && fromValue !== prog) setProg(fromValue); }, [fromValue]);
  const { data, loading, error } = useModels(prog);
  const opts = modelOptions(prog, data, value.startsWith(`${prog}/`) || value === prog ? value : '', !!props.bare);
  const mine = prog && (value === prog || value.startsWith(`${prog}/`));
  // Set the initial model before the list becomes interactive: a deferred effect can overwrite a choice made as it arrives.
  useLayoutEffect(() => {
    if (!prog || !data || data.program !== prog || data.loading || mine) return;
    if (opts.length) onChange(opts[0].value);
  }, [prog, data, mine]);
  const listed = programsFor(programs, fromValue || prog).filter((p) => !(props.skip ?? []).includes(p.name) || p.name === prog);
  const pick = (p: string): void => {
    setProg(p);
    if (!p) onChange('');
  };
  return (
    <div class="cpick">
      <label class="fl2">{t('st.program')}
        <select class="field" id={`st-prog-${id}`} value={prog} onChange={(e) => pick((e.target as HTMLSelectElement).value)}>
          {props.emptyLabel !== undefined && <option value="">{props.emptyLabel}</option>}
          {!props.emptyLabel && !prog && <option value="">{t('st.chooseProgram')}</option>}
          {listed.map((p) => {
            const mark = programMark(p);
            return <option key={p.name} value={p.name} disabled={p.state.avail.state === 'signed_out' && p.name !== prog}>{mark ? `${p.name} ${mark}` : p.name}</option>;
          })}
        </select>
      </label>
      {prog && (
        <label class="fl2">{t('st.model')}
          <select class="field" id={`st-model-${id}`} value={mine ? value : ''} disabled={loading || !opts.length} onChange={(e) => onChange((e.target as HTMLSelectElement).value)}>
            {!mine && <option value="">{loading ? t('st.modelsLoadingShort') : t('st.chooseModel')}</option>}
            {opts.map((o) => (
              <option key={o.value} value={o.value}>
                {o.kind === 'default' ? t('st.defaultModel', { p: prog }) : o.kind === 'current' ? t('st.currentModel', { v: o.value }) : o.value}
              </option>
            ))}
          </select>
        </label>
      )}
      {prog && <ModelsHint prog={prog} data={data} loading={loading} error={error} />}
    </div>
  );
}

function ModelsHint(props: { prog: string; data: ModelsData | null; loading: boolean; error: string }): JSX.Element {
  const { prog, data, loading, error } = props;
  let text = '';
  if (loading) text = t('st.modelsLoading');
  else if (error) text = error;
  else if (data && data.names.length) text = t('st.modelsCount', { n: data.names.length });
  else if (data && data.suggested.length) text = t('st.modelsFromUse', { p: prog, n: data.suggested.length });
  else if (data) text = t('st.noModels', { p: prog });
  return <div class="hint" data-models-hint={prog}>{text}</div>;
}

/** The reasoning effort a role's program takes; the levels come with its models. */
export function EffortPick(props: { agent: string; value: string; onChange: (v: string) => void; id: string; emptyLabel: string }): JSX.Element {
  const prog = programOf(props.agent);
  const { data, loading } = useModels(prog);
  const levels = data?.effort ?? [];
  const all = !props.value || levels.includes(props.value) ? levels : [props.value, ...levels];
  const none = !prog ? t('st.effortNeedsAgent') : loading ? t('st.modelsLoadingShort') : levels.length ? '' : t('st.noEffort', { p: prog });
  return (
    <div class="cpick">
      <label class="fl2">{prog ? t('st.effortLevel', { p: prog }) : t('st.f.effort')}
        <select class="field" id={`st-effort-${props.id}`} value={props.value} disabled={!prog || loading} onChange={(e) => props.onChange((e.target as HTMLSelectElement).value)}>
          <option value="">{props.emptyLabel}</option>
          {all.map((l) => <option key={l} value={l}>{l}</option>)}
        </select>
      </label>
      {none && <div class="hint">{none}</div>}
    </div>
  );
}

/**
 * A fallback chain: the subscriptions that take over, in order, when the role's agent cannot go on. Add from the programs
 * installed here (a signed-out one cannot be added), move and remove. The value is the board's own (`codex, claude/claude-opus-5-5`
 * or `none`).
 */
export function ChainEditor(props: { programs: StationProgram[]; value: string; onChange: (v: string) => void; id: string; agent: string }): JSX.Element {
  const items = chainOf(props.value);
  const [add, setAdd] = useState('');
  const set = (next: string[]): void => props.onChange(chainValue(next));
  const move = (i: number, d: number): void => {
    const next = items.slice();
    const [x] = next.splice(i, 1);
    next.splice(i + d, 0, x);
    set(next);
  };
  const canAdd = !!add && !items.includes(add);
  return (
    <div class="chain" data-chain={props.id}>
      {items.length === 0 && <div class="hint">{t('st.chainEmpty')}</div>}
      {items.length > 0 && (
        <ol class="chain-list">
          {items.map((it, i) => {
            const p = props.programs.find((x) => x.name === programName(it));
            const mark = programMark(p);
            return (
              <li key={it} data-chain-item={it}>
                <span class="n">{i + 1}</span>
                <code>{it}</code>
                {mark && <span class="m warn">{mark}</span>}
                {it === props.agent && <span class="m warn">{t('st.chainSame')}</span>}
                <button class="btn sm" type="button" data-chain-up={it} disabled={i === 0} aria-label={t('st.chainUp')} onClick={() => move(i, -1)}>↑</button>
                <button class="btn sm" type="button" data-chain-down={it} disabled={i === items.length - 1} aria-label={t('st.chainDown')} onClick={() => move(i, 1)}>↓</button>
                <button class="btn sm danger" type="button" data-chain-del={it} aria-label={t('st.chainDel')} onClick={() => set(items.filter((x) => x !== it))}>✕</button>
              </li>
            );
          })}
        </ol>
      )}
      <div class="chain-add">
        <AgentPick key={items.join('|')} programs={props.programs.filter(offerable)} value={add} onChange={setAdd} id={`${props.id}-add`} emptyLabel={t('st.chainAddFrom')} bare />
        <button class="btn sm primary" type="button" data-chain-add={props.id} disabled={!canAdd} onClick={() => { set([...items, add]); setAdd(''); }}>{t('st.chainAdd')}</button>
      </div>
    </div>
  );
}
