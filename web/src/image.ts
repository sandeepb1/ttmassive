// Getting a photo ready to attach: phone cameras make 5-12 MB pictures, which the agent does
// not need. Shrink to a sensible size in the browser before sending.

const MAX_SIDE = 2048;
const MAX_ORIGINAL_BYTES = 10 * 1024 * 1024;
const PASS_THROUGH = /^image\/(png|jpeg|gif|webp)$/;

/** The image as a JPEG no larger than 2048 px a side; the original if it cannot be decoded here. */
export async function prepareImage(file: File): Promise<Blob> {
  if (!file.type.startsWith('image/')) throw new Error('That is not an image');
  try {
    const bitmap = await createImageBitmap(file);
    const scale = Math.min(1, MAX_SIDE / Math.max(bitmap.width, bitmap.height));
    const w = Math.max(1, Math.round(bitmap.width * scale));
    const h = Math.max(1, Math.round(bitmap.height * scale));
    const canvas = document.createElement('canvas');
    canvas.width = w;
    canvas.height = h;
    const ctx = canvas.getContext('2d');
    if (ctx) {
      // JPEG has no transparency: put transparent PNGs on white, not black.
      ctx.fillStyle = '#fff';
      ctx.fillRect(0, 0, w, h);
      ctx.drawImage(bitmap, 0, 0, w, h);
      bitmap.close();
      const blob = await new Promise<Blob | null>((done) => canvas.toBlob(done, 'image/jpeg', 0.85));
      if (blob) return blob;
    }
  } catch {
    /* fall through to sending the file as it is */
  }
  if (PASS_THROUGH.test(file.type) && file.size <= MAX_ORIGINAL_BYTES) return file;
  throw new Error('This kind of image is not supported');
}
