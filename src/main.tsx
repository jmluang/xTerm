import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";
import { ErrorBoundary } from "./debug/ErrorBoundary";
import { GlobalErrorOverlay } from "./debug/GlobalErrorOverlay";
import { installGlobalErrorHandlers } from "./debug/installGlobalErrorHandlers";
import { initTheme } from "./lib/theme";
import { initPerfMetrics } from "./lib/perfMetrics";

installGlobalErrorHandlers();
initTheme();
initPerfMetrics();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
      <GlobalErrorOverlay />
    </ErrorBoundary>
  </React.StrictMode>
);
