/** Only the browser continuation fields needed by the native PKCE handoff. */
export function mobileAuthContext(search = location.search) {
  const params = new URLSearchParams(search);
  const challenge = params.get("challenge");
  const state = params.get("state");
  if (params.get("mobile-auth") !== "1" || !challenge || !/^[A-Za-z0-9_-]{43}$/.test(challenge) || !state || !/^[a-f0-9]{32,128}$/i.test(state) || params.getAll("challenge").length !== 1 || params.getAll("state").length !== 1) return null;
  return { challenge, state, screen: params.get("screen") || undefined, thread: params.get("thread") || undefined };
}

/** A mail continuation must always offer the same review in the browser, without native proof. */
export function browserReviewUrl(input: string): string {
  const url = new URL(input);
  for (const key of ["mobile-auth", "challenge", "state", "mobile", "native"]) url.searchParams.delete(key);
  url.hash = "";
  return url.href;
}
