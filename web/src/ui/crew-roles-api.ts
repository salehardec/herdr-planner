/* The crew's own roles (PLN-90) and the calls the crew window makes about them. Mirrors src/roles.rs, src/roles_api.rs.
   A role is the planner's; it reaches the board only as the board's own `set` and `add` commands, after «было → стало». */
import { api, post } from '../api';
import type { Affect } from './station-api';

export type RoleKind = 'worker' | 'reviewer';
/** How a role goes on a task: a worker's `worker.*`, a reviewer `instead` of the board's, or `after` the task as a research task. */
export type RoleWay = 'worker' | 'instead' | 'after';

export interface RoleSetting { setting: string; value: string }
export interface RoleUsedTask { id: string; tag: string; title: string; status: string; how: string }
export interface RoleUsage { tasks: RoleUsedTask[]; rules: number; presets: string[] }

export interface CrewRole {
  id: string;
  name: string;
  kind: RoleKind;
  agent: string;
  effort: string;
  fallback: string;
  instructions: string;
  /** The reviewer's own way when the choice does not say. */
  how: RoleWay;
  suggested: string;
  /** What the role sets on the board, by way. */
  sets: Partial<Record<RoleWay, RoleSetting[]>>;
  usage: RoleUsage;
}

export interface CrewRule { id: string; role: string; repo: string; title: string; branch: string; how: string; words: string }
export interface SuggestedRole { key: string; name: string; kind: RoleKind; how: RoleWay; instructions: string }

export interface RolesData {
  ok: true;
  roles: CrewRole[];
  rules: CrewRule[];
  suggested: SuggestedRole[];
  repos: string[];
}

/** What the page sends for a role (the id and the usage are the planner's). */
export interface RoleInput { name: string; kind: RoleKind; agent: string; effort: string; fallback: string; instructions: string; how: string }
export interface RuleInput { role: string; repo: string; title: string; branch: string; how: string }

export interface RoleOp {
  op: 'add' | 'update' | 'delete' | 'suggest' | 'rule_add' | 'rule_update' | 'rule_delete' | 'rule_up';
  id?: string;
  role?: RoleInput;
  rule?: RuleInput;
  key?: string;
  force?: boolean;
}

export const rolesGet = (): Promise<RolesData> => api<RolesData>('/api/crew/roles');
export const rolesPost = (b: RoleOp): Promise<{ ok: true; id: string; usage?: RoleUsage }> => post('/api/crew/roles', b);

export interface RoleStep {
  kind: 'set' | 'add';
  setting: string;
  was: string;
  was_from: string;
  now: string;
  error: string;
  command: string;
}

export interface RolePreview {
  ok: true;
  role: { id: string; name: string; kind: RoleKind };
  how: RoleWay;
  task: { id: string; tag: string; title: string; repo: string; status: string };
  steps: RoleStep[];
  valid: boolean;
  /** An «after» role that already runs after this task. */
  duplicate: boolean;
  /** The task's agent is at work: the settings count from its next start. */
  started: boolean;
  nothing: boolean;
}

export const rolePreview = (b: { role: string; task: string; how?: string }): Promise<RolePreview> => post<RolePreview>('/api/crew/role/preview', b);
export const roleApply = (b: { role: string; task: string; how?: string }): Promise<{ ok: true; role: string; how: RoleWay; ran: string[]; follow: string }> =>
  post('/api/crew/role/apply', { ...b, confirm: true });

export interface RoleMatch { ok: true; role: { id: string; name: string; kind: RoleKind } | null; how?: RoleWay; rule?: string }
export const roleMatch = (b: { repo: string; title: string; branch?: string }): Promise<RoleMatch> => post<RoleMatch>('/api/crew/role/match', b);

export type { Affect };
