// Price candles (TradingView lightweight-charts) with volume in its own pane below —
// two stacked panes on a shared time axis rather than two y-scales on one plot.
import {
  CandlestickSeries,
  ColorType,
  CrosshairMode,
  HistogramSeries,
  createChart,
  type IChartApi,
  type ISeriesApi,
  type Time,
  type UTCTimestamp,
} from "lightweight-charts";
import { useEffect, useRef, useState } from "react";
import type { Candle } from "../lib/api";
import { compact, price as fmtPrice, rawToNumber } from "../lib/format";
import { useThemeColors, withAlpha } from "../lib/useThemeColors";

export interface Tick {
  time: number;
  price: number;
  volumeY: number;
}

interface Props {
  candles: Candle[];
  intervalSecs: number;
  decimalsY: number | null;
  quoteLabel: string;
  tick: Tick | null;
}

interface Bar {
  time: number;
  open: number;
  high: number;
  low: number;
  close: number;
  volume: number;
}

const localTime = (t: number) =>
  new Date(t * 1000).toLocaleString("en-US", { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });

export function CandleChart({ candles, intervalSecs, decimalsY, quoteLabel, tick }: Props) {
  const el = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const priceSeries = useRef<ISeriesApi<"Candlestick"> | null>(null);
  const volSeries = useRef<ISeriesApi<"Histogram"> | null>(null);
  const last = useRef<Bar | null>(null);
  const bars = useRef<Map<number, Bar>>(new Map());
  const [readout, setReadout] = useState<Bar | null>(null);
  // Latest candle for the readout; a ref alone wouldn't re-render the row.
  const [latest, setLatest] = useState<Bar | null>(null);
  const c = useThemeColors();

  // Create once.
  useEffect(() => {
    if (!el.current) return;
    const ch = createChart(el.current, {
      autoSize: true,
      crosshair: { mode: CrosshairMode.Normal },
      localization: { priceFormatter: (p: number) => fmtPrice(p), timeFormatter: (t: Time) => localTime(t as number) },
      timeScale: {
        timeVisible: true,
        secondsVisible: false,
        tickMarkFormatter: (t: Time) => {
          const d = new Date((t as number) * 1000);
          return intervalSecs >= 86400
            ? d.toLocaleDateString("en-US", { month: "short", day: "numeric" })
            : d.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit", hour12: false });
        },
      },
      rightPriceScale: { scaleMargins: { top: 0.12, bottom: 0.08 } },
    });
    priceSeries.current = ch.addSeries(CandlestickSeries, {
      priceFormat: { type: "custom", formatter: (p: number) => fmtPrice(p), minMove: 1e-12 },
      priceLineVisible: true,
      lastValueVisible: true,
    });
    volSeries.current = ch.addSeries(HistogramSeries, { priceFormat: { type: "volume" }, priceLineVisible: false, lastValueVisible: false }, 1);
    ch.panes()[1]?.setHeight(72);
    ch.subscribeCrosshairMove((param) => {
      const t = param.time as number | undefined;
      setReadout(t != null ? bars.current.get(t) ?? null : null);
    });
    chart.current = ch;
    return () => {
      ch.remove();
      chart.current = null;
    };
  }, []);

  // Theme.
  useEffect(() => {
    chart.current?.applyOptions({
      layout: {
        background: { type: ColorType.Solid, color: c.surface },
        textColor: c.ink3,
        fontFamily: c.fontUi,
        fontSize: 11,
        panes: { separatorColor: c.grid, separatorHoverColor: c.axis },
      },
      grid: { vertLines: { color: c.grid }, horzLines: { color: c.grid } },
      rightPriceScale: { borderColor: c.axis },
      timeScale: { borderColor: c.axis },
      crosshair: {
        vertLine: { color: c.ink3, labelBackgroundColor: c.ink2 },
        horzLine: { color: c.ink3, labelBackgroundColor: c.ink2 },
      },
    });
    priceSeries.current?.applyOptions({
      upColor: c.up,
      downColor: c.down,
      borderUpColor: c.up,
      borderDownColor: c.down,
      wickUpColor: c.up,
      wickDownColor: c.down,
      priceLineColor: c.ink3,
    });
    if (last.current) setData(); // recolor volume bars
  }, [c]);

  function volumeBar(b: Bar) {
    return { time: b.time as UTCTimestamp, value: b.volume, color: withAlpha(b.close >= b.open ? c.up : c.down, 0.45) };
  }

  function setData() {
    const list = [...bars.current.values()].sort((a, b) => a.time - b.time);
    priceSeries.current?.setData(list.map((b) => ({ ...b, time: b.time as UTCTimestamp })));
    volSeries.current?.setData(list.map(volumeBar));
  }

  // Data.
  useEffect(() => {
    const m = new Map<number, Bar>();
    for (const k of candles) {
      m.set(k.time, {
        time: k.time,
        open: k.open,
        high: k.high,
        low: k.low,
        close: k.close,
        volume: rawToNumber(k.volume_y, decimalsY ?? 0) ?? 0,
      });
    }
    bars.current = m;
    const sorted = [...m.values()].sort((a, b) => a.time - b.time);
    last.current = sorted[sorted.length - 1] ?? null;
    setLatest(last.current);
    setData();
    chart.current?.timeScale().fitContent();
  }, [candles, decimalsY]);

  // Live ticks extend the current candle or open the next one.
  useEffect(() => {
    if (!tick || !priceSeries.current) return;
    const bucket = Math.floor(tick.time / intervalSecs) * intervalSecs;
    const prev = last.current;
    if (prev && bucket < prev.time) return;
    const bar: Bar =
      prev && prev.time === bucket
        ? {
            ...prev,
            high: Math.max(prev.high, tick.price),
            low: Math.min(prev.low, tick.price),
            close: tick.price,
            volume: prev.volume + tick.volumeY,
          }
        : { time: bucket, open: prev?.close ?? tick.price, high: tick.price, low: tick.price, close: tick.price, volume: tick.volumeY };
    bar.high = Math.max(bar.high, bar.open);
    bar.low = Math.min(bar.low, bar.open);
    bars.current.set(bucket, bar);
    last.current = bar;
    setLatest(bar);
    priceSeries.current.update({ ...bar, time: bar.time as UTCTimestamp });
    volSeries.current?.update(volumeBar(bar));
  }, [tick]);

  const shown = readout ?? latest;
  const change = shown ? shown.close / shown.open - 1 : null;
  const val = (label: string, v: string) => (
    <span>
      {label} <b className="ohlc__v">{v}</b>
    </span>
  );
  return (
    <div className="chart-box">
      {/* OHLC readout for the hovered (or latest) candle, in its own row: never over the data. */}
      <div className="ohlc num" aria-live="off">
        {shown ? (
          <>
            {val("O", fmtPrice(shown.open))}
            {val("H", fmtPrice(shown.high))}
            {val("L", fmtPrice(shown.low))}
            {val("C", fmtPrice(shown.close))}
            {change != null && (
              <span className={change > 0 ? "up" : change < 0 ? "down" : ""}>
                {change > 0 ? "▲" : change < 0 ? "▼" : ""} {(change * 100).toFixed(2)}%
              </span>
            )}
            <span>
              Vol <b className="ohlc__v">{compact(shown.volume)}</b> {quoteLabel}
            </span>
          </>
        ) : (
          <span>&nbsp;</span>
        )}
      </div>
      <div ref={el} className="chart-box__plot" role="img" aria-label={`Price candles in ${quoteLabel}`} />
    </div>
  );
}
