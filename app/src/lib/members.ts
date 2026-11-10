// What a member of the distribution sees that others do not (docs/adr/members-only-features.md):
// the features that rely on the distribution's hosted services. Outside members they leave no
// trace: hidden, never greyed.

import type { ExtensionId, ExtensionInfo, IdentityState } from "@chauffeur/protocol";

/** Extensions only members are offered: the Office, its EOW board with it. */
const MEMBERS_ONLY_EXTENSIONS: readonly ExtensionId[] = ["office"];

/** Settings sections only members see. */
export const MEMBERS_ONLY_SECTIONS: readonly string[] = ["slack"];

/** The extensions this user is offered; all of them for a member. */
export function visibleExtensions(extensions: ExtensionInfo[], member: boolean): ExtensionInfo[] {
  return member ? extensions : extensions.filter((extension) => !MEMBERS_ONLY_EXTENSIONS.includes(extension.id));
}

/** Set once the sign-in banner was dismissed, or the user seen signed in: it never comes back. */
export const SIGN_IN_BANNER_KEY = "chauffeur.signInBannerDone";

/** The sidebar's sign-in banner shows to a user signed out who can sign in, until it is done. */
export function signInBannerShown(identity: IdentityState | null, done: boolean): boolean {
  return !done && identity !== null && identity.email === null && identity.can_sign_in;
}
