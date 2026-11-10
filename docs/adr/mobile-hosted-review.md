# Review shared designs inside the mobile companion

- **Date:** 2026-11-08
- **Status:** accepted

Settled with Justin on 2026-11-08 while implementing the mobile review screens.

A shared design is the share service's hosted copy, separate from the paired Mac's
private Canvas. The companion presents the hosted review page in a restricted
WebView. Reviewers authenticate and comment through the existing service contracts;
the owner Mac can be offline. The service keeps the last screens and reviewer
comments until the owner reconnects. The phone does not publish a private Canvas.

The hosted page adapts to the phone: one interactive screen, comment pins and a
feedback panel, screen and thread lists, resolved filtering, replies and
resolve/reopen. Pin coordinates remain screen percentages, independent of fit or
zoom. Drafts survive connection failures in session storage, scoped by share,
author and target; a nonempty draft requires explicit discard before leaving.
Posting is confirmed by the service's comment event, not by sending a socket frame.
An unconfirmed send is reconciled against a fresh snapshot before offering retry.

Google and email authentication take place in the system browser, never inside
an embedded Google flow. The app generates a random verifier and state in memory
and passes only its S256 challenge to the browser's hosted sign-in page.
After cookie authentication and the share ACL check, the reviewer explicitly
consents to signing the app in; the page names the account and actual design and
offers View in this browser with every native auth parameter removed. Only then
does the page request a
60-second, one-use code, bound to identity, share, challenge, state and exact
screen/thread target. The fixed callback is `chauffeur://review/auth`; it carries
only code and state, never a session cookie or bearer token.

The app validates pending state, loads the trusted share page, and injects a
main-frame-only same-origin fetch exchanging the code with its verifier. Android
WebView ignores custom headers for URI-source POSTs, hence the fetch. The service
validates configured Origin and custom header, verifies PKCE and state, rechecks
ACL, sets its existing HttpOnly cookie and redirects to the stored target.
Design documents remain opaque sandboxed frames and receive no native messaging
bridge, verifier, pairing credentials or session cookie.

Only documented design service origins and the controlled custom scheme are
accepted. Navigation guards handle every top-level navigation. Universal/App Link
association deployment is separate: verified Apple team and Android signing
certificate identifiers were not available in the repository, so credentials are
not invented. Physical iOS/Android callback, cookie, browser handoff and navigation
checks are required before release. No service deployment or store submission is
implied.

Codes are in service memory. A restart invalidates them; multiple instances need
affinity or a shared atomic expiring store before using this flow across instances.

The callback scheme is fixed but app ownership is not verified by the operating
system without deployed Universal/App Link association. PKCE protects interception
of a legitimate app request; it cannot establish who originated an arbitrary
challenge in a mailed link. No mobile authorization happens automatically. The
exchanged cookie is the existing 30-day reviewer identity cookie, usable wherever
that identity has access; its scope is not limited to this design. Shorter-lived,
share-scoped mobile sessions remain a possible hardening step, not an implemented
property of this handoff.
