"use client";

import dynamic from "next/dynamic";
import { useEffect } from "react";
import { ToastProvider } from "@/components/Toasts";
import { Recover } from "@/components/Recover";
import { installErrorReporting } from "@/lib/report";

// The app is a pure client: it talks to the engine over IPC or HTTP at runtime.
const App = dynamic(() => import("@/components/App"), { ssr: false });

export default function Page() {
  useEffect(installErrorReporting, []);
  return (
    <ToastProvider>
      {/* A restarted view skips the launch sequence. */}
      <Recover>{(restarts) => <App skipLaunch={restarts > 0} />}</Recover>
    </ToastProvider>
  );
}
