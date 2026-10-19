"use client";

import { AnimatePresence, motion } from "motion/react";
import { createContext, useCallback, useContext, useRef, useState, type ReactNode } from "react";
import { TOAST, useRegister } from "@/lib/motion";

type Tone = "info" | "ok" | "error";
interface Toast {
  id: number;
  title: string;
  body?: string;
  tone: Tone;
  /** Milliseconds on screen; the countdown line runs for as long. */
  life: number;
}

const Ctx = createContext<(title: string, opts?: { body?: string; tone?: Tone }) => void>(() => {});

export const useToast = () => useContext(Ctx);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<Toast[]>([]);
  const seq = useRef(0);
  const choreo = TOAST[useRegister()];
  const push = useCallback((title: string, opts?: { body?: string; tone?: Tone }) => {
    const id = ++seq.current;
    const life = opts?.tone === "error" ? 7000 : 4200;
    setItems((xs) => [...xs.slice(-3), { id, title, body: opts?.body, tone: opts?.tone ?? "info", life }]);
    setTimeout(() => setItems((xs) => xs.filter((x) => x.id !== id)), life);
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
              initial={choreo.initial}
              animate={choreo.animate}
              exit={choreo.exit}
              transition={choreo.transition}
              onClick={() => setItems((xs) => xs.filter((x) => x.id !== t.id))}
            >
              <span className="toast-dot" />
              <div>
                <div className="toast-title">{t.title}</div>
                {t.body && <div className="toast-body">{t.body}</div>}
              </div>
              <span className="toast-life" style={{ animationDuration: `${t.life}ms` }} />
            </motion.div>
          ))}
        </AnimatePresence>
      </div>
    </Ctx.Provider>
  );
}
