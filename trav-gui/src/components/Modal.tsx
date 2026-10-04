"use client";

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";

export const ease = [0.17, 0.84, 0.44, 1] as const;

export function Modal({
  open,
  onClose,
  children,
  width = 620,
  label,
  eyebrow,
  title,
}: {
  open: boolean;
  onClose: () => void;
  children: ReactNode;
  width?: number;
  label: string;
  eyebrow?: ReactNode;
  title?: ReactNode;
}) {
  const panel = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const prev = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true);
    requestAnimationFrame(() => {
      const first = panel.current?.querySelector<HTMLElement>("[data-autofocus], input, button, select, textarea");
      first?.focus();
    });
    return () => {
      window.removeEventListener("keydown", onKey, true);
      prev?.focus?.();
    };
  }, [open, onClose]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="modal-scrim"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.18 }}
          onMouseDown={(e) => e.target === e.currentTarget && onClose()}
        >
          <motion.div
            ref={panel}
            role="dialog"
            aria-modal="true"
            aria-label={label}
            className="modal"
            style={{ width: `min(${width}px, calc(100vw - 32px))` }}
            initial={{ opacity: 0, y: 14, scale: 0.985 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 8, scale: 0.99 }}
            transition={{ duration: 0.32, ease }}
          >
            {(eyebrow || title) && (
              <header className="modal-head">
                <div>
                  {eyebrow && <div className="eyebrow">{eyebrow}</div>}
                  {title && <h2 className="modal-title display">{title}</h2>}
                </div>
                <button className="icon-btn" onClick={onClose} aria-label="Close">
                  <X size={16} />
                </button>
              </header>
            )}
            {children}
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
