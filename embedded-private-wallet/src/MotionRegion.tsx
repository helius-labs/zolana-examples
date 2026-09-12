import { useLayoutEffect, useRef, type ReactNode } from "react";

/** Animates intrinsic height without guessing a max-height or delaying actions. */
export function MotionRegion({
  children,
  transitionKey,
  className = "",
}: {
  children: ReactNode;
  className?: string;
  // An outer region animates only identity changes, letting inner regions flow.
  transitionKey?: string | boolean;
}) {
  const outer = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  const height = useRef<number | null>(null);
  const animation = useRef<Animation | null>(null);
  const previousKey = useRef(transitionKey);
  const keyChanged = useRef(false);

  useLayoutEffect(() => {
    keyChanged.current = previousKey.current !== transitionKey;
    previousKey.current = transitionKey;
  }, [transitionKey]);

  useLayoutEffect(() => {
    const frame = outer.current;
    const content = inner.current;
    if (!frame || !content || typeof ResizeObserver === "undefined") return;
    const reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)");
    const resize = () => {
      const next = content.getBoundingClientRect().height;
      const previous = height.current;
      const shouldAnimate = transitionKey === undefined || keyChanged.current;
      keyChanged.current = false;
      height.current = next;
      if (previous === null || Math.abs(previous - next) < 0.5) return;
      const from = animation.current
        ? frame.getBoundingClientRect().height
        : previous;
      animation.current?.cancel();
      animation.current = null;
      if (!shouldAnimate || reduceMotion.matches || !frame.animate) return;
      const motion = frame.animate(
        [{ height: `${from}px` }, { height: `${next}px` }],
        {
          duration: 340,
          easing: "cubic-bezier(0.32, 0.72, 0, 1)",
        }
      );
      animation.current = motion;
      motion.onfinish = () => {
        if (animation.current === motion) animation.current = null;
      };
    };
    const stop = () => {
      if (reduceMotion.matches) {
        animation.current?.cancel();
        animation.current = null;
      }
    };
    resize();
    const observer = new ResizeObserver(resize);
    observer.observe(content);
    reduceMotion.addEventListener("change", stop);
    return () => {
      observer.disconnect();
      reduceMotion.removeEventListener("change", stop);
    };
  }, [children, transitionKey]);

  useLayoutEffect(
    () => () => {
      animation.current?.cancel();
    },
    []
  );

  return (
    <div ref={outer} className={`motion-region ${className}`}>
      <div ref={inner} className="motion-region-content">
        {children}
      </div>
    </div>
  );
}
