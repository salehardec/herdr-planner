/* The person's own wallpaper: a file turned into a data: URL small enough for the browser's storage. */

const MAX_SIDE = 1920;
const BIG = 1_500_000;

const readUrl = (file: Blob): Promise<string> => new Promise((ok, no) => {
  const r = new FileReader();
  r.onload = () => ok(String(r.result));
  r.onerror = () => no(r.error);
  r.readAsDataURL(file);
});

const decode = (url: string): Promise<HTMLImageElement> => new Promise((ok, no) => {
  const im = new Image();
  im.onload = () => ok(im);
  im.onerror = () => no(new Error('not an image'));
  im.src = url;
});

/** A picture as a data: URL; one that is big is drawn smaller (1920 px at most) and kept as a JPEG. Rejects for a file that is not an image. */
export async function readWallpaper(file: Blob): Promise<string> {
  if (!file.type.startsWith('image/')) throw new Error('not an image');
  const url = await readUrl(file);
  if (url.length <= BIG) return url;
  const im = await decode(url);
  const k = Math.min(1, MAX_SIDE / Math.max(im.naturalWidth, im.naturalHeight));
  const c = document.createElement('canvas');
  c.width = Math.max(1, Math.round(im.naturalWidth * k));
  c.height = Math.max(1, Math.round(im.naturalHeight * k));
  c.getContext('2d')?.drawImage(im, 0, 0, c.width, c.height);
  return c.toDataURL('image/jpeg', 0.85);
}
