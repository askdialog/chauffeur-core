// The packaged host receives credentials over its private stdin pipe.
let credential: string | undefined;

export function setHostCredential(value: string | undefined) {
  credential = value;
}

export function hostCredential(): string | undefined {
  return credential;
}
