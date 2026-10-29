import { signal } from '@preact/signals';
import type { JSX } from 'preact';
import { useEffect, useState } from 'preact/hooks';
import { addDialogs, todoDraft } from '../dialogs';
import { currentView, snap, taskOf } from '../store';
import { todoAssign, todoSend, todoToTask, todoUpdate } from '../actions';
import { t } from '../i18n';
import { Gl } from './Gl';
import { loadModels, modelList } from './Todo';
import { AddUniverse } from './AddUniverse';
import { AddRunner } from './AddRunner';
import { lightPreset } from './station-api';
import { rolesGet, roleMatch, RoleMatch, RolesData } from './crew-roles-api';
import { SavedFiles } from './Attached';
import { AutoTextarea } from './AutoText';
import { toast } from '../toast';
import type { SavedFile } from '../attach';
import { Coach, FirstRun } from './FirstRun';
import './dialogs.css';

const popProgram = signal('claude');
const popModel = signal('');
/** The crew's own role for the new task: `auto` (the rules decide), `none`, or a role's id; and the way a reviewer role goes on. */
const popRole = signal('auto');
const popHow = signal('');
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

function DraftFields(props: { prefix: string; onSend: () => void; hint: string }): JSX.Element {
  return (
    <>
      <label>{t('todo.popText')}
        <AutoTextarea
          class="field" id={`${props.prefix}-text`} value={editText.value}
          onInput={(e) => { editText.value = (e.target as HTMLTextAreaElement).value; }} onSend={props.onSend}
        />
        <span class="hint keyhint" id={`${props.prefix}-text-hint`}>{props.hint}</span>
      </label>
      <SavedFiles list={editFiles.value} class="dfiles" onRemove={(id) => { editFiles.value = editFiles.value.filter((a) => a.id !== id); }} />
    </>
  );
}

/** Which of the crew's own roles the new task gets: by the rules (and which they give now), none, or one picked. */
function PopRole(props: { repo: string; text: string }): JSX.Element | null {
  const [roles, setRoles] = useState<RolesData | null>(null);
  const [match, setMatch] = useState<RoleMatch | null>(null);
  useEffect(() => { let alive = true; rolesGet().then((d) => { if (alive) setRoles(d); }).catch(() => undefined); return () => { alive = false; }; }, []);
  const title = props.text.split('\n').find((l) => l.trim()) ?? '';
  useEffect(() => {
    let alive = true;
    roleMatch({ repo: props.repo, title }).then((m) => { if (alive) setMatch(m); }).catch(() => { if (alive) setMatch(null); });
    return () => { alive = false; };
  }, [props.repo, title]);
  if (!roles || roles.roles.length === 0) return null;
  const picked = roles.roles.find((r) => r.id === popRole.value);
  return (
    <div class="pop-role" id="pop-role-box">
      <label>{t('cr.pop.role')}
        <select class="field" id="pop-role" value={popRole.value} onChange={(e) => { popRole.value = (e.target as HTMLSelectElement).value; popHow.value = ''; }}>
          <option value="auto">{t('cr.pop.auto')}</option>
          <option value="none">{t('cr.pop.none')}</option>
          {roles.roles.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}
        </select>
      </label>
      {popRole.value === 'auto' && (
        <div class="hint" id="pop-role-match" data-match={match?.role?.name ?? ''}>
          {match?.role ? t('cr.pop.byRule', { name: match.role.name, rule: match.rule ?? '' }) : t('cr.pop.noRule')}
        </div>
      )}
      {picked?.kind === 'reviewer' && (
        <label>{t('cr.put.how')}
          <select class="field" id="pop-role-how" value={popHow.value || picked.how} onChange={(e) => { popHow.value = (e.target as HTMLSelectElement).value; }}>
            <option value="instead">{t('cr.way.instead')} — {t('cr.way.insteadHint')}</option>
            <option value="after">{t('cr.way.after')} — {t('cr.way.afterHint')}</option>
          </select>
        </label>
      )}
    </div>
  );
}

/** Dropping a list item on a universe: the text and the files to correct, which project, which program and which model. */
function TodoPopover(): JSX.Element | null {
  const p = addDialogs.popover.value;
  const projects = currentView.value.projects;
  const list = modelList.value;
  useEffect(() => { if (p) void loadModels(popProgram.value); }, [p?.id, popProgram.value]);
  useEffect(() => { popRole.value = 'auto'; popHow.value = ''; }, [p?.id]);
  if (!p) return null;
  const go = async (): Promise<void> => {
    const agent = popModel.value.trim() ? (popModel.value.includes('/') ? popModel.value.trim() : `${popProgram.value}/${popModel.value.trim()}`) : '';
    if (!(await saveDraft(p.id))) return;
    const ok = await todoToTask(p.id, p.repo, agent, popRole.value, popHow.value);
    if (ok) addDialogs.popover.value = null;
  };
  return (
    <div class="scrim" onMouseDown={(e) => { if (e.target === e.currentTarget) addDialogs.popover.value = null; }}>
      <section class="panel dlg sm" id="pop" role="dialog" aria-label={t('todo.popTitle')}>
        <header><b>{t('todo.popTitle')}</b><button class="btn icon sm" type="button" aria-label={t('close')} onClick={() => { addDialogs.popover.value = null; }}><Gl n="close" /></button></header>
        <div class="bd">
          <DraftFields prefix="pop" onSend={() => void go()} hint={t('text.keys.create')} />
          <label>{t('todo.popProject')}
            <select class="field" id="pop-repo" value={p.repo} onChange={(e) => { addDialogs.popover.value = { id: p.id, repo: (e.target as HTMLSelectElement).value }; }}>
              {projects.map((x) => <option value={x.name}>{x.name}</option>)}
            </select>
          </label>
          <PopRole repo={p.repo} text={editText.value} />
          <label>{t('todo.popProgram')}</label>
          <div class="prog">
            {['claude', 'codex', 'pi', 'omp'].map((x) => (
              <button type="button" class={`btn sm ${popProgram.value === x ? 'on' : ''}`} key={x} data-prog={x} onClick={() => { popProgram.value = x; popModel.value = ''; }}>{x}</button>
            ))}
          </div>
          <div class="prog">
            <button type="button" class="btn sm" id="pop-light" title={t('todo.lightHint')} onClick={() => {
              void lightPreset().then((v) => {
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
  const tag = taskOf(p.task)?.tag ?? p.task;
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
          <DraftFields prefix="tell" onSend={() => void go()} hint={t('text.keys.send')} />
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

export function Dialogs(): JSX.Element {
  return (
    <>
      <TodoPopover />
      <TodoTell />
      <AddUniverse />
      <AddRunner />
      <FirstRun />
      <Coach />
    </>
  );
}
