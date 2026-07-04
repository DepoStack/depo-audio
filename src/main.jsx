import React from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider } from "next-themes";
import { TooltipProvider } from "./components/ui/tooltip";
import { PreferencesProvider } from "./hooks/PreferencesContext";
import App from "./App.jsx";
import "./globals.css";

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <ThemeProvider attribute="class" defaultTheme="dark" themes={['dark', 'light']} storageKey="depoaudio-theme">
      <TooltipProvider delayDuration={300}>
        <PreferencesProvider>
          <App />
        </PreferencesProvider>
      </TooltipProvider>
    </ThemeProvider>
  </React.StrictMode>
);
