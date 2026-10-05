"use client";

import dynamic from "next/dynamic";
import { ToastProvider } from "@/components/Toasts";

// The app is a pure client: it talks to the engine over IPC or HTTP at runtime.
const App = dynamic(() => import("@/components/App"), { ssr: false });

export default function Page() {
  return (
    <ToastProvider>
      <App />
    </ToastProvider>
  );
}
