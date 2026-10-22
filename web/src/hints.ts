/* The two questions the `/` and `@` lists ask the server, kept between openings: the commands of a
   program (read from the program's own files on this machine) and the files of the repositories. */
import { api } from './api';
import type { FileHit, SlashCmd } from './complete';

const cmds = new Map<string, SlashCmd[]>();
let files = new Map<string, FileHit[]>();

/** The commands of a program in a folder; read once per program and folder, until the page is reloaded. */
export async function commandsFor(program: string, dir: string): Promise<SlashCmd[]> {
  const key = `${program}|${dir}`;
  const hit = cmds.get(key);
  if (hit) return hit;
  const r = await api<{ commands: SlashCmd[] }>(
    `/api/hints/slash?program=${encodeURIComponent(program)}&dir=${encodeURIComponent(dir)}`,
  );
  const list = r.commands ?? [];
  cmds.set(key, list);
  return list;
}

/** Forgets what was read about a program: the next `/` reads it again (the Coordinator may have changed). */
export function forgetCommands(program?: string): void {
  if (program === undefined) { cmds.clear(); return; }
  for (const key of [...cmds.keys()]) if (key.startsWith(`${program}|`)) cmds.delete(key);
}

/** The files and folders of the repositories whose path holds the typed words. */
export async function filesFor(query: string): Promise<FileHit[]> {
  const q = query.trim().toLowerCase();
  const hit = files.get(q);
  if (hit) return hit;
  const r = await api<{ items: FileHit[] }>(`/api/hints/files?q=${encodeURIComponent(query.trim())}&limit=20`);
  const list = r.items ?? [];
  if (files.size > 200) files = new Map();
  files.set(q, list);
  return list;
}
