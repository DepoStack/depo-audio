import React from "react";
import ReactDOM from "react-dom/client";
import { TooltipProvider } from "./components/common/Tooltip";
import App from "./App.jsx";
import "./App.css";

ReactDOM.createRoot(document.getElementById("root")).render(
  <React.StrictMode>
    <TooltipProvider>
      <App />
    </TooltipProvider>
  </React.StrictMode>
);
