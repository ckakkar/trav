"use client";

import { AnimatePresence, motion } from "motion/react";
import { useEffect, useRef, type ReactNode } from "react";
import { X } from "lucide-react";
import { DIALOG, useRegister } from "@/lib/motion";

export const ease = [0.17, 0.84, 0.44, 1] as const;

const FOCUSABLE = 'a[href], button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])';

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
  const motionSet = DIALOG[useRegister()];
  // Parents pass a fresh `onClose` on every render (the engine polls twice a
  // second); focus must follow opening and closing only, not those renders.
  const close = useRef(onClose);
  useEffect(() => {
    close.current = onClose;
  }, [onClose]);
  useEffect(() => {
    if (!open) return;
    const prev = document.activeElement as HTMLElement | null;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        close.current();
      } else if (e.key === "Tab" && panel.current) {
        // Keep focus inside the dialog: Tab from the last control wraps to the first.
        const items = [...panel.current.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.closest("[inert]") && el.offsetParent !== null);
        if (!items.length) return;
        const first = items[0];
        const last = items[items.length - 1];
        const at = document.activeElement;
        const inside = at instanceof HTMLElement && panel.current.contains(at);
        if (!inside || (e.shiftKey && at === first) || (!e.shiftKey && at === last)) {
          e.preventDefault();
          (e.shiftKey ? last : first).focus();
        }
      }
    };
    window.addEventListener("keydown", onKey, true);
    requestAnimationFrame(() => {
      const root = panel.current;
      const first =
        root?.querySelector<HTMLElement>("[data-autofocus]:not(:disabled)") ??
        root?.querySelector<HTMLElement>(".modal-body input, .modal-body textarea, .modal-body select") ??
        root?.querySelector<HTMLElement>(".modal-foot button:not(:disabled)");
      first?.focus();
    });
    return () => {
      window.removeEventListener("keydown", onKey, true);
      prev?.focus?.();
    };
  }, [open]);

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
            initial={motionSet.initial}
            animate={motionSet.animate}
            exit={motionSet.exit}
            transition={motionSet.transition}
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
