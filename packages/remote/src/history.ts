// A mobile history request can trade round trips for bytes without exceeding
// the bounded encrypted JSON channel. Oversized pages are retried read-only.
export const HISTORY_PAGE_EVENTS = 200;
export const HISTORY_PAGE_BYTES = 512 * 1024;
export const HISTORY_PAGE_TOO_LARGE = "history_page_too_large";
