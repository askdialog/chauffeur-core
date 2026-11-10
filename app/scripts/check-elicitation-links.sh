#!/usr/bin/env bash
# Run against an isolated dev server: bash scripts/check-elicitation-links.sh <browser-dir> <url>.
set -euo pipefail
browser="$(dirname "$0")/../../.agents/skills/verify-in-app/browser.ts"
"$browser" "$1" go "$2/#mcp-fixture"
"$browser" "$1" run - <<'JS'
await page.evaluate(() => {
  window.open = (url) => { document.body.dataset.openedUrl = String(url); return null; };
});
const resolved = page.locator('[data-testid="mcp-elicitation"]').filter({ hasText: 'Reconnect PostHog' });
await resolved.locator('a').click();
if (await page.locator('body').getAttribute('data-opened-url') !== 'https://chatgpt.com/apps/posthog/example') throw new Error('Answered confirmation link did not open');
if (await resolved.getByRole('button').count()) throw new Error('Answered confirmation can still be answered');
const pending = page.locator('[data-testid="mcp-elicitation"]').filter({ hasText: 'Review the confirmation page' });
await pending.getByRole('button', { name: 'Continue', exact: true }).click();
await pending.locator('a').click();
if (await page.locator('body').getAttribute('data-opened-url') !== 'https://example.com/confirm') throw new Error('Continuing removed access to the link');
await page.evaluate(() => { window.open = () => { throw new Error('Browser unavailable'); }; });
await resolved.locator('a').click();
await resolved.getByRole('alert').waitFor();
return 'Answered links open, Continue preserves access, and browser errors remain visible';
JS
