export function mac_pub_b64(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return btoa(String.fromCharCode(...bytes));
}
export function ios_pub_b64(): string { return mac_pub_b64(); }
export function fakePairToken(): string { return "ABC234"; }
