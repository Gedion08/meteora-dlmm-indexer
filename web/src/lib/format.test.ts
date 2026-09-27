import { describe, expect, it } from "vitest";
import { amount, binRatio, compact, pct, price, scaleRaw, short } from "./format";

describe("scaleRaw", () => {
  it("scales u64 strings exactly", () => {
    expect(scaleRaw("25343185513707", 6)).toBe("25343185.513707");
    expect(scaleRaw("18446744073709551615", 9)).toBe("18446744073.709551615");
    expect(scaleRaw("5", 6)).toBe("0.000005");
    expect(scaleRaw("1000000", 6)).toBe("1");
    expect(scaleRaw("0", 9)).toBe("0");
  });
});

describe("price", () => {
  it("uses 4 significant figures and subscript zeros for tiny prices", () => {
    expect(price(0.5)).toBe("0.5");
    expect(price(0.1234)).toBe("0.1234");
    expect(price(0.01234)).toBe("0.01234");
    expect(price(0.001)).toBe("0.001");
    expect(price(0.00011296228808)).toBe("0.000113");
    expect(price(0.0000480327)).toBe("0.0₄4803");
    expect(price(1.5e-12)).toBe("0.0₁₁15");
  });
  it("handles rounding into the next decade", () => {
    expect(price(0.099999)).toBe("0.1");
    expect(price(0.99999)).toBe("1.00");
  });
  it("formats large prices", () => {
    expect(price(1234.5678)).toBe("1,234.57");
    expect(price(12.3)).toBe("12.30");
    expect(price(2_500_000)).toBe("2.50M");
  });
});

describe("compact / amount / pct", () => {
  it("compacts magnitudes", () => {
    expect(compact(1284)).toBe("1,284");
    expect(compact(12900)).toBe("12.9K");
    expect(compact(4_210_000)).toBe("4.21M");
    expect(compact(null)).toBe("—");
  });
  it("formats raw token amounts with decimals", () => {
    expect(amount("25343185513707", 6)).toBe("25.34M");
    expect(amount("5051439539", 9)).toBe("5.05");
    expect(amount("123", null)).toBe("123 raw");
  });
  it("signs percentages with a real minus", () => {
    expect(pct(0.0639)).toBe("+6.39%");
    expect(pct(-0.0123)).toBe("−1.23%");
    expect(pct(0)).toBe("0.00%");
  });
  it("computes exact DLMM bin ratios", () => {
    expect(binRatio(10, 0)).toBe(0);
    expect(binRatio(100, 1)).toBeCloseTo(0.01, 12);
    expect(binRatio(10, 62)).toBeCloseTo(1.001 ** 62 - 1, 12);
  });
  it("shortens addresses", () => {
    expect(short("LBUZKhRxPF3XUpBCjp4YzTKgLccjZhTSDM9YuVaPwxo")).toBe("LBUZ…Pwxo");
    expect(short("3r1iZmqhyen4KbZfY6XpebF", 5, 0)).toBe("3r1iZ…");
    expect(short(null)).toBe("—");
  });
});
