// One shared WebSocket to /v1/ws for the whole app.
// - reconnects with backoff and re-subscribes everything after a reconnect;
// - maps server subscription ids to local handlers (the server acks in order);
// - on "lagged"/"resync" (or a reconnect) it notifies listeners so pages refetch via REST.

import { useEffect, useRef, useSyncExternalStore } from "react";
import { wsUrl } from "./api";

export type Channel = "swaps" | "events" | "pairs";
export interface SubFilter {
  channel: Channel;
  lb_pair?: string;
  wallet?: string;
  names?: string[];
}
type Handler = (data: unknown) => void;
export type LiveState = "connecting" | "live" | "offline";

interface LocalSub {
  filter: SubFilter;
  handler: Handler;
  serverId: number | null;
  active: boolean;
}

class LiveClient {
  private ws: WebSocket | null = null;
  private subs = new Set<LocalSub>();
  private awaitingAck: LocalSub[] = [];
  private byId = new Map<number, LocalSub>();
  private retry = 0;
  private keepAlive = false;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private state: LiveState = "connecting";
  private stateListeners = new Set<() => void>();
  private resyncListeners = new Set<() => void>();

  getState = () => this.state;

  onState = (fn: () => void) => {
    this.stateListeners.add(fn);
    return () => {
      this.stateListeners.delete(fn);
    };
  };

  onResync(fn: () => void): () => void {
    this.resyncListeners.add(fn);
    return () => {
      this.resyncListeners.delete(fn);
    };
  }

  private setState(s: LiveState) {
    if (s === this.state) return;
    this.state = s;
    this.stateListeners.forEach((f) => f());
  }

  private connect() {
    if (this.ws || typeof window === "undefined") return;
    this.setState("connecting");
    const ws = new WebSocket(wsUrl());
    this.ws = ws;
    ws.onopen = () => {
      this.retry = 0;
      this.setState("live");
      this.awaitingAck = [];
      this.byId.clear();
      for (const s of this.subs) this.sendSubscribe(s);
      // Anything pushed while we were disconnected is gone: let pages refetch.
      this.resyncListeners.forEach((f) => f());
    };
    ws.onmessage = (ev) => this.onMessage(ev.data as string);
    ws.onclose = () => {
      this.ws = null;
      this.setState("offline");
      if (this.subs.size === 0 && !this.keepAlive) return;
      const delay = Math.min(15_000, 500 * 2 ** this.retry++) + Math.random() * 300;
      this.timer = setTimeout(() => {
        this.timer = null;
        this.connect();
      }, delay);
    };
  }

  private sendSubscribe(s: LocalSub) {
    s.serverId = null;
    this.awaitingAck.push(s);
    this.ws?.send(JSON.stringify({ op: "subscribe", ...s.filter }));
  }

  private onMessage(raw: string) {
    let m: { type: string; id?: number; sub?: number; data?: unknown };
    try {
      m = JSON.parse(raw);
    } catch {
      return;
    }
    switch (m.type) {
      case "subscribed": {
        const s = this.awaitingAck.shift();
        if (!s || m.id == null) return;
        if (!s.active) {
          this.ws?.send(JSON.stringify({ op: "unsubscribe", id: m.id }));
          return;
        }
        s.serverId = m.id;
        this.byId.set(m.id, s);
        return;
      }
      case "swap":
      case "event":
      case "pair": {
        const s = m.sub != null ? this.byId.get(m.sub) : undefined;
        if (s?.active) s.handler(m.data);
        return;
      }
      case "lagged":
      case "resync":
        this.resyncListeners.forEach((f) => f());
        return;
    }
  }

  subscribe(filter: SubFilter, handler: Handler): () => void {
    const s: LocalSub = { filter, handler, serverId: null, active: true };
    this.subs.add(s);
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    if (!this.ws) this.connect();
    else if (this.ws.readyState === WebSocket.OPEN) this.sendSubscribe(s);
    return () => {
      s.active = false;
      this.subs.delete(s);
      if (s.serverId != null) {
        this.byId.delete(s.serverId);
        if (this.ws?.readyState === WebSocket.OPEN)
          this.ws.send(JSON.stringify({ op: "unsubscribe", id: s.serverId }));
      }
    };
  }

  /** Keep a connection open for the status indicator even with no subscriptions. */
  ensureConnected() {
    this.keepAlive = true;
    if (!this.ws && !this.timer) this.connect();
  }
}

export const live = new LiveClient();

export function useLiveState(): LiveState {
  return useSyncExternalStore(live.onState, live.getState, live.getState);
}

/** Subscribe while mounted. The handler may change between renders without resubscribing. */
export function useLive<T>(filter: SubFilter | null, handler: (data: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;
  const key = filter ? JSON.stringify(filter) : null;
  useEffect(() => {
    if (!key) return;
    return live.subscribe(JSON.parse(key) as SubFilter, (d) => ref.current(d as T));
  }, [key]);
}

export function useResync(fn: () => void) {
  const ref = useRef(fn);
  ref.current = fn;
  useEffect(() => live.onResync(() => ref.current()), []);
}
