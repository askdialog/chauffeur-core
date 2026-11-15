# Slide decks are chat deliverables with a reader, presenting and text editing

- **Date:** 2026-11-14
- **Status:** accepted

## Context

Chauffeur is not only for developers: people ask it for presentations. A deck today is an HTML or PDF deliverable read as a scrolling document, with no slides, no presenting and no way to fix a word without asking the agent. [chat-document-deliverables](chat-document-deliverables.md) left dedicated slides for later. Justin asked for slides on 2026-11-13, taking Claude Slides as the reference.

## Decision

Decided with Justin 2026-11-13, 2026-11-14 and 2026-11-15 (comments in the first version):

- **A deck is a deliverable of its normal chat**, like HTML and PDF documents, never a design chat. It is a folder `.chauffeur/artifacts/<deck>/` holding `deck.json`, one self-contained HTML file per slide and shared CSS and assets beside them. `deck.json` reads `{"title", "width", "height", "brand"?, "slides": [{"file", "title", "notes"?, "skip"?}]}`, 1920 × 1080 by default. The agent declares it in the `deliverable` fence with `"kind": "deck"` and `path` naming `deck.json`.
- **The agent works as Claude Slides does**: it asks the purpose, the audience and the sources when they are missing, may propose an outline first, leaves missing figures as visible placeholders and names them in its reply, writes speaker notes when useful, and changes only the slides named by number or title.
- **The deck reader**, beside the chat as the document reader: a column of numbered thumbnails (drag to reorder, duplicate, skip, delete), the current slide scaled to fit, its speaker notes below (editable), ← and → between slides.
- **Retyping text by double-click, from the first version.** In the reader, a double-click on a text of a slide makes it editable in place: plain text, bold, italic and line breaks; pasting pastes plain text; Esc cancels, a click outside or ⌘⏎ saves. Text is retyped, never moved or resized. A text that overflows its box after the edit is outlined as a warning.
- **Present** runs the deck full screen, skipped slides left out. **Present with notes** opens the presenter's page in the user's browser, with the notes, a clock, the current and the next slide; its "Open the slides" opens the window shown to the audience, which that page drives.
- **Downloads**: PDF (one page per slide), the current slide as PNG, and one self-contained HTML that plays the deck in a browser. Skipped slides are left out.
- **Comments on a slide's element, from the first version**: with comments on, a click selects an element of a slide (a text, a chart, a picture); the user writes a comment and sends it to the agent with "Send to agent" (on by default). The comment goes to the deck's chat as a message naming the slide, the element and its text; the agent edits that slide and answers in the comment's thread, which shows on the element as a numbered pin. A thread is resolved by hand or stays open.
- **Later stages, decided now**: a shared deck opening in presentation mode, with comments for its reviewers; PowerPoint export with text, tables, pictures and notes editable, charts, icons and shapes as pictures, with the deck's fonts or universal ones.

## Consequences

- Every change by hand (retyped text, notes, order, skip, duplicate, delete) is an RPC: the daemon writes the files, as it owns every write. A text edit sends the slide, the element's position in the slide's source, its text before and after; the daemon refuses when the source no longer holds that text there ("This slide changed, try again"), so an agent's write is never overwritten. Only text written in the HTML source is editable; text a script draws (a chart) is not.
- At its next turn the agent is told which slides the user changed by hand, so that it keeps those changes.
- Slides are served by the screens' listener and framed sandboxed, as design screens are. Text editing goes through a script the daemon adds to the slide only when the reader asks for it, talking to the app by `postMessage`; the frame keeps no access to the app.
- The presenter's page is served once on the screens' listener, as an export's page is; the app keeps no second window of its own.
- PDF, PNG and HTML downloads use the headless Chrome the daemon already finds for design exports; without one, the download says what to install.
- Agent instructions (`DOCUMENT_NOTICE`) gain the deck layout, the fence's `kind` and the rules above; a deck requested from a document stays a separate deliverable.
- Every change, by the agent or by hand, makes a version ([deliverable-versions](deliverable-versions.md)); a deck's look comes from a brand ([brands](brands.md)).
- Left out: moving or resizing elements by hand, adding shapes or images by hand, real-time co-editing, Google Slides export, transitions.
