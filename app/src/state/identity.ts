import { create } from "zustand";
import { client, setIdentity, toast } from "./app";
import { errorMessage } from "@/lib/rpc";

/** A sign-in or sign-out under way, and the last one's error; who is signed in is `useApp`'s `identity`. */
export const useIdentity = create<{ signingIn: boolean; error: string | null }>(() => ({ signingIn: false, error: null }));

/** Core sharing uses the user's identity even when Office is disabled. */
export async function signIn(): Promise<boolean> {
  if (useIdentity.getState().signingIn) return false;
  useIdentity.setState({ signingIn: true, error: null });
  try {
    setIdentity(await client.call("identity.sign_in", {}));
    return true;
  } catch (error) {
    const message = `Could not sign in: ${errorMessage(error)}`;
    useIdentity.setState({ error: message });
    toast(message, "error");
    return false;
  } finally {
    useIdentity.setState({ signingIn: false });
  }
}

export async function signOut(): Promise<void> {
  try {
    setIdentity(await client.call("identity.sign_out", {}));
  } catch (error) {
    toast(`Could not sign out: ${errorMessage(error)}`, "error");
  }
}
