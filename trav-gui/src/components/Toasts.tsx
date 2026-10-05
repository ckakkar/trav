"use client";

import { AnimatePresence, motion } from "motion/react";
import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { ease } from "./Modal";

type Tone = "info" | "ok" | "error";
interface Toast {
  id: number;
  title: string;
  body?: string;
  tone: Tone;
}

const Ctx = createContext<(title: string, opts?: { body?: string; tone?: Tone }) => void>(() => {});

export const useToast = () => useContext(Ctx);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Toast[]>([]);
  const seq = useRef(0);
  const push = useCallback((title: string, opts?: { body?: string; tone?: Tone }) => {
    const id = ++seq.current;
    setItems((xs) => [...xs.slice(-3), { id, title, body: opts?.body, tone: opts?.tone ?? "info" }]);
    setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== id)), opts?.tone === "error" ? 7000 : 4200);
  }, []);
  return (
    <Ctx.Provider value={push}>
      {children}
      <div className="toasts" aria-live="polite">
        <AnimatePresence initial={false}>
          {items.map((t) => (
            <motion.div
              key={t.id}
              layout
              className={`toast toast-${t.tone}`}
              initial={{ opacity: 0, y: 16, scale: 0.98 }}
              animate={{ opacity: 1, y: 0, scale: 1 }}
              exit={{ opacity: 0, x: 24, transition: { duration: 0.18 } }}
              transition={{ duration: 0.34, ease }}
              onClick={() => setItems((xs) => xs.filter((x) => x.id !== t.id))}
            >
              <span className="toast-dot" />
              <div>
                <div className="toast-title">{t.title}</div>
                {t.body && <div className="toast-body">{t.body}</div>}
              </div>
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </Ctx.Provider>
  );
}
