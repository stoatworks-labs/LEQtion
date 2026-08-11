import { Component, type ErrorInfo, type ReactNode } from 'react';

/**
 * Stops one broken render from taking the whole application down.
 *
 * React unmounts the entire tree when a render throws and nothing catches it,
 * so before this existed a single bad property access in a toolbar panel left
 * an empty window — no tiles, no toolbar, no way back except relaunching. That
 * is the worst possible failure for this app in particular: the measurement is
 * still running in Rust, the log is still being written, and the operator can
 * see none of it.
 *
 * Two things follow from that, and both are deliberate:
 *
 * - **The boundary reports, it does not reload.** Reloading the webview would
 *   throw away the frame subscription and re-run startup while audio is live.
 *   The engine is not the thing that broke, so it is left alone and the user
 *   decides.
 * - **`Try again` remounts only the subtree.** It clears the captured error and
 *   re-renders the children; if the underlying data is still bad it will throw
 *   again and land back here, which is honest rather than a loop.
 *
 * Wrapped around each tile as well as the root, so a tile that cannot draw
 * degrades to a message inside its own frame and its neighbours keep updating.
 */
export class ErrorBoundary extends Component<
  { children: ReactNode; label?: string },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Goes to the webview console, which the dev build has open and which a bug
    // report can be asked for. The Rust log is not reachable from here.
    console.error('[LEQtion] render failed', error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (!error) return this.props.children;

    const what = this.props.label ? `${this.props.label} could not be drawn` : 'Something broke';

    return (
      <div className="render-error" role="alert">
        <p>
          <strong>{what}.</strong> The measurement is still running — this is a display fault,
          not a fault in the audio or the log.
        </p>
        <p className="render-error-detail">{error.message}</p>
        <button type="button" onClick={() => this.setState({ error: null })}>
          Try again
        </button>
      </div>
    );
  }
}
