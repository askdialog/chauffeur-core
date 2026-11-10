import { Component, type ReactNode } from "react";
import { Button, EmptyState } from "./ui";

/** Keeps a rendering bug in one view from blanking the whole window. */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error) {
    console.error("view crashed", error);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <EmptyState title="This view crashed">
        <div className="selectable font-mono text-[11.5px]">{this.state.error.message}</div>
        <div className="mt-3">
          <Button size="sm" onClick={() => this.setState({ error: null })}>
            Try again
          </Button>
        </div>
      </EmptyState>
    );
  }
}
