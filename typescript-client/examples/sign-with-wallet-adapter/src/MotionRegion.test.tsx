// @vitest-environment jsdom
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { MotionRegion } from "./MotionRegion";

let contentHeight = 100;
let resize: () => void;
let reduced = false;
const cancel = vi.fn();
const animate = vi.fn(() => ({ cancel, onfinish: null }));
beforeEach(() => {
  contentHeight = 100;
  reduced = false;
  vi.clearAllMocks();
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        resize = callback;
      }
      observe() {}
      disconnect() {}
    }
  );
  vi.stubGlobal("matchMedia", () => ({
    matches: reduced,
    addEventListener() {},
    removeEventListener() {},
  }));
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(
    function (this: HTMLElement) {
      return {
        height: this.classList.contains("motion-region-content")
          ? contentHeight
          : 120,
      } as DOMRect;
    }
  );
  vi.stubGlobal("Animation", class {});
  Object.defineProperty(HTMLElement.prototype, "animate", {
    configurable: true,
    value: animate,
  });
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  delete (HTMLElement.prototype as Partial<HTMLElement>).animate;
});

it("animates expansion and retargets a rapid reversal from its visible height", () => {
  render(
    <MotionRegion>
      <p>Contents</p>
    </MotionRegion>
  );
  expect(animate).not.toHaveBeenCalled();
  contentHeight = 200;
  act(() => resize());
  expect(animate).toHaveBeenLastCalledWith(
    [{ height: "100px" }, { height: "200px" }],
    expect.objectContaining({ duration: 340 })
  );
  contentHeight = 80;
  act(() => resize());
  expect(cancel).toHaveBeenCalledTimes(1);
  expect(animate).toHaveBeenLastCalledWith(
    [{ height: "120px" }, { height: "80px" }],
    expect.anything()
  );
});
it("respects reduced motion and cancels work on unmount", () => {
  reduced = true;
  const view = render(
    <MotionRegion>
      <p>Contents</p>
    </MotionRegion>
  );
  contentHeight = 200;
  act(() => resize());
  expect(animate).not.toHaveBeenCalled();
  view.unmount();
});
it("lets inner height changes flow, animating the outer region only on identity changes", () => {
  const view = render(
    <MotionRegion transitionKey={false}>
      <p>Sign in</p>
    </MotionRegion>
  );
  contentHeight = 120;
  act(() => resize());
  expect(animate).not.toHaveBeenCalled();
  contentHeight = 400;
  view.rerender(
    <MotionRegion transitionKey={true}>
      <p>Wallet</p>
    </MotionRegion>
  );
  expect(animate).toHaveBeenCalledTimes(1);
  view.unmount();
  expect(cancel).toHaveBeenCalled();
});
