import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect } from 'preact/hooks';
import { addDialogs, todoDraft } from '../dialogs';
import { currentView, galaxyId, snap, taskOf } from '../store';
import { installYazi, todoAssign, todoSend, todoToTask, todoUpdate, yaziDlg } from '../actions';
import { armed } from '../store';
import { t } from '../i18n';
import { Gl } from './Gl';
import { loadModels, modelList } from './Todo';
import { AddGalaxy } from './AddGalaxy';
import { AddUniverse } from './AddUniverse';
import { lightPreset } from './station-api';
import { SavedFiles } from './Attached';
import { toast } from '../toast';
import type { SavedFile } from '../attach';
import { Coach, FirstRun } from './FirstRun';
import './dialogs.css';

const popProgram = signal('claude');
const popModel = signal('');
const editText = todoDraft.text;
const editFiles = todoDraft.files;

/** Saves the corrections (if there are any) so that what goes out is what the window shows. */
async function saveDraft(id: string): Promise<boolean> {
  const item = snap.value?.todo.find((x) => x.id === id);
  if (!item) return false;
  const text = editText.value.trim();
  if (!text) return false;
  const files = editFiles.value;
  const patch: { text?: string; attachments?: SavedFile[] } = {};
  if (text !== item.text) patch.text = text;
  if (files.length !== (item.attachments ?? []).length) patch.attachments = files;
  if (!patch.text && !patch.attachments) return true;
  return !!(await todoUpdate(id, patch));
}

function DraftFields(props: { prefix: string }): JSX.Element {
  return (
    <>
      <label>{t('todo.popText')}
        <textarea class="field" id={`${props.prefix}-text`} rows={3} maxLength={400} value={editText.value} onInput={(e) => { editText.value = (e.target as HTMLTextAreaElement).value; }} />
      </label>
      <SavedFiles list={editFiles.value} class="dfiles" onRemove={(id) => { editFiles.value = editFiles.value.filter((a) => a.id !== id); }} />
    </>
  );
}

/** Dropping a list item on a universe: the text and the files to correct, which project, which program and which model. */
function TodoPopover(): JSX.Element | null {
  const p = addDialogs.popover.value;
  const projects = currentView.value.projects;
  const list = modelList.value;
  useEffect(() => { if (p) void loadModels(popProgram.value); }, [p?.id, popProgram.value]);
  if (!p) return null;
  const go = async (): Promise<void> => {
    const agent = popModel.value.trim() ? (popModel.value.includes('/') ? popModel.value.trim() : `${popProgram.value}/${popModel.value.trim()}`) : '';
    if (!(await saveDraft(p.id))) return;
    const ok = await todoToTask(p.id, p.repo, agent);
    if (ok) addDialogs.popover.value = null;
  };
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) addDialogs.popover.value = null; }}>
      <section class="panel dlg sm" id="pop" role="dialog" aria-label={t('todo.popTitle')}>
        <header><b>{t('todo.popTitle')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { addDialogs.popover.value = null; }}><Gl n="close" /></button></header>
        <div class="bd">
          <DraftFields prefix="pop" />
          <label>{t('todo.popProject')}
            <select class="field" id="pop-repo" value={p.repo} onChange={(e) => { addDialogs.popover.value = { id: p.id, repo: (e.target as HTMLSelectElement).value, galaxy: p.galaxy }; }}>
              {projects.map((x) => <option value={x.name}>{x.name}</option>)}
            </select>
          </label>
          <label>{t('todo.popProgram')}</label>
          <div class="prog">
            {['claude', 'codex', 'pi', 'omp'].map((x) => (
              <button type="button" class={`btn sm ${popProgram.value === x ? 'on' : ''}`} key={x} data-prog={x} onClick={() => { popProgram.value = x; popModel.value = ''; }}>{x}</button>
            ))}
          </div>
          <div class="prog">
            <button type="button" class="btn sm" id="pop-light" title={t('todo.lightHint')} onClick={() => {
              void lightPreset(p.galaxy ?? '').then((v) => {
                if (v) { popProgram.value = v.split('/')[0]; popModel.value = v; } else toast(t('todo.lightNone'));
              });
            }}>{t('todo.light')}</button>
          </div>
          <label>{t('todo.popModel')}
            <input class="field" id="pop-model" list="models-list" value={popModel.value} placeholder={list.loading ? t('todo.modelsLoading') : t('todo.modelPh')} onInput={(e) => { popModel.value = (e.target as HTMLInputElement).value; }} />
            <datalist id="models-list">{list.names.map((n) => <option value={n} />)}</datalist>
          </label>
          <div class="hint">{list.loading ? t('todo.modelsLoading') : list.names.length ? t('todo.modelsCount', { n: list.names.length }) : t('todo.modelsFree')}</div>
          <div class="act">
            <button class="btn primary" type="button" id="pop-go" disabled={!editText.value.trim()} onClick={() => void go()}>{t('todo.popGo')}</button>
            <button class="btn" type="button" onClick={() => { addDialogs.popover.value = null; }}>{t('cancel')}</button>
          </div>
        </div>
      </section>
    </div>
  );
}

/** Dropping a list item on a planet: the words and the files for its agent, to correct before they are sent. */
function TodoTell(): JSX.Element | null {
  const p = addDialogs.tell.value;
  if (!p) return null;
  const tag = taskOf(p.task, p.galaxy)?.tag ?? p.task;
  const go = async (): Promise<void> => {
    if (!(await saveDraft(p.id))) return;
    await todoAssign(p.id, p.task);
    await todoSend(p.id);
    addDialogs.tell.value = null;
  };
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) addDialogs.tell.value = null; }}>
      <section class="panel dlg sm" id="pop-tell" role="dialog" aria-label={t('todo.tellTitle', { task: tag })}>
        <header><b>{t('todo.tellTitle', { task: tag })}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { addDialogs.tell.value = null; }}><Gl n="close" /></button></header>
        <div class="bd">
          <DraftFields prefix="tell" />
          <p class="hint">{t('todo.tellHint')}</p>
          <div class="act">
            <button class="btn primary" type="button" id="tell-go" disabled={!editText.value.trim()} onClick={() => void go()}>{t('todo.tellGo')}</button>
            <button class="btn" type="button" onClick={() => { addDialogs.tell.value = null; }}>{t('cancel')}</button>
          </div>
        </div>
      </section>
    </div>
  );
}

/** yazi is not on the far machine: how to put it there (a release binary without sudo, or the package manager). */
function YaziDialog(): JSX.Element | null {
  const d = yaziDlg.value;
  if (!d) return null;
  const st = d.state;
  const close = (): void => { yaziDlg.value = null; };
  const binOn = armed.value?.key === 'yazi:binary';
  const brewOn = armed.value?.key === 'yazi:brew';
  const brew = st?.managers.find((m) => m.id === 'brew');
  const pacman = st?.managers.find((m) => m.id === 'pacman');
  const apt = st?.managers.find((m) => m.id === 'apt');
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget && !d.busy) close(); }}>
      <section class="panel dlg" id="yazi-dlg" role="dialog" aria-label={t('far.yaziTitle')}>
        <header><b>{t('far.yaziTitle', { g: d.galaxy })}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={close}><Gl n="close" /></button></header>
        <div class="bd">
          <p class="hint">{t('far.yaziMissing', { os: st?.os ?? '?', arch: st?.arch ?? '?' })}</p>
          {st && !st.has_file && <p class="hint bad">{t('far.noFile')}</p>}
          {st?.binary_url && (
            <div class="info">
              <b>{t('far.byBinary')}</b>
              <div class="hint">{t('far.byBinaryHint')} <code>{st.binary_url}</code></div>
              <div class="act"><button class={`btn primary ${binOn ? 'arm' : ''}`} type="button" id="yazi-binary" disabled={d.busy} onClick={() => installYazi('binary')}>{binOn ? t('galaxy.addConfirm') : t('far.install')}</button></div>
            </div>
          )}
          {brew && (
            <div class="info"><b>Homebrew</b> <code>{brew.command}</code>
              <div class="act"><button class={`btn ${brewOn ? 'arm' : ''}`} type="button" id="yazi-brew" disabled={!brew.available || d.busy} onClick={() => installYazi('brew')}>{brewOn ? t('galaxy.addConfirm') : t('far.install')}</button></div>
              {brew.why_not && <div class="hint">{brew.why_not}</div>}</div>
          )}
          {pacman && (
            <div class="info"><b>pacman</b> <code>{pacman.command}</code>
              <div class="hint">{t('far.sudoHint')}</div>
              <div class="act"><button class="btn" type="button" id="yazi-pacman" disabled={!pacman.available || d.busy} onClick={() => installYazi('pacman')}>{t('far.inTerminal')}</button></div></div>
          )}
          {apt && <div class="info"><b>apt</b> <div class="hint">{apt.why_not ?? t('far.noApt')}</div></div>}
          {d.log.length > 0 && <ul class="steps">{d.log.map((l, i) => <li class={i === d.log.length - 1 && d.busy ? 'run' : 'ok'} key={i}><span class="i">{i === d.log.length - 1 && d.busy ? '…' : '✓'}</span><div>{l}</div></li>)}</ul>}
          {d.err && <p class="hint bad">{d.err}</p>}
          <div class="act"><button class="btn" type="button" onClick={close}>{t('cancel')}</button></div>
        </div>
      </section>
    </div>
  );
}

export function Dialogs(): JSX.Element {
  return (
    <>
      <YaziDialog />
      <AddGalaxy />
      <TodoPopover />
      <TodoTell />
      <AddUniverse />
      <FirstRun />
      <Coach />
    </>
  );
}

void galaxyId;
