const enc = new TextEncoder();

export function randomBytes(len: number): Uint8Array {
  const out = new Uint8Array(len);
  crypto.getRandomValues(out);
  return out;
}

export function b64encode(bytes: Uint8Array): string {
  return btoa(String.fromCharCode(...bytes));
}

export function b64decode(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

// 32 字节随机 device_secret
export function genDeviceSecret(): string {
  return b64encode(randomBytes(32));
}

// base32 Crockford 风格（去掉 0,1,O,I 易混字符），6 字符 → 30 bit ≈ 1B 唯一空间
const ALPHA = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
export function genPairToken(): string {
  const bytes = randomBytes(6);
  let out = "";
  for (let i = 0; i < 6; i++) out += ALPHA[bytes[i]! % 32];
  return out;
}

export async function hmacSha256(secret: Uint8Array, msg: string): Promise<Uint8Array> {
  const key = await crypto.subtle.importKey(
    "raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(msg));
  return new Uint8Array(sig);
}

export async function hmacVerify(secret: Uint8Array, msg: string, sig: Uint8Array): Promise<boolean> {
  const key = await crypto.subtle.importKey(
    "raw", secret, { name: "HMAC", hash: "SHA-256" }, false, ["verify"],
  );
  return crypto.subtle.verify("HMAC", key, sig, enc.encode(msg));
}

// X25519 公钥校验：32 字节 base64
export function isValidX25519PubB64(s: string): boolean {
  try {
    const b = b64decode(s);
    return b.length === 32;
  } catch {
    return false;
  }
}
