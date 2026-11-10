// "Export PNG" of a design chat (story 47), kept pure so that it is tested in designExport.test.ts:
// which screens an export takes, what the PNG is named after, and what the toast says.

import type { DesignExportResult } from "@chauffeur/protocol";
import type { ShownSection } from "./canvas";

export type ExportRequest = { files: string[]; name: string };

/**
 * The selected screens of the page, in canvas order (missing files left out), named after the
 * one screen, the one section they all belong to, or their number. Null when none is selected.
 */
export function selectionExport(sections: ShownSection[], selection: readonly string[]): ExportRequest | null {
  const picked = sections.flatMap((section) =>
    section.screens.filter((screen) => screen.url != null && selection.includes(screen.file)).map((screen) => ({ section, screen })),
  );
  if (picked.length === 0) return null;
  const files = picked.map((p) => p.screen.file);
  const first = picked[0];
  if (picked.length === 1) return { files, name: first.screen.label?.trim() || first.screen.file };
  const oneSection = picked.every((p) => p.section === first.section);
  return { files, name: oneSection ? sectionName(first.section) : `${picked.length} screens` };
}

/** Every present screen of a section, named after it; null when it has none. */
export function sectionExport(section: ShownSection): ExportRequest | null {
  const files = section.screens.filter((screen) => screen.url != null).map((screen) => screen.file);
  return files.length > 0 ? { files, name: sectionName(section) } : null;
}

function sectionName(section: ShownSection): string {
  return section.title.trim() || (section.unsorted ? "Unsorted" : `${section.screens.length} screens`);
}

/** What the toast says once the PNG is written. */
export function exportedMessage(result: Pick<DesignExportResult, "path" | "copied">): string {
  const name = result.path.slice(result.path.lastIndexOf("/") + 1);
  return result.copied ? `Exported ${name}, copied to the clipboard` : `Exported ${name} into the design folder's exports/`;
}
