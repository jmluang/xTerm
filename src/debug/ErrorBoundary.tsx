import React from "react";

type Props = {
  children: React.ReactNode;
};

type State = {
  error: unknown;
};

export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: unknown): State {
    return { error };
  }

  componentDidCatch(error: unknown, info: unknown) {
    console.error("[ErrorBoundary]", error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;

    const message =
      this.state.error instanceof Error
        ? `${this.state.error.name}: ${this.state.error.message}`
        : String(this.state.error);

    const stack = this.state.error instanceof Error ? this.state.error.stack : "";

    return (
      <div style={{ padding: 16, fontFamily: "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace" }}>
        <h1 style={{ margin: "0 0 12px 0", fontSize: 18 }}>App crashed (JS)</h1>
        <div style={{ whiteSpace: "pre-wrap", color: "#b91c1c" }}>{message}</div>
        {stack ? (
          <pre style={{ marginTop: 12, whiteSpace: "pre-wrap", opacity: 0.85 }}>{stack}</pre>
        ) : null}
      </div>
    );
  }
}

