"use client";

// The launch sequence: the mark assembles itself (the arrow drops onto the
// line, which fills like a finished download), the wordmark rises, then the
// whole thing dissolves into the app. About a second and a half; any key or
// click skips it, and it's skipped outright with reduced motion.

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useState } from "react";
import { MarkIntro } from "./Mark";

const out = [0.17, 0.84, 0.44, 1] as const;
const WORD = "Trav";

/** `onReveal` fires as the overlay starts to dissolve (time for the app to animate in), `onGone` once it has. */
export function Launch({ onReveal, onGone }: { onReveal: () => void; onGone: () => void }) {
  const [shown, setShown] = useState(true);

  useEffect(() => {
    if (!shown) onReveal();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [shown]);

  useEffect(() => {
    const done = () => setShown(false);
    // Hold briefly after the line fills; a fallback in case the animation is cut short.
    const fallback = setTimeout(done, 2200);
    window.addEventListener("keydown", done, { once: true });
    window.addEventListener("pointerdown", done, { once: true });
    return () => {
      clearTimeout(fallback);
      window.removeEventListener("keydown", done);
      window.removeEventListener("pointerdown", done);
    };
  }, []);

  return (
    <AnimatePresence onExitComplete={onGone}>
      {shown && (
        <motion.div
          className="launch"
          data-tauri-drag-region
          initial={{ opacity: 1 }}
          exit={{ opacity: 0, transition: { duration: 0.5, ease: out } }}
        >
          <motion.div
            className="launch-stage"
            exit={{ scale: 1.08, filter: "blur(10px)", opacity: 0, transition: { duration: 0.5, ease: out } }}
          >
            <div className="launch-glow" />
            <MarkIntro className="launch-mark" onFilled={() => setTimeout(() => setShown(false), 260)} />
            <div className="launch-word display" aria-label={WORD}>
              {[...WORD].map((ch, i) => (
                <motion.span
                  key={i}
                  initial={{ opacity: 0, y: 14, filter: "blur(6px)" }}
                  animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
                  transition={{ delay: 0.62 + i * 0.06, duration: 0.55, ease: out }}
                >
                  {ch}
                </motion.span>
              ))}
            </div>
            <motion.div className="launch-sub mono" initial={{ opacity: 0 }} animate={{ opacity: 1 }} transition={{ delay: 0.95, duration: 0.5 }}>
              nova · bittorrent
            </motion.div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
