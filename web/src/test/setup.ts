// Test environment: a real DOM (happy-dom) so components can be rendered and
// interacted with, not just typechecked.
import { GlobalRegistrator } from "@happy-dom/global-registrator";

GlobalRegistrator.register({ url: "http://localhost/" });

// jsdom/happy-dom ship neither of these, and the console uses both:
// useCountUp drives number animations off rAF, and every motion path asks
// matchMedia whether the user prefers reduced motion.
if (!globalThis.requestAnimationFrame) {
  globalThis.requestAnimationFrame = ((cb: FrameRequestCallback) =>
    setTimeout(() => cb(performance.now()), 0) as unknown as number) as typeof requestAnimationFrame;
  globalThis.cancelAnimationFrame = ((id: number) =>
    clearTimeout(id)) as typeof cancelAnimationFrame;
}

if (!globalThis.matchMedia) {
  globalThis.matchMedia = ((query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  })) as unknown as typeof matchMedia;
}

// Pointer events: happy-dom has no PointerEvent, and the pause control is
// driven entirely by pointerdown/up.
if (!globalThis.PointerEvent) {
  globalThis.PointerEvent = globalThis.MouseEvent as unknown as typeof PointerEvent;
}
