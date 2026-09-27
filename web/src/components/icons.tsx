// Minimal line icons (16px grid, 1.5px stroke) so the UI has one consistent icon voice.
import type { SVGProps } from "react";

const base = (p: SVGProps<SVGSVGElement>) => ({
  width: 16,
  height: 16,
  viewBox: "0 0 16 16",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.5,
  strokeLinecap: "round" as const,
  strokeLinejoin: "round" as const,
  "aria-hidden": true,
  ...p,
});

export const SearchIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base(p)}>
    <circle cx="7" cy="7" r="4.5" />
    <path d="m10.5 10.5 3 3" />
  </svg>
);
export const SunIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base(p)}>
    <circle cx="8" cy="8" r="3" />
    <path d="M8 1.5v1.5M8 13v1.5M1.5 8H3M13 8h1.5M3.4 3.4l1 1M11.6 11.6l1 1M3.4 12.6l1-1M11.6 4.4l1-1" />
  </svg>
);
export const MoonIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base(p)}>
    <path d="M13.5 9.5A5.5 5.5 0 0 1 6.5 2.5a5.5 5.5 0 1 0 7 7Z" />
  </svg>
);
export const CopyIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base({ width: 13, height: 13, ...p })}>
    <rect x="5" y="5" width="8.5" height="8.5" rx="1.5" />
    <path d="M10.5 5V3.5A1.5 1.5 0 0 0 9 2H3.5A1.5 1.5 0 0 0 2 3.5V9a1.5 1.5 0 0 0 1.5 1.5H5" />
  </svg>
);
export const CheckIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base({ width: 13, height: 13, ...p })}>
    <path d="m3 8.5 3 3 7-7" />
  </svg>
);
export const ChevronIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base({ width: 12, height: 12, ...p })}>
    <path d="m6 3.5 4.5 4.5L6 12.5" />
  </svg>
);
export const AlertIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base({ width: 13, height: 13, ...p })}>
    <path d="M8 2 1.5 13.5h13L8 2Z" />
    <path d="M8 6.5v3M8 11.6v.1" />
  </svg>
);
export const ExternalIcon = (p: SVGProps<SVGSVGElement>) => (
  <svg {...base({ width: 12, height: 12, ...p })}>
    <path d="M9 2.5h4.5V7M13.5 2.5 7 9M11.5 9.5v3a1 1 0 0 1-1 1h-7a1 1 0 0 1-1-1v-7a1 1 0 0 1 1-1h3" />
  </svg>
);

/** Brand mark: five bins, the active one in the accent. */
export const BrandMark = () => (
  <svg width="22" height="22" viewBox="0 0 22 22" aria-hidden="true">
    <rect x="1" y="11" width="3" height="9" rx="1" fill="var(--series)" />
    <rect x="5.2" y="7" width="3" height="13" rx="1" fill="var(--series)" />
    <rect x="9.5" y="2" width="3" height="18" rx="1" fill="var(--accent)" />
    <rect x="13.8" y="6" width="3" height="14" rx="1" fill="var(--series)" />
    <rect x="18" y="12" width="3" height="8" rx="1" fill="var(--series)" />
  </svg>
);
