// A file dropped where no drop target takes it: the web view's default is to open the file in place
// of the app (WebKitGTK navigates to its file:// URL), which leaves no way back but a restart.
// Only drags that bring files are refused: text or a link dropped in a text field keeps the
// browser's default, which a field takes without cancelling `dragover`.

import { bringsFiles } from "./fileDrops";

type DragLike = Pick<DragEvent, "defaultPrevented" | "preventDefault"> & {
  dataTransfer: Pick<DataTransfer, "types" | "dropEffect"> | null;
};

/**
 * For `dragover` and `drop` on the window: refuses a drag of files no drop target accepted on the
 * way up. `native` as for `bringsFiles`.
 */
export function refuseStrayDrop(event: DragLike, native: boolean): void {
  if (event.defaultPrevented || !event.dataTransfer) return;
  if (!bringsFiles(Array.from(event.dataTransfer.types), native)) return;
  event.preventDefault();
  event.dataTransfer.dropEffect = "none";
}
