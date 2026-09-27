// Display helpers for tokens. Many devnet tokens have no metadata, so every token needs
// a stable fallback: a short mint and a deterministic mark derived from the address.

import { short } from "./format";

export function tokenLabel(symbol: string | null | undefined, mint: string | null | undefined): string {
  return symbol?.trim() || short(mint, 4, 3);
}

// FNV-1a over the mint → a hue on a restrained ring (no neon, fixed lightness/chroma),
// so marks are distinguishable but never compete with the data colors.
export function mintHue(mint: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < mint.length; i++) {
    h ^= mint.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0) % 360;
}

export function mintMarkStyle(mint: string): { background: string } {
  const hue = mintHue(mint);
  return { background: `oklch(0.52 0.09 ${hue})` };
}

export function initials(label: string): string {
  const clean = label.replace(/[^A-Za-z0-9]/g, "");
  return (clean.slice(0, 2) || "?").toUpperCase();
}
