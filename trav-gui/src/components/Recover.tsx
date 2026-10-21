"use client";

// An error while rendering would otherwise leave Next's dead "This page
// couldn't load" screen, and a dead window can't even fetch the update that
// fixes it. Instead: log the error, and restart the view (the engine and its
// transfers live on the native side and never notice). If it keeps failing,
// say so and offer a full reload.

import { Component, Fragment, type ReactNode } from "react";
import { reportError } from "@/lib/report";

/** Restarts allowed before giving up on doing it automatically… */
const RETRIES = 3;
const RETRY_MS = 900;
/** …counted afresh once the window has run this long without failing. */
const HEALTHY_MS = 60_000;

interface Props {
  /** Rendered with the restart count, so a restarted view can skip its intro. */
  children: (restarts: number) => ReactNode;
}

export class Recover extends Component<Props, { failed: boolean; restarts: number }> {
  state = { failed: false, restarts: 0 };
  private timer: ReturnType<typeof setTimeout> | undefined;
  private lastFailure = 0;

  static getDerivedStateFromError() {
    return { failed: true };
  }

  componentDidCatch(error: unknown, info: { componentStack?: string | null }) {
    reportError("render", error, info.componentStack ?? undefined);
    const now = Date.now();
    const restarts = now - this.lastFailure > HEALTHY_MS ? 0 : this.state.restarts;
    this.lastFailure = now;
    if (restarts !== this.state.restarts) this.setState({ restarts });
    if (restarts < RETRIES) this.timer = setTimeout(this.restart, RETRY_MS);
  }

  componentWillUnmount() {
    clearTimeout(this.timer);
  }

  restart = () => this.setState((s) => ({ failed: false, restarts: s.restarts + 1 }));

  render() {
    const { failed, restarts } = this.state;
    if (!failed) return <Fragment key={restarts}>{this.props.children(restarts)}</Fragment>;
    const giveUp = restarts >= RETRIES;
    return (
      <div className="recover" role="alert">
        <div className="recover-card">
          <h1 className="display recover-title">{giveUp ? "The window keeps failing." : "Restarting the window…"}</h1>
          <p className="recover-body">
            {giveUp
              ? "Transfers are still running. The error is in Trav's log, in ~/Library/Application Support/trav/logs. Reload to try again."
              : "Something in the window stopped working. Your transfers keep running."}
          </p>
          {giveUp && (
            <button className="btn primary" onClick={() => window.location.reload()}>
              Reload
            </button>
          )}
        </div>
      </div>
    );
  }
}
