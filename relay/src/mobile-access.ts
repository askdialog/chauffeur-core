import { createRemoteJWKSet, jwtVerify, type JWTVerifyGetKey } from "jose";

export type MobileGrant = { subject: string; expiresAt: number };

const googleKeys = createRemoteJWKSet(new URL("https://www.googleapis.com/oauth2/v3/certs"));

export function googleMobileAccess(
  audience: string,
  workspace: string,
  invitations: readonly string[],
  keys: JWTVerifyGetKey = googleKeys,
) {
  const invited = new Set(invitations.map(email => email.trim().toLowerCase()));
  return async (credential: string): Promise<MobileGrant | null> => {
    try {
      const { payload } = await jwtVerify(credential, keys, {
        algorithms: ["RS256"],
        issuer: ["https://accounts.google.com", "accounts.google.com"],
        audience,
        requiredClaims: ["sub", "exp", "email", "email_verified"],
      });
      if (!payload.sub || typeof payload.exp !== "number" ||
          payload.aud !== audience || (payload.azp !== undefined && payload.azp !== audience) ||
          typeof payload.email !== "string" || payload.email_verified !== true) return null;
      const email = payload.email.toLowerCase();
      const hostedDomain = typeof payload.hd === "string" ? payload.hd.toLowerCase() : "";
      // Google can vouch for current ownership of Gmail and Workspace addresses.
      const member = hostedDomain === workspace && email.endsWith(`@${workspace}`);
      const guest = invited.has(email) && (email.endsWith("@gmail.com") || hostedDomain !== "");
      return member || guest ? { subject: payload.sub, expiresAt: payload.exp * 1000 } : null;
    } catch {
      return null;
    }
  };
}
