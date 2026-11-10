// Files attached to a message before it is sent: pasted, dropped or picked; images shown as
// thumbnails that open full screen, other files by name. Used by the chat composer and the create dialog's first prompt.

import { Paperclip, X } from "lucide-react";
import { useRef, useState } from "react";
import { checkFileSize, checkMessageTotal, formatBytes, type PendingImage, readAttachment } from "@/lib/images";
import { bringsFiles, FILE_DROP_ATTRIBUTE, type FileDrop, registerDropTarget } from "@/lib/fileDrops";
import { nativeFileDrops, readDroppedFile } from "@/lib/native";
import { FileIcon } from "./FileIcon";
import { errorMessage } from "@/lib/rpc";
import { toast } from "@/state/app";
import { Lightbox } from "../Lightbox";
import { cx, IconButton, Spinner } from "../ui";

export function useImageAttachments(onAdded?: () => void, initial: () => PendingImage[] = () => []) {
  const [images, setImages] = useState<PendingImage[]>(initial);
  const [reading, setReading] = useState(0);
  const [dropping, setDropping] = useState(false);

  /** Each file is read only when its turn comes, a dropped one from the shell. */
  const attach = async (files: { name: string; read: () => Promise<PendingImage> }[]) => {
    if (files.length === 0) return;
    setReading((n) => n + files.length);
    let attached = images.reduce((sum, image) => sum + image.size, 0);
    for (const file of files) {
      try {
        const image = await file.read();
        checkMessageTotal(attached, image.size);
        attached += image.size;
        setImages((current) => [...current, image]);
      } catch (error) {
        toast(`Could not attach ${file.name || "the file"}: ${errorMessage(error)}`, "error");
      } finally {
        setReading((n) => n - 1);
      }
    }
    onAdded?.();
  };
  const addFiles = (files: File[]) => attach(files.map((file) => ({ name: file.name, read: () => readAttachment(file) })));
  const takeDrop = (drop: FileDrop) => {
    setDropping(false);
    void attach(
      drop.files.map((file, index) => ({
        name: file.name,
        read: async () => {
          checkFileSize({ type: file.mediaType, size: file.size });
          return readAttachment(await readDroppedFile(drop, index));
        },
      })),
    );
  };

  const remove = (id: string) =>
    setImages((current) => {
      const gone = current.find((image) => image.id === id);
      if (gone?.previewUrl) URL.revokeObjectURL(gone.previewUrl);
      return current.filter((image) => image.id !== id);
    });

  /** After sending: the thumbnails' object URLs are released. */
  const clear = () =>
    setImages((current) => {
      for (const image of current) if (image.previewUrl) URL.revokeObjectURL(image.previewUrl);
      return [];
    });

  /** Takes the attachments out of the composer, previews kept, while the message is sent. */
  const detach = (): PendingImage[] => {
    const taken = images;
    setImages([]);
    return taken;
  };
  /** Puts back attachments whose message could not be sent, before any added since. */
  const restore = (taken: PendingImage[]) => setImages((current) => [...taken, ...current]);
  /** Once their message is sent: the previews' object URLs are released. */
  const release = (taken: PendingImage[]) => {
    for (const image of taken) if (image.previewUrl) URL.revokeObjectURL(image.previewUrl);
  };

  const hasFiles = (event: React.DragEvent) => bringsFiles(Array.from(event.dataTransfer.types), nativeFileDrops);

  /** Spread on the element files are dropped on. */
  const dropTarget = {
    [FILE_DROP_ATTRIBUTE]: "",
    ref: (element: HTMLElement | null) => (element ? registerDropTarget(element, takeDrop) : undefined),
    onDragOver: (event: React.DragEvent) => {
      if (!hasFiles(event)) return;
      event.preventDefault();
      event.dataTransfer.dropEffect = "copy";
      setDropping(true);
    },
    onDragLeave: (event: React.DragEvent) => {
      if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setDropping(false);
    },
    onDrop: (event: React.DragEvent) => {
      if (!hasFiles(event)) return;
      event.preventDefault();
      setDropping(false);
      // On Linux the files come from the shell (`takeDrop`): WebKitGTK lists none here.
      void addFiles(Array.from(event.dataTransfer.files));
    },
  };

  /** For a text field: a pasted screenshot, or files copied in the Finder, become attachments. */
  const onPaste = (event: Pick<ClipboardEvent, "clipboardData" | "preventDefault">) => {
    const files = Array.from(event.clipboardData?.files ?? []);
    if (files.length === 0) return;
    // A screenshot comes alone; text copied with images still pastes as text.
    if (!event.clipboardData?.getData("text/plain")) event.preventDefault();
    void addFiles(files);
  };

  return { images, reading, dropping, addFiles, remove, clear, detach, restore, release, dropTarget, onPaste };
}

export function DropOverlay() {
  return (
    <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center rounded-lg border border-dashed border-accent bg-accent/10 text-[12.5px] text-fg">
      Drop files to attach them
    </div>
  );
}

export function ImageStrip({
  images,
  reading,
  onRemove,
  className,
}: {
  images: PendingImage[];
  reading: number;
  onRemove: (id: string) => void;
  className?: string;
}) {
  /** The open thumbnail's place among the images, which the Lightbox steps through. */
  const [open, setOpen] = useState<number | null>(null);
  const shown = images.filter((image) => image.previewUrl);
  // Rendered even once the strip is empty (a message sent with ↵), for it to close itself.
  const lightbox =
    open != null ? (
      <Lightbox images={shown.map((image) => ({ src: image.previewUrl, name: image.name }))} start={open} onClose={() => setOpen(null)} />
    ) : null;
  if (images.length === 0 && reading === 0) return lightbox;
  return (
    <div className={cx("flex flex-wrap gap-1.5", className)} data-testid="composer-images">
      {images.map((image) => (
        <div
          key={image.id}
          className={cx(
            "group relative h-14 overflow-hidden rounded-md border border-line bg-bg",
            image.previewUrl ? "w-14" : "flex max-w-[220px] items-center gap-2 px-2.5",
          )}
          title={image.name}
        >
          {image.previewUrl ? (
            <button type="button" onClick={() => setOpen(shown.indexOf(image))} className="block h-full w-full cursor-zoom-in">
              <img src={image.previewUrl} alt={image.name} className="h-full w-full object-cover" draggable={false} />
            </button>
          ) : (
            <>
              <FileIcon name={image.name} size={18} className="shrink-0 text-muted" />
              <span className="min-w-0">
                <span className="block truncate text-[12px] text-fg">{image.name}</span>
                <span className="block text-[11px] text-faint">{formatBytes(image.size)}</span>
              </span>
            </>
          )}
          <button
            type="button"
            title="Remove"
            onClick={() => onRemove(image.id)}
            className="absolute top-0.5 right-0.5 hidden h-4 w-4 items-center justify-center rounded-full bg-black/70 text-white group-hover:flex"
          >
            <X size={10} />
          </button>
        </div>
      ))}
      {reading > 0 && (
        <div className="flex h-14 w-14 items-center justify-center rounded-md border border-dashed border-line">
          <Spinner size={12} />
        </div>
      )}
      {lightbox}
    </div>
  );
}

export function AttachButton({ onFiles, disabled }: { onFiles: (files: File[]) => void; disabled?: boolean }) {
  const picker = useRef<HTMLInputElement>(null);
  return (
    <>
      <IconButton title="Attach images or files (or paste, or drop them)" onClick={() => picker.current?.click()} disabled={disabled}>
        <Paperclip size={13} />
      </IconButton>
      <input
        ref={picker}
        type="file"
        multiple
        hidden
        onChange={(event) => {
          onFiles(Array.from(event.target.files ?? []));
          event.target.value = "";
        }}
      />
    </>
  );
}
