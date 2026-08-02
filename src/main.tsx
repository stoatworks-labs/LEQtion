import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { routeExternalLinksToBrowser } from './lib/externalLinks';
import { App } from './ui/App';
import './styles.css';

// The About dialog's data file ships a version baked at sync time; this is the
// one the build actually produced. Spread, not assign: about-data.js may not
// have run yet, and it merges rather than overwriting. See public/about.js.
window.STOATWORKS_ABOUT = { ...window.STOATWORKS_ABOUT, version: __APP_VERSION__ };

// Tauri's webview silently refuses target="_blank". Every link in the About
// dialog is external, so without this the desktop build looks broken.
routeExternalLinksToBrowser();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
