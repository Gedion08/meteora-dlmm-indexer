import { Component, type ErrorInfo, type ReactNode } from "react";

const CHUNK_ERROR = /dynamically imported module|Importing a module script failed|Loading chunk|Failed to fetch dynamically/i;
const RELOAD_FLAG = "binscope-chunk-reload";

interface State {
  error: Error | null;
}

/**
 * Keeps one broken page from blanking the whole app. A failed lazy-chunk load almost
 * always means a new version was deployed while this tab was open: reload once to pick it
 * up. Anything else shows a recoverable error instead of an empty screen.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    if (CHUNK_ERROR.test(error.message)) {
      let reloaded = false;
      try {
        reloaded = sessionStorage.getItem(RELOAD_FLAG) === "1";
        sessionStorage.setItem(RELOAD_FLAG, "1");
      } catch {
        /* storage unavailable: still reload once per page load */
      }
      if (!reloaded) window.location.reload();
      return;
    }
    console.error("Page crashed", error, info.componentStack);
  }

  componentDidMount() {
    try {
      sessionStorage.removeItem(RELOAD_FLAG);
    } catch {
      /* ignore */
    }
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;
    const isUpdate = CHUNK_ERROR.test(error.message);
    return (
      <div className="state" role="alert">
        <h3>{isUpdate ? "Binscope was updated" : "This page hit a problem"}</h3>
        <p>
          {isUpdate
            ? "A new version is available. Reload to continue."
            : "Something on this page failed to render. Reloading usually fixes it."}
        </p>
        <button type="button" className="btn" onClick={() => window.location.reload()}>
          Reload
        </button>
      </div>
    );
  }
}
