import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Link, Outlet, Route, Routes, useLocation } from "react-router";
import { Suspense, lazy, useEffect } from "react";
import { EmptyState } from "./components/bits";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { GapBanner } from "./components/GapBanner";
import { TopBar } from "./components/TopBar";
import { PoolsPage } from "./pages/PoolsPage";

// Detail pages load on demand; the pool page carries the charting library.
const PoolPage = lazy(() => import("./pages/PoolPage").then((m) => ({ default: m.PoolPage })));
const WalletPage = lazy(() => import("./pages/WalletPage").then((m) => ({ default: m.WalletPage })));
const PositionPage = lazy(() => import("./pages/PositionPage").then((m) => ({ default: m.PositionPage })));
const TxPage = lazy(() => import("./pages/TxPage").then((m) => ({ default: m.TxPage })));
import { ApiError } from "./lib/api";

const client = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 10_000,
      refetchOnWindowFocus: false,
      // Don't retry "not found" / bad input; do retry transient failures once.
      retry: (count, err) => !(err instanceof ApiError && err.status >= 400 && err.status < 500) && count < 1,
    },
  },
});

function ScrollToTop() {
  const { pathname } = useLocation();
  useEffect(() => {
    window.scrollTo(0, 0);
  }, [pathname]);
  return null;
}

function Layout() {
  const { pathname } = useLocation();
  return (
    <>
      <ScrollToTop />
      <TopBar />
      <GapBanner />
      <main className="page" id="main">
        {/* Keyed by path: navigating away from a crashed page recovers. */}
        <ErrorBoundary key={pathname}>
          <Suspense fallback={<span className="skeleton" style={{ height: 240 }} aria-label="Loading" />}>
            <Outlet />
          </Suspense>
        </ErrorBoundary>
      </main>
      <footer className="footer">
        <span>Binscope · live index of the Meteora DLMM program</span>
        <span className="mono">LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo</span>
        <span>Data comes straight from the chain; nothing here is financial advice.</span>
      </footer>
    </>
  );
}

export function App() {
  return (
    <QueryClientProvider client={client}>
      <BrowserRouter>
        <Routes>
          <Route element={<Layout />}>
            <Route index element={<PoolsPage />} />
            <Route path="pool/:address" element={<PoolPage />} />
            <Route path="wallet/:address" element={<WalletPage />} />
            <Route path="position/:address" element={<PositionPage />} />
            <Route path="tx/:signature" element={<TxPage />} />
            <Route
              path="*"
              element={
                <EmptyState title="Page not found">
                  <Link to="/">Back to pools</Link>
                </EmptyState>
              }
            />
          </Route>
        </Routes>
      </BrowserRouter>
    </QueryClientProvider>
  );
}
