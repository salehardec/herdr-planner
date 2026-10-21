/* Is a text mostly not Russian? Then the page offers a translation. */

export function cyrillicShare(text: string): number {
  let letters = 0;
  let cyr = 0;
  for (const ch of text) {
    if (/\p{L}/u.test(ch)) {
      letters++;
      if (/[Ѐ-ӿ]/.test(ch)) cyr++;
    }
  }
  return letters === 0 ? 1 : cyr / letters;
}

/** Few letters mean code or numbers: nothing to translate. */
export function needsTranslation(text: string, target: 'ru' | 'en' = 'ru'): boolean {
  const s = text.replace(/```[\s\S]*?```/g, '');
  let letters = 0;
  for (const ch of s) if (/\p{L}/u.test(ch)) letters++;
  if (letters < 20) return false;
  const share = cyrillicShare(s);
  return target === 'ru' ? share < 0.3 : share > 0.7;
}
