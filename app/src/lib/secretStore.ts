import { isMac } from "./keys";

/**
 * Where the daemon keeps the user's credentials (`crates/chauffeurd/src/secrets.rs`), as the system
 * names it: the Keychain on macOS, the Secret Service keyring (GNOME Keyring, KWallet…) elsewhere.
 */
export function secretStore(french: boolean, mac = isMac): string {
  if (mac) return french ? "le trousseau" : "the Keychain";
  return french ? "le trousseau du système" : "the system keyring";
}
