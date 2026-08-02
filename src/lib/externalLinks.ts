import { invoke } from '@tauri-apps/api/core'

/**
 * Send external links to the system browser when running inside Tauri.
 *
 * A `target="_blank"` anchor works in a browser tab and does nothing at all in a
 * Tauri webview: Tauri denies the new-window request, silently. The About
 * dialog is all external links — the user guide, the project page, the source,
 * the four funding pages — so without this it looks broken in the desktop build
 * and fine everywhere else, which is the worst way for it to fail.
 *
 * `tauri-plugin-opener` is already a dependency and `opener:allow-open-url` is
 * already in the default capability, so this needs no new permission. It is
 * invoked through the core `invoke` rather than @tauri-apps/plugin-opener to
 * avoid pulling in a package for one call.
 *
 * Two details that are easy to get wrong:
 *
 *   - `event.target` is NOT the anchor when the click happens inside a shadow
 *     root — the browser retargets it to the shadow host, which is exactly what
 *     the About dialog is. `composedPath()` gives the real path through it.
 *   - The listener is on `document` in the capture phase so it still runs if
 *     something between the anchor and the document stops propagation.
 */
export function routeExternalLinksToBrowser(): void {
  // Every Tauri v2 webview has this; a browser tab does not, and there the
  // ordinary target="_blank" is already right.
  if (!('__TAURI_INTERNALS__' in window)) return

  document.addEventListener(
    'click',
    (event) => {
      if (event.defaultPrevented || event.button !== 0) return

      const anchor = event
        .composedPath()
        .find((node): node is HTMLAnchorElement => node instanceof HTMLAnchorElement && !!node.href)
      if (!anchor) return

      const url = new URL(anchor.href, location.href)
      if (url.protocol !== 'http:' && url.protocol !== 'https:') return

      event.preventDefault()
      void invoke('plugin:opener|open_url', { url: url.href }).catch((error: unknown) => {
        console.error('could not open', url.href, error)
      })
    },
    true
  )
}
