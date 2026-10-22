import { describe, expect, it } from 'vitest';
import { parseDocId } from './memory-ids';

describe('memory document ids', () => {
  it('the Galaxy level has an empty scope', () => {
    expect(parseDocId('|gitlab-bez-pr')).toEqual({ repo: '', foreign: '', id: 'gitlab-bez-pr', focus: 'gitlab-bez-pr' });
  });
  it('a universe is named by its repo', () => {
    expect(parseDocId('alpha|merge-squash')).toEqual({ repo: 'alpha', foreign: '', id: 'merge-squash', focus: 'merge-squash' });
  });
  it('the memory of the Coordinator\'s program is marked with @ and keeps it in the focus', () => {
    expect(parseDocId('@claude|user-answers-short')).toEqual({ repo: '', foreign: 'claude', id: 'user-answers-short', focus: '@claude|user-answers-short' });
  });
  it('a repo name with dots and dashes is one scope; only the first bar splits', () => {
    expect(parseDocId('my.repo-2|a')).toMatchObject({ repo: 'my.repo-2', id: 'a' });
    expect(parseDocId('|odd|id')).toMatchObject({ repo: '', id: 'odd|id' });
  });
  it('an id without a scope is a Galaxy entry', () => {
    expect(parseDocId('plain')).toMatchObject({ repo: '', foreign: '', id: 'plain' });
  });
});
