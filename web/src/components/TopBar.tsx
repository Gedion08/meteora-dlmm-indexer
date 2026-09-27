import { useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { NavLink, useNavigate } from "react-router";
import { api, endpoints, type GlobalStats, type Status } from "../lib/api";
import { live, useLiveState } from "../lib/live";
import { AlertIcon, BrandMark, MoonIcon, SearchIcon, SunIcon } from "./icons";

const B58 = /^[1-9A-HJ-NP-Za-km-z]+$/;

async function resolve(term: string): Promise<string> {
  const t = term.trim();
  if (B58.test(t) && t.length >= 64 && t.length <= 90) return `/tx/${t}`;
  if (B58.test(t) && t.length >= 32 && t.length <= 44) {
    try {
      await api(endpoints.pair(t));
      return `/pool/${t}`;
    } catch {
      /* not a pool */
    }
    try {
      await api(endpoints.position(t));
      return `/position/${t}`;
    } catch {
      /* not a position */
    }
    return `/wallet/${t}`;
  }
  return `/?q=${encodeURIComponent(t)}`;
}

function Search() {
  const nav = useNavigate();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement;
      if (e.key === "/" && target.tagName !== "INPUT" && target.tagName !== "TEXTAREA") {
        e.preventDefault();
        input.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <form
      className="search"
      role="search"
      onSubmit={async (e) => {
        e.preventDefault();
        if (!value.trim() || busy) return;
        setBusy(true);
        try {
          nav(await resolve(value));
          setValue("");
          input.current?.blur();
        } finally {
          setBusy(false);
        }
      }}
    >
      <SearchIcon className="search__icon" />
      <label htmlFor="global-search" className="visually-hidden">
        Search pools, wallets, positions or transactions
      </label>
      <input
        id="global-search"
        ref={input}
        value={value}
        onChange={(e) => setValue(e.target.value)}
        placeholder="Token, pair, wallet, position or signature"
        autoComplete="off"
        spellCheck={false}
      />
      <span className="kbd" aria-hidden="true">
        {busy ? "…" : "/"}
      </span>
    </form>
  );
}

function StatusPill() {
  const ws = useLiveState();
  useEffect(() => {
    live.ensureConnected();
  }, []);
  const status = useQuery({
    queryKey: ["status"],
    queryFn: ({ signal }) => api<Status>(endpoints.status(), signal),
    refetchInterval: 5000,
  });
  const stats = useQuery({
    queryKey: ["stats"],
    queryFn: ({ signal }) => api<GlobalStats>(endpoints.stats(), signal),
    staleTime: 60_000,
  });
  const network = stats.data?.network ?? "…";
  const behind = status.data?.seconds_behind ?? null;

  let tone: "good" | "warning" | "critical";
  let label: string;
  if (status.isError) {
    tone = "critical";
    label = "API offline";
  } else if (behind != null && behind > 60) {
    tone = "critical";
    label = `Stalled · ${Math.round(behind)}s behind`;
  } else if (ws !== "live") {
    tone = "warning";
    label = ws === "connecting" ? "Connecting" : "Reconnecting";
  } else if (behind != null && behind > 10) {
    tone = "warning";
    label = `Delayed · ${Math.round(behind)}s`;
  } else {
    tone = "good";
    label = "Live";
  }
  return (
    <span className={`status status--${tone}`} role="status" title={behind != null ? `Index is ${behind.toFixed(1)}s behind the chain` : undefined}>
      <span className="status__icon">{tone === "good" ? <span className="status__dot" /> : <AlertIcon />}</span>
      {label}
      <span className="muted">· {network}</span>
    </span>
  );
}

function ThemeToggle() {
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    const stamped = document.documentElement.dataset.theme;
    if (stamped === "light" || stamped === "dark") return stamped;
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });
  const next = theme === "dark" ? "light" : "dark";
  return (
    <button
      type="button"
      className="icon-btn"
      aria-label={`Switch to ${next} theme`}
      title={`Switch to ${next} theme`}
      onClick={() => {
        document.documentElement.dataset.theme = next;
        try {
          localStorage.setItem("binscope-theme", next);
        } catch {
          /* storage unavailable */
        }
        setTheme(next);
        window.dispatchEvent(new Event("binscope-theme"));
      }}
    >
      {theme === "dark" ? <SunIcon /> : <MoonIcon />}
    </button>
  );
}

export function TopBar() {
  return (
    <header className="topbar">
      <div className="topbar__inner">
        <NavLink to="/" className="brand" aria-label="Binscope home">
          <BrandMark />
          Binscope
        </NavLink>
        <nav className="nav" aria-label="Main">
          <NavLink to="/" end>
            Pools
          </NavLink>
        </nav>
        <Search />
        <span className="topbar__spacer" />
        <StatusPill />
        <ThemeToggle />
      </div>
    </header>
  );
}
