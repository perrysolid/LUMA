import { describe, expect, it } from "vitest";
import { arrowBetween, clampInto, edgePoint, minVisible, overlapArea, placeLabel } from "./geometry";

const vp = { x: 0, y: 0, w: 1440, h: 900 };

describe("edgePoint", () => {
  it("leaves through the side facing the target", () => {
    const r = { x: 100, y: 100, w: 200, h: 100 };
    expect(edgePoint(r, { x: 1000, y: 150 })).toEqual({ x: 300, y: 150 });
    expect(edgePoint(r, { x: 200, y: -500 })).toEqual({ x: 200, y: 100 });
    const p = edgePoint(r, { x: 1000, y: 150 }, 5);
    expect(p.x).toBe(305);
  });
});

describe("arrowBetween", () => {
  it("connects borders, not centres, and points at the target", () => {
    const a = arrowBetween({ x: 0, y: 0, w: 100, h: 100 }, { x: 400, y: 0, w: 100, h: 100 }, 0, 0);
    expect(a.start).toEqual({ x: 100, y: 50 });
    expect(a.end).toEqual({ x: 400, y: 50 });
    expect(a.endAngle).toBeCloseTo(0);
  });
  it("bends to one side", () => {
    const a = arrowBetween({ x: 0, y: 0, w: 100, h: 100 }, { x: 400, y: 0, w: 100, h: 100 });
    expect(a.control.y).not.toBe(50);
    expect(a.mid.x).toBeGreaterThan(100);
    expect(a.mid.x).toBeLessThan(400);
  });
});

describe("placeLabel", () => {
  const size = { w: 120, h: 28 };
  it("prefers above the target", () => {
    const p = placeLabel({ x: 500, y: 400, w: 200, h: 80 }, size, vp, []);
    expect(p.side).toBe("top");
    expect(p.rect.y + p.rect.h).toBeLessThanOrEqual(400);
  });
  it("goes below when the target hugs the top edge", () => {
    const p = placeLabel({ x: 500, y: 2, w: 200, h: 80 }, size, vp, []);
    expect(p.side).toBe("bottom");
    expect(p.rect.y).toBeGreaterThanOrEqual(82);
  });
  it("avoids existing labels", () => {
    const target = { x: 500, y: 400, w: 200, h: 80 };
    const first = placeLabel(target, size, vp, []);
    const second = placeLabel(target, size, vp, [first.rect]);
    expect(overlapArea(first.rect, second.rect)).toBe(0);
  });
  it("never leaves the viewport and never covers the target when avoidable", () => {
    for (const t of [
      { x: 1400, y: 860, w: 30, h: 30 },
      { x: 0, y: 0, w: 20, h: 20 },
      { x: 1300, y: 10, w: 140, h: 40 },
    ]) {
      const p = placeLabel(t, size, vp, []);
      expect(p.rect.x).toBeGreaterThanOrEqual(0);
      expect(p.rect.y).toBeGreaterThanOrEqual(0);
      expect(p.rect.x + p.rect.w).toBeLessThanOrEqual(vp.w);
      expect(p.rect.y + p.rect.h).toBeLessThanOrEqual(vp.h);
      expect(overlapArea(p.rect, t)).toBe(0);
    }
  });
  it("falls back inside a full-screen target", () => {
    const p = placeLabel({ x: 0, y: 0, w: 1440, h: 900 }, size, vp, []);
    expect(p.side).toBe("inside");
  });
});

describe("helpers", () => {
  it("clampInto keeps margin", () => {
    expect(clampInto({ x: -50, y: 2000, w: 100, h: 20 }, vp)).toEqual({ x: 6, y: 874, w: 100, h: 20 });
  });
  it("minVisible grows around the centre", () => {
    expect(minVisible({ x: 100, y: 100, w: 4, h: 4 })).toEqual({ x: 93, y: 93, w: 18, h: 18 });
  });
});
