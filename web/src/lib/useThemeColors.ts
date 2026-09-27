import { useEffect, useState } from "react";

export interface ThemeColors {
  up: string;
  down: string;
  series: string;
  ink: string;
  ink2: string;
  ink3: string;
  grid: string;
  axis: string;
  surface: string;
  fontUi: string;
}

function read(): ThemeColors {
  const s = getComputedStyle(document.documentElement);
  const v = (name: string) => s.getPropertyValue(name).trim();
  return {
    up: v("--up"),
    down: v("--down"),
    series: v("--series"),
    ink: v("--ink"),
    ink2: v("--ink-2"),
    ink3: v("--ink-3"),
    grid: v("--grid"),
    axis: v("--axis"),
    surface: v("--surface"),
    fontUi: v("--font-ui"),
  };
}

/** Chart libraries need concrete colors; re-read the tokens whenever the theme changes. */
export function useThemeColors(): ThemeColors {
  const [colors, setColors] = useState(read);
  useEffect(() => {
    const update = () => setColors(read());
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    mq.addEventListener("change", update);
    window.addEventListener("binscope-theme", update);
    return () => {
      mq.removeEventListener("change", update);
      window.removeEventListener("binscope-theme", update);
    };
  }, []);
  return colors;
}

export function withAlpha(hex: string, alpha: number): string {
  const h = hex.replace("#", "");
  if (h.length !== 6) return hex;
  const n = parseInt(h, 16);
  return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${alpha})`;
}
