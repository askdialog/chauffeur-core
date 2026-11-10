// Files the user attaches to a message: pasted, dropped or picked. Images are scaled down and
// encoded before they are sent to the daemon; any other file (a zip, an HTML page) goes as it is,
// for the agent to open.

import type { ImageInput } from "@chauffeur/protocol";

export type PendingImage = {
  id: string;
  name: string;
  mediaType: string;
  /** Base64, without the data-URL prefix. */
  data: string;
  /** For the composer's thumbnail; null for a file that is not an image. */
  previewUrl: string | null;
  size: number;
};

/** The daemon's limit for one attached file. */
export const MAX_FILE_BYTES = 20 << 20;
/**
 * The files of one message together: they travel base64-encoded in one WebSocket message, which
 * the daemon caps at 64 MB (`MAX_MESSAGE_BYTES`); over it, it drops the connection.
 */
export const MAX_MESSAGE_FILE_BYTES = 40 << 20;

/** Refuses a file that would take a message's files, `attached` bytes so far, over their limit. */
export function checkMessageTotal(attached: number, size: number): void {
  if (attached + size > MAX_MESSAGE_FILE_BYTES)
    throw new Error(`the files of one message are limited to ${formatBytes(MAX_MESSAGE_FILE_BYTES)} together`);
}

/** Claude reads images up to about 1568 px on the long side without scaling them itself. */
export const MAX_EDGE = 2000;
/** Under the API's 5 MB once base64-encoded. */
export const MAX_ENCODED_BYTES = 3_700_000;
const TYPES = ["image/png", "image/jpeg", "image/gif", "image/webp"];

/** The size an image is scaled to so that its long side fits `max`. */
export function fitWithin(width: number, height: number, max = MAX_EDGE): { width: number; height: number } {
  const long = Math.max(width, height);
  if (long <= max) return { width, height };
  const scale = max / long;
  return { width: Math.max(1, Math.round(width * scale)), height: Math.max(1, Math.round(height * scale)) };
}

export function isImageFile(file: { type: string }): boolean {
  return TYPES.includes(file.type);
}

/** The image files among pasted or dropped items. */
export function imageFiles(list: FileList | File[] | null | undefined): File[] {
  return Array.from(list ?? []).filter(isImageFile);
}

/** Refuses a file over the daemon's limit, unless it is an image, which is scaled down once read. */
export function checkFileSize(file: { type: string; size: number }): void {
  if (!isImageFile(file) && file.size > MAX_FILE_BYTES)
    throw new Error(`${formatBytes(file.size)} is over the ${formatBytes(MAX_FILE_BYTES)} limit`);
}

/** An image is read as one (scaled down when large); any other file as its bytes, up to the daemon's limit. */
export async function readAttachment(file: File): Promise<PendingImage> {
  if (isImageFile(file)) return readImage(file);
  checkFileSize(file);
  return {
    id: crypto.randomUUID(),
    name: file.name || "attachment",
    // `text/html;charset=utf-8` → `text/html`.
    mediaType: file.type.split(";")[0].trim(),
    data: await base64(file),
    previewUrl: null,
    size: file.size,
  };
}

/** `2.4 MB`, `812 KB`, `90 B`. */
export function formatBytes(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1).replace(/\.0$/, "")} MB`;
  if (bytes >= 1 << 10) return `${Math.round(bytes / (1 << 10))} KB`;
  return `${bytes} B`;
}

/**
 * Reads an image, scaled down when it is large (Retina screenshots are) and re-encoded as JPEG
 * only when PNG stays too heavy. GIFs keep their bytes, animation included.
 */
export async function readImage(file: File): Promise<PendingImage> {
  const name = file.name && file.name !== "image.png" ? file.name : `Pasted image ${new Date().toLocaleTimeString()}.png`;
  let blob: Blob = file;
  if (file.type !== "image/gif") {
    const bitmap = await createImageBitmap(file);
    const size = fitWithin(bitmap.width, bitmap.height);
    if (size.width !== bitmap.width || file.size > MAX_ENCODED_BYTES) {
      const canvas = document.createElement("canvas");
      canvas.width = size.width;
      canvas.height = size.height;
      canvas.getContext("2d")?.drawImage(bitmap, 0, 0, size.width, size.height);
      blob = await toBlob(canvas, "image/png");
      if (blob.size > MAX_ENCODED_BYTES) blob = await toBlob(canvas, "image/jpeg", 0.88);
    }
    bitmap.close();
  }
  return {
    id: crypto.randomUUID(),
    name,
    mediaType: blob.type || file.type,
    data: await base64(blob),
    previewUrl: URL.createObjectURL(blob),
    size: blob.size,
  };
}

export function toImageInputs(images: PendingImage[]): ImageInput[] | undefined {
  if (images.length === 0) return undefined;
  return images.map((image) => ({ name: image.name, media_type: image.mediaType || undefined, data: image.data }));
}

function toBlob(canvas: HTMLCanvasElement, type: string, quality?: number): Promise<Blob> {
  return new Promise((resolve, reject) =>
    canvas.toBlob((blob) => (blob ? resolve(blob) : reject(new Error("could not encode the image"))), type, quality),
  );
}

async function base64(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = "";
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}
