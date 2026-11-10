// Google's sign-in button on the share's page (story 51): Google Identity Services, loaded from
// accounts.google.com, gives an ID token of any Google account for the service's web OAuth
// client; the page trades it for the service's session cookie. Nothing else of Google is used.

type Credential = { credential: string };
type Google = {
  accounts: {
    id: {
      initialize(config: { client_id: string; callback: (response: Credential) => void; ux_mode?: "popup"; auto_select?: boolean }): void;
      renderButton(parent: HTMLElement, options: { theme: "outline" | "filled_blue" | "filled_black"; size: "large" | "medium" | "small"; text?: string; shape?: string; width?: number }): void;
    };
  };
};

declare global {
  interface Window {
    google?: Google;
  }
}

const SCRIPT = "https://accounts.google.com/gsi/client";
let loading: Promise<Google> | null = null;

/** Google's library, loaded once; rejects when the page cannot reach accounts.google.com. */
export function loadGoogle(): Promise<Google> {
  if (window.google) return Promise.resolve(window.google);
  loading ??= new Promise((resolve, reject) => {
    const script = document.createElement("script");
    script.src = SCRIPT;
    script.async = true;
    script.onload = () => (window.google ? resolve(window.google) : reject(new Error("Google's sign-in did not load")));
    script.onerror = () => reject(new Error("Could not reach accounts.google.com"));
    document.head.appendChild(script);
  });
  return loading;
}

/** Google's "Continue with Google" button in `parent`, `width` pixels wide; `onToken` gets the ID
 *  token of the account picked. */
export async function renderSignIn(parent: HTMLElement, clientId: string, width: number, onToken: (idToken: string) => void): Promise<void> {
  const google = await loadGoogle();
  google.accounts.id.initialize({ client_id: clientId, callback: ({ credential }) => onToken(credential), ux_mode: "popup" });
  google.accounts.id.renderButton(parent, { theme: "filled_black", size: "large", text: "continue_with", shape: "rectangular", width });
}
