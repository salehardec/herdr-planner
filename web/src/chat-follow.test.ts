import { describe, expect, it } from 'vitest';
import { keepFollowing, MOVED_PX } from './chat-follow';

describe('keepFollowing', () => {
  it('follows when the log still stands where the page left it (a turn grew it, the place did not move)', () => {
    expect(keepFollowing(true, 1000, 1000)).toBe(true);
    expect(keepFollowing(true, 1000, 1000 + MOVED_PX)).toBe(true);
  });
  it('does not follow a log the reader moved: the scroll event of that move may come after the turns', () => {
    expect(keepFollowing(true, 1000, 300)).toBe(false);
    expect(keepFollowing(true, 1000, 1000 + MOVED_PX + 1)).toBe(false);
  });
  it('follows the first history of a page, whose place is not known yet', () => {
    expect(keepFollowing(true, null, 0)).toBe(true);
  });
  it('never follows when the page is not following at all', () => {
    expect(keepFollowing(false, 1000, 1000)).toBe(false);
    expect(keepFollowing(false, null, 0)).toBe(false);
  });
});
