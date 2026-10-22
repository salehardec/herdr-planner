/* The id of a memory document in the search index is `<scope>|<entry id>`: the scope is empty for the Galaxy's own memory,
   a repo name for a universe's, `@program` for the memory the Coordinator's program keeps (read only).
   Mirrors `memory::doc_id` in src/memory.rs. */

export interface MemRef {
  /** The universe (`''` for the Galaxy level). */
  repo: string;
  /** The program whose own memory it is (`''` for ours). */
  foreign: string;
  /** The file's id. */
  id: string;
  /** What the panel marks to scroll to: the id, or `@program|id` for the program's own memory. */
  focus: string;
}

export function parseDocId(docId: string): MemRef {
  const at = docId.indexOf('|');
  const scope = at < 0 ? '' : docId.slice(0, at);
  const id = at < 0 ? docId : docId.slice(at + 1);
  const foreign = scope.startsWith('@') ? scope.slice(1) : '';
  const repo = scope && !foreign ? scope : '';
  return { repo, foreign, id, focus: foreign ? `${scope}|${id}` : id };
}
